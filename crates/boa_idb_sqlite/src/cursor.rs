//! SQLite cursor implementation for all 4 directions.
//!
//! Cursors fetch rows lazily in pages (`LIMIT`/`OFFSET`) instead of
//! materializing whole ranges: large stores and externalized blobs only pay
//! for visited rows. Range predicates compile to plain comparisons (open vs
//! closed fixed at build time, bounds emitted only when present), so every
//! cursor query is served by the clustered primary-key index — no table scans
//! (covered by the EXPLAIN unit tests at the bottom).
//!
//! `*unique` directions collapse groups in SQL (`GROUP BY` + `MIN(pkey)`),
//! yielding the smallest primary key per index key as required.
//!
//! Seeking re-issues the query with an additional seek predicate and resets
//! paging, which makes seeks direction-aware by construction.

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendCursor;
use boa_idb_core::backend::types::CursorSeek;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::proto::{Direction, SourceRef, StoreId};
use rusqlite::Connection;

use crate::blob::BlobManager;

/// Rows fetched per page.
const PAGE_SIZE: i64 = 128;

/// Value slot of a buffered cursor entry.
enum EntryValue {
    /// No value (key-only cursor, or NULL value with no external file).
    None,
    /// Value stored inline in `records.value`.
    Inline(Vec<u8>),
    /// Value externalized in a blob file: relative path from `records.ext`
    /// plus cached bytes (read lazily, only for the row the cursor currently
    /// sits on, so paging never materializes more than one external value).
    External(String, Option<Vec<u8>>),
}

/// A cursor entry: (key, primary_key, value slot).
type CursorEntry = (Vec<u8>, Vec<u8>, EntryValue);

/// A bound query parameter.
#[derive(Debug, Clone)]
pub(crate) enum BoundParam {
    Int(i64),
    Blob(Vec<u8>),
}

impl BoundParam {
    pub(crate) fn to_sql(&self) -> Box<dyn rusqlite::types::ToSql> {
        match self {
            BoundParam::Int(v) => Box::new(*v),
            BoundParam::Blob(b) => Box::new(b.clone()),
        }
    }
}

/// What the cursor iterates.
#[derive(Clone, Copy)]
enum CursorKind {
    Store {
        store_id: StoreId,
    },
    Index {
        store_id: StoreId,
        index_id: u64,
        unique: bool,
    },
}

/// SQLite-backed cursor with lazy keyset-paged fetching.
pub struct SqliteCursor<'a> {
    conn: Option<&'a Connection>,
    kind: CursorKind,
    range: EncodedRange,
    /// Active seek target (adds predicates on the first page only).
    seek_key: Option<Vec<u8>>,
    seek_pkey: Option<Vec<u8>>,
    order_desc: bool,
    key_only: bool,
    blob_manager: Option<&'a BlobManager>,
    /// Current page window and absolute offset of its first row.
    buffer: Vec<CursorEntry>,
    buf_start: u64,
    /// Absolute position.
    pos: u64,
    exhausted: bool,
    /// Keyset anchor: value of the last represented row (buffered or
    /// drained) for resume. `None` until the first page is fetched.
    ///
    /// Replaces `OFFSET` (M7-B H-F2): SQLite re-walks absolute offsets on
    /// every page, making full scans quadratic. Resuming inclusively at the
    /// anchor costs one bounded index range scan per page.
    anchor_key: Option<Vec<u8>>,
    anchor_pkey: Option<Vec<u8>>,
    /// Rows equal to the anchor already represented (buffered or consumed).
    ///
    /// The inclusive resume predicate re-reads the anchor group from its
    /// first occurrence, so exactly this many leading rows are already known
    /// and must be skipped. Bounded by the duplicate-group size, not by the
    /// scan position: memory stays O(1) in the range size.
    anchor_eq: usize,
}

/// Cursor shape selector for [`SqliteCursor::debug_sql_with_literals`].
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub enum CursorKindRepr {
    /// Object-store cursor.
    Store,
    /// Plain index cursor.
    IndexPlain,
    /// Unique-deduped index cursor.
    IndexUnique,
}

impl<'a> SqliteCursor<'a> {
    /// Creates a new empty cursor (never fetches).
    pub fn new_empty(key_only: bool) -> Self {
        Self {
            conn: None,
            kind: CursorKind::Store { store_id: 0 },
            range: EncodedRange::all(),
            seek_key: None,
            seek_pkey: None,
            order_desc: false,
            key_only,
            blob_manager: None,
            buffer: Vec::new(),
            buf_start: 0,
            pos: 0,
            exhausted: true,
            anchor_key: None,
            anchor_pkey: None,
            anchor_eq: 0,
        }
    }

    /// Opens a cursor over an object store.
    pub fn open_store(
        conn: &'a Connection,
        store_id: StoreId,
        range: &EncodedRange,
        dir: Direction,
        key_only: bool,
        blob_manager: Option<&'a BlobManager>,
    ) -> Self {
        Self {
            conn: Some(conn),
            kind: CursorKind::Store { store_id },
            range: range.clone(),
            seek_key: None,
            seek_pkey: None,
            order_desc: dir.is_prev(),
            key_only,
            blob_manager,
            buffer: Vec::new(),
            buf_start: 0,
            pos: 0,
            exhausted: false,
            anchor_key: None,
            anchor_pkey: None,
            anchor_eq: 0,
        }
    }

    /// Opens a cursor over an index.
    pub fn open_index(
        conn: &'a Connection,
        store_id: StoreId,
        index_id: u64,
        range: &EncodedRange,
        dir: Direction,
        key_only: bool,
        blob_manager: Option<&'a BlobManager>,
    ) -> Self {
        Self {
            conn: Some(conn),
            kind: CursorKind::Index {
                store_id,
                index_id,
                unique: dir.is_unique(),
            },
            range: range.clone(),
            seek_key: None,
            seek_pkey: None,
            order_desc: dir.is_prev(),
            key_only,
            blob_manager,
            buffer: Vec::new(),
            buf_start: 0,
            pos: 0,
            exhausted: false,
            anchor_key: None,
            anchor_pkey: None,
            anchor_eq: 0,
        }
    }

    /// Builds the cursor SQL with bounds inlined as literals.
    ///
    /// Test hook (`#[doc(hidden)]`): lets EXPLAIN tests run the *exact*
    /// query shape the cursor executes, with bound values inlined as `X'..'`
    /// literals. Shares [`Self::build_query`]; `anchor` selects the paged
    /// resume shape so the planner is checked for follow-up pages too.
    #[doc(hidden)]
    pub fn debug_sql_with_literals(
        kind: CursorKindRepr,
        range: &EncodedRange,
        dir: Direction,
        key_only: bool,
        seek: Option<(Vec<u8>, Option<Vec<u8>>)>,
        anchor: Option<(&[u8], &[u8])>,
    ) -> String {
        let cursor = SqliteCursor {
            conn: None,
            kind: match kind {
                CursorKindRepr::Store => CursorKind::Store { store_id: 1 },
                CursorKindRepr::IndexUnique => CursorKind::Index {
                    store_id: 1,
                    index_id: 1,
                    unique: true,
                },
                CursorKindRepr::IndexPlain => CursorKind::Index {
                    store_id: 1,
                    index_id: 1,
                    unique: false,
                },
            },
            range: range.clone(),
            seek_key: seek.clone().map(|(k, _)| k),
            seek_pkey: seek.and_then(|(_, p)| p),
            order_desc: dir.is_prev(),
            key_only,
            blob_manager: None,
            buffer: Vec::new(),
            buf_start: 0,
            pos: 0,
            exhausted: false,
            anchor_key: None,
            anchor_pkey: None,
            anchor_eq: 0,
        };
        let (sql, params) = cursor.build_query(anchor);
        inline_literals(sql, &params)
    }

    /// Builds the paged resume SQL with `?` placeholders intact.
    ///
    /// Test hook (`#[doc(hidden)]`): literal inlining shows the
    /// constant-folded plan, but production prepares placeholders — and an
    /// `OR` resume that seeks with literals plans a bare `store_id` search
    /// with bound parameters (M7-B H-F2). Tests prepare `EXPLAIN QUERY PLAN`
    /// over this exact string with dummy bindings to check the generic plan
    /// seeks.
    #[doc(hidden)]
    pub fn debug_paged_sql(
        kind: CursorKindRepr,
        range: &EncodedRange,
        dir: Direction,
        key_only: bool,
        anchor: (&[u8], &[u8]),
    ) -> String {
        let cursor = SqliteCursor {
            conn: None,
            kind: match kind {
                CursorKindRepr::Store => CursorKind::Store { store_id: 1 },
                CursorKindRepr::IndexUnique => CursorKind::Index {
                    store_id: 1,
                    index_id: 1,
                    unique: true,
                },
                CursorKindRepr::IndexPlain => CursorKind::Index {
                    store_id: 1,
                    index_id: 1,
                    unique: false,
                },
            },
            range: range.clone(),
            seek_key: None,
            seek_pkey: None,
            order_desc: dir.is_prev(),
            key_only,
            blob_manager: None,
            buffer: Vec::new(),
            buf_start: 0,
            pos: 0,
            exhausted: false,
            anchor_key: None,
            anchor_pkey: None,
            anchor_eq: 0,
        };
        let (sql, _) = cursor.build_query(Some(anchor));
        sql
    }

    /// Builds the paged cursor query.
    ///
    /// `anchor` resumes after the last represented row (keyset pagination).
    /// The resume predicate is deliberately `OR`-free: with bound parameters
    /// SQLite cannot turn an `OR` into a range seek (it plans a bare
    /// `store_id` search and filters from the start — quadratic again).
    /// Stores use a single-column range (keys are unique), plain indexes a
    /// row-value range, grouped-unique shapes resume strictly after the
    /// group key. Inclusive resumes re-read the anchor; the caller skips the
    /// already-represented prefix. With no anchor the seek predicates select
    /// the first page. Pages always end at `LIMIT`; there is no `OFFSET`.
    fn build_query(&self, anchor: Option<(&[u8], &[u8])>) -> (String, Vec<BoundParam>) {
        let order = if self.order_desc { "DESC" } else { "ASC" };
        let mut params: Vec<BoundParam> = Vec::new();
        let sql = match self.kind {
            CursorKind::Store { store_id } => {
                let cols = if self.key_only {
                    "key, key"
                } else {
                    "key, key, value, ext"
                };
                let mut sql = format!("SELECT {cols} FROM records WHERE store_id = ?");
                params.push(BoundParam::Int(store_id as i64));
                append_range(&mut sql, "key", &self.range, &mut params);
                append_resume_store(
                    &mut sql,
                    self.order_desc,
                    self.seek_key.as_ref().map(|k| (k, self.seek_pkey.as_ref())),
                    anchor.map(|(k, _)| k),
                    &mut params,
                );
                sql.push_str(&format!(" ORDER BY key {order} LIMIT ?"));
                params.push(BoundParam::Int(PAGE_SIZE));
                sql
            }
            CursorKind::Index {
                store_id,
                index_id,
                unique,
            } => {
                if unique {
                    // Deterministic representative: smallest primary key.
                    let mut sql = String::from("SELECT g.key, g.pkey");
                    if !self.key_only {
                        sql.push_str(", r.value, r.ext");
                    }
                    sql.push_str(
                        " FROM (SELECT ir.key AS key, MIN(ir.pkey) AS pkey \
                         FROM index_records ir WHERE ir.index_id = ?",
                    );
                    params.push(BoundParam::Int(index_id as i64));
                    append_range(&mut sql, "ir.key", &self.range, &mut params);
                    append_resume_grouped(
                        &mut sql,
                        self.order_desc,
                        self.seek_key.as_ref(),
                        anchor.map(|(k, _)| k),
                        &mut params,
                    );
                    sql.push_str(" GROUP BY ir.key) g");
                    sql.push_str(" JOIN records r ON r.store_id = ? AND r.key = g.pkey");
                    params.push(BoundParam::Int(store_id as i64));
                    sql.push_str(&format!(" ORDER BY g.key {order} LIMIT ?"));
                    params.push(BoundParam::Int(PAGE_SIZE));
                    sql
                } else {
                    let cols = if self.key_only {
                        "ir.key, ir.pkey"
                    } else {
                        "ir.key, ir.pkey, r.value, r.ext"
                    };
                    // The records join filters orphaned index entries
                    // (stale pkey) while staying index-served.
                    let mut sql = format!(
                        "SELECT {cols} FROM index_records ir \
                         JOIN records r ON r.store_id = ? AND r.key = ir.pkey \
                         WHERE ir.index_id = ?"
                    );
                    params.push(BoundParam::Int(store_id as i64));
                    params.push(BoundParam::Int(index_id as i64));
                    append_range(&mut sql, "ir.key", &self.range, &mut params);
                    append_resume_index(
                        &mut sql,
                        self.order_desc,
                        self.seek_key.as_ref().map(|k| (k, self.seek_pkey.as_ref())),
                        anchor,
                        &mut params,
                    );
                    sql.push_str(&format!(
                        " ORDER BY ir.key {order}, ir.pkey {order} LIMIT ?"
                    ));
                    params.push(BoundParam::Int(PAGE_SIZE));
                    sql
                }
            }
        };
        (sql, params)
    }

    /// Records a newly buffered row for keyset resume.
    ///
    /// The anchor is always the last represented row and `anchor_eq` counts
    /// its already-represented duplicates (buffered or drained): drops only
    /// trim the head, so the tail statistics never need recomputation.
    fn note_appended(&mut self, key: &[u8], pkey: &[u8]) {
        let same = self
            .anchor_key
            .as_ref()
            .is_some_and(|k| k.as_slice() == key)
            && self
                .anchor_pkey
                .as_ref()
                .is_some_and(|p| p.as_slice() == pkey);
        if same {
            self.anchor_eq += 1;
        } else {
            self.anchor_key = Some(key.to_vec());
            self.anchor_pkey = Some(pkey.to_vec());
            self.anchor_eq = 1;
        }
    }

    /// Fetches the next page at the current buffer end.
    ///
    /// Returns `true` when rows were added. Never fetches past the end
    /// (empty page sets `exhausted`). Pages resume at the anchor with a
    /// bounded skip of already-represented duplicates — no `OFFSET`.
    fn fetch_next_page(&mut self) -> Result<bool, BackendError> {
        let Some(conn) = self.conn else {
            return Ok(false);
        };
        let grouped = matches!(self.kind, CursorKind::Index { unique: true, .. });
        // Owned clone: the fetch loop mutates the anchor via `note_appended`
        // while still comparing against it (two key-sized vectors per page).
        let anchor: Option<(Vec<u8>, Vec<u8>)> = match (&self.anchor_key, &self.anchor_pkey) {
            (Some(k), Some(p)) => Some((k.clone(), p.clone())),
            _ => None,
        };
        let anchor_ref = anchor.as_ref().map(|(k, p)| (k.as_slice(), p.as_slice()));
        // Grouped shapes resume strictly (no duplicates possible); plain
        // shapes resume inclusively and skip the represented prefix.
        let skip = if grouped { 0 } else { self.anchor_eq };
        let (sql, params) = self.build_query(anchor_ref);
        let boxes: Vec<Box<dyn rusqlite::types::ToSql>> =
            params.iter().map(BoundParam::to_sql).collect();
        let refs: Vec<&dyn rusqlite::types::ToSql> = boxes.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn
            .prepare_cached(&sql)
            .map_err(|e| BackendError::Internal(format!("Failed to prepare cursor SQL: {e}")))?;
        let col_count = stmt.column_count();
        let rows = stmt
            .query_map(refs.as_slice(), |row| {
                let key: Vec<u8> = row.get(0)?;
                let pkey: Vec<u8> = row.get(1)?;
                let value: Option<Vec<u8>> = if col_count > 2 { row.get(2)? } else { None };
                let ext: Option<String> = if col_count > 3 { row.get(3)? } else { None };
                Ok((key, pkey, value, ext))
            })
            .map_err(|e| BackendError::Internal(format!("Cursor query failed: {e}")))?;
        let mut added = false;
        let mut skipped = 0;
        for row in rows {
            let (key, pkey, value, ext) =
                row.map_err(|e| BackendError::Internal(format!("Cursor row error: {e}")))?;
            if skipped < skip && anchor_is_equal(anchor_ref, grouped, &key, &pkey) {
                skipped += 1;
                continue;
            }
            // Blob files are NOT read here: external values stay references
            // until the cursor visits their row (see `materialize_current`).
            let value = match (value, ext) {
                (Some(v), _) => EntryValue::Inline(v),
                (None, Some(rel)) => EntryValue::External(rel, None),
                (None, None) => EntryValue::None,
            };
            self.note_appended(&key, &pkey);
            self.buffer.push((key, pkey, value));
            added = true;
        }
        if !added {
            self.exhausted = true;
        }
        // Bound memory: drop fully-consumed prefix pages.
        if self.buffer.len() > 512 && self.pos > self.buf_start + 256 {
            let drop_n = (self.pos - self.buf_start) as usize;
            self.buffer.drain(..drop_n);
            self.buf_start = self.pos;
        }
        Ok(added)
    }

    /// Resolves the current row's value if it lives in an external blob file.
    ///
    /// Called after every position change, and it also drops the cached bytes
    /// of the previously visited row: at most one external value is held in
    /// memory at a time, no matter how large the store is.
    fn materialize_current(&mut self) -> Result<(), BackendError> {
        if self.key_only || self.exhausted {
            return Ok(());
        }
        // Free the previous row's cached bytes (no-op when already drained).
        if self.pos > self.buf_start {
            let prev_idx = (self.pos - 1 - self.buf_start) as usize;
            if let Some((.., EntryValue::External(_, cached))) = self.buffer.get_mut(prev_idx) {
                *cached = None;
            }
        }
        let idx = (self.pos - self.buf_start) as usize;
        let Some(entry) = self.buffer.get_mut(idx) else {
            return Ok(());
        };
        let rel = match &entry.2 {
            EntryValue::External(rel, None) => Some(rel.clone()),
            _ => None,
        };
        if let Some(rel) = rel {
            let data = match self.blob_manager {
                Some(bm) => bm.read(&rel)?,
                None => {
                    return Err(BackendError::Internal(
                        "Blob reference found but no blob manager".into(),
                    ));
                }
            };
            if let Some((.., slot)) = self.buffer.get_mut(idx) {
                *slot = EntryValue::External(rel, Some(data));
            }
        }
        Ok(())
    }

    /// Ensures the row at absolute `idx` is buffered (fetches pages).
    fn ensure(&mut self, idx: u64) -> Result<bool, BackendError> {
        if self.exhausted {
            return Ok(false);
        }
        while idx >= self.buf_start + self.buffer.len() as u64 {
            if !self.fetch_next_page()? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Buffered row at absolute `idx` (call `ensure` first).
    fn buffered(&self, idx: u64) -> Option<&CursorEntry> {
        self.buffer.get(idx.wrapping_sub(self.buf_start) as usize)
    }
}

impl<'a> BackendCursor for SqliteCursor<'a> {
    fn seek(&mut self, target: CursorSeek) -> Result<bool, BackendError> {
        if self.conn.is_none() {
            self.exhausted = true;
            return Ok(false);
        }
        match target {
            CursorSeek::First => {
                self.seek_key = None;
                self.seek_pkey = None;
            }
            CursorSeek::Key(key) => {
                self.seek_key = Some(key);
                self.seek_pkey = None;
            }
            CursorSeek::KeyAndPrimaryKey { key, pkey } => {
                self.seek_key = Some(key);
                self.seek_pkey = Some(pkey);
            }
        }
        // Re-query from the start: predicates are direction-aware, so the
        // first page already holds the seek position. The keyset anchor is
        // reset: the narrowed query re-establishes it from its first page.
        self.buffer.clear();
        self.buf_start = 0;
        self.pos = 0;
        self.exhausted = false;
        self.anchor_key = None;
        self.anchor_pkey = None;
        self.anchor_eq = 0;
        if !self.fetch_next_page()? {
            return Ok(false);
        }
        self.materialize_current()?;
        Ok(true)
    }

    fn step(&mut self, count: u32) -> Result<bool, BackendError> {
        if self.exhausted {
            return Ok(false);
        }
        self.pos = self.pos.saturating_add(u64::from(count));
        if !self.ensure(self.pos)? {
            self.exhausted = true;
            return Ok(false);
        }
        self.materialize_current()?;
        Ok(true)
    }

    fn current_key(&self) -> &[u8] {
        if self.exhausted {
            return &[];
        }
        self.buffered(self.pos).map_or(&[], |e| e.0.as_slice())
    }

    fn current_primary_key(&self) -> &[u8] {
        if self.exhausted {
            return &[];
        }
        self.buffered(self.pos).map_or(&[], |e| e.1.as_slice())
    }

    fn current_value(&self) -> Option<&[u8]> {
        if self.key_only || self.exhausted {
            return None;
        }
        self.buffered(self.pos).and_then(|e| match &e.2 {
            EntryValue::Inline(v) => Some(v.as_slice()),
            EntryValue::External(_, cached) => cached.as_deref(),
            EntryValue::None => None,
        })
    }
}

/// Compiles static range predicates for a key column.
///
/// Bounds are emitted only when present; open/closed is fixed at build time
/// so the planner sees plain comparisons served by the index.
#[allow(clippy::too_many_arguments)]
fn append_range(sql: &mut String, col: &str, range: &EncodedRange, params: &mut Vec<BoundParam>) {
    if let Some((key, open)) = &range.lower {
        let op = if *open { ">" } else { ">=" };
        sql.push_str(&format!(" AND {col} {op} ?"));
        params.push(BoundParam::Blob(key.clone()));
    }
    if let Some((key, open)) = &range.upper {
        let op = if *open { "<" } else { "<=" };
        sql.push_str(&format!(" AND {col} {op} ?"));
        params.push(BoundParam::Blob(key.clone()));
    }
}

/// Appends the seek predicate and its parameters.
///
/// `seek` is `Some((target_key, Option<target_pkey>))`; `key_col`/`pkey_col`
/// are the compared columns (`grouped` usage goes through
/// `append_seek_grouped`).
fn append_seek(
    sql: &mut String,
    key_col: &str,
    pkey_col: &str,
    desc: bool,
    seek: Option<(&Vec<u8>, Option<&Vec<u8>>)>,
    params: &mut Vec<BoundParam>,
) {
    let Some((target, seek_pkey)) = seek else {
        return;
    };
    match seek_pkey {
        Some(pkey) => {
            let (kop, pop) = if desc { ("<", "<=") } else { (">", ">=") };
            sql.push_str(&format!(
                " AND ({key_col} {kop} ? OR ({key_col} = ? AND {pkey_col} {pop} ?))"
            ));
            params.push(BoundParam::Blob(target.clone()));
            params.push(BoundParam::Blob(target.clone()));
            params.push(BoundParam::Blob(pkey.clone()));
        }
        None => {
            let cmp = if desc { "<=" } else { ">=" };
            sql.push_str(&format!(" AND {key_col} {cmp} ?"));
            params.push(BoundParam::Blob(target.clone()));
        }
    }
}

/// Appends the seek predicate for grouped unique queries (group key only).
fn append_seek_grouped(
    sql: &mut String,
    desc: bool,
    seek_key: Option<&Vec<u8>>,
    params: &mut Vec<BoundParam>,
) {
    if let Some(target) = seek_key {
        let cmp = if desc { "<=" } else { ">=" };
        sql.push_str(&format!(" AND ir.key {cmp} ?"));
        params.push(BoundParam::Blob(target.clone()));
    }
}

/// Appends the resume predicate for store shapes: the keyset anchor when
/// paging, the seek predicates for the first page. Exactly one of the two
/// applies: the anchor always dominates the seek position (pages advance
/// monotonically from it), so emitting both would only duplicate parameters.
///
/// Single-column range: store keys are unique, so inclusivity plus the
/// caller-side skip is exact. Crucially there is no `OR`: an `OR` resume
/// plans a bare `store_id` search with bound parameters (verified by
/// experiment) and reintroduces the quadratic cliff.
#[allow(clippy::too_many_arguments)]
fn append_resume_store(
    sql: &mut String,
    desc: bool,
    seek: Option<(&Vec<u8>, Option<&Vec<u8>>)>,
    anchor_key: Option<&[u8]>,
    params: &mut Vec<BoundParam>,
) {
    if let Some(key) = anchor_key {
        let cmp = if desc { "<=" } else { ">=" };
        sql.push_str(&format!(" AND key {cmp} ?"));
        params.push(BoundParam::Blob(key.to_vec()));
        return;
    }
    append_seek(sql, "key", "key", desc, seek, params);
}

/// Appends the resume predicate for plain index shapes: the keyset anchor
/// as a row-value range when paging, the seek predicates for the first page.
///
/// Row values `(ir.key, ir.pkey) >= (?, ?)` keep the multi-column range seek
/// with bound parameters, where an `OR` pair predicate would not (same
/// cliff as stores). The caller skips the re-read anchor prefix.
#[allow(clippy::too_many_arguments)]
fn append_resume_index(
    sql: &mut String,
    desc: bool,
    seek: Option<(&Vec<u8>, Option<&Vec<u8>>)>,
    anchor: Option<(&[u8], &[u8])>,
    params: &mut Vec<BoundParam>,
) {
    if let Some((anchor_key, anchor_pkey)) = anchor {
        let cmp = if desc { "<=" } else { ">=" };
        sql.push_str(&format!(" AND (ir.key, ir.pkey) {cmp} (?, ?)"));
        params.push(BoundParam::Blob(anchor_key.to_vec()));
        params.push(BoundParam::Blob(anchor_pkey.to_vec()));
        return;
    }
    append_seek(sql, "ir.key", "ir.pkey", desc, seek, params);
}

/// Appends the resume predicate for grouped unique queries (group key only).
fn append_resume_grouped(
    sql: &mut String,
    desc: bool,
    seek_key: Option<&Vec<u8>>,
    anchor_key: Option<&[u8]>,
    params: &mut Vec<BoundParam>,
) {
    if let Some(key) = anchor_key {
        // Groups are distinct: resume strictly after the anchor group.
        let cmp = if desc { "<" } else { ">" };
        sql.push_str(&format!(" AND ir.key {cmp} ?"));
        params.push(BoundParam::Blob(key.to_vec()));
        return;
    }
    append_seek_grouped(sql, desc, seek_key, params);
}

/// Tests whether a fetched row equals the resume anchor (skip check).
///
/// Grouped shapes resume strictly, so anchor rows cannot reappear; the check
/// is belt-and-braces there.
fn anchor_is_equal(anchor: Option<(&[u8], &[u8])>, grouped: bool, key: &[u8], pkey: &[u8]) -> bool {
    match anchor {
        None => false,
        Some((ak, ap)) => {
            if grouped {
                key == ak
            } else {
                key == ak && pkey == ap
            }
        }
    }
}

/// Builds the `count` query over a range for a store or index source.
///
/// Shared with `SqliteTxn::count` so the EXPLAIN tests exercise the exact
/// SQL the backend executes (R8.2.3).
pub(crate) fn build_count_sql(src: SourceRef, range: &EncodedRange) -> (String, Vec<BoundParam>) {
    let (table, id_col, id) = match src {
        SourceRef::Store(store) => ("records", "store_id", store),
        SourceRef::Index { index, .. } => ("index_records", "index_id", index),
    };
    let mut sql = format!("SELECT COUNT(*) FROM {table} WHERE {id_col} = ?");
    let mut params = vec![BoundParam::Int(id as i64)];
    append_range(&mut sql, "key", range, &mut params);
    (sql, params)
}

/// Inlines bound parameters into a query as SQL literals (test hooks).
fn inline_literals(mut sql: String, params: &[BoundParam]) -> String {
    for param in params {
        let literal = match param {
            BoundParam::Int(v) => v.to_string(),
            BoundParam::Blob(b) => format!("X'{}'", crate::naming::hex_encode(b)),
        };
        sql = sql.replacen('?', &literal, 1);
    }
    sql
}

/// Builds the exact `count` query with bounds inlined as literals.
///
/// Test hook (`#[doc(hidden)]`), mirroring [`SqliteCursor::debug_sql_with_literals`].
#[doc(hidden)]
pub fn debug_count_sql_with_literals(src: SourceRef, range: &EncodedRange) -> String {
    let (sql, params) = build_count_sql(src, range);
    inline_literals(sql, &params)
}
