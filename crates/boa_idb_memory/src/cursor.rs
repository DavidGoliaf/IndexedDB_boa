//! Lazy in-memory cursor: merge-iterates committed maps with the
//! transaction's pending overlay, one buffered row (M7-B H-MEM).
//!
//! The previous implementation collected the whole range (keys AND values)
//! into a `Vec` at `scan` time: a 1M walk peaked at ~327 MB. This cursor
//! re-seeks per step (`BTreeMap` ranges, O(log n)) and merges the two
//! sources with tombstone/shadow handling, so memory stays O(1) in the
//! range size. Values are cloned only for the yielded row (unless
//! `key_only`). No locks are held across steps.
//!
//! Snapshot note: committed rows are re-read per step, so a concurrent
//! commit mid-walk is visible, unlike the old scan-time snapshot. The crate
//! declares `snapshot_isolation: false` for exactly this reason; the L1
//! driver never branches on the flag and walks backend cursors
//! synchronously, so no in-tree flow can observe a torn walk.
//! Same-transaction writes stay visible through the pending overlay
//! (read-your-writes, matching siblings).

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendCursor;
use boa_idb_core::backend::types::CursorSeek;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::proto::{Direction, IndexId, StoreId};
use parking_lot::RwLock;

use crate::storage::{IndexKey, RecordKey, StorageState};

/// Type alias for cursor entries: (key, primary_key, optional_value).
type CursorEntry = (Vec<u8>, Vec<u8>, Option<Vec<u8>>);

/// What the cursor iterates.
#[derive(Clone, Copy)]
pub(crate) enum CursorKind {
    Store {
        store: StoreId,
    },
    Index {
        store: StoreId,
        index: IndexId,
        unique: bool,
    },
}

/// Lazy merge cursor over committed storage plus the pending overlay.
pub struct MemoryCursor<'a> {
    committed: Arc<RwLock<StorageState>>,
    pending_records: &'a BTreeMap<RecordKey, Option<Vec<u8>>>,
    pending_index: &'a BTreeMap<IndexKey, bool>,
    kind: CursorKind,
    range: EncodedRange,
    desc: bool,
    key_only: bool,
    /// Last examined committed record key (exclusive resume).
    committed_record_pos: Option<RecordKey>,
    /// Last examined pending record key (exclusive resume).
    pending_record_pos: Option<RecordKey>,
    /// Last examined committed index key (exclusive resume).
    committed_index_pos: Option<IndexKey>,
    /// Last examined pending index key (exclusive resume).
    pending_index_pos: Option<IndexKey>,
    /// Inclusive seek floor for record keys (clamped with the range).
    floor_record: Option<(RecordKey, bool)>,
    /// Inclusive seek floor for index keys (clamped with the range).
    floor_index: Option<(IndexKey, bool)>,
    /// Last yielded index key (unique dedup).
    last_index_key: Option<Vec<u8>>,
    /// The single buffered current row.
    current: Option<CursorEntry>,
    exhausted: bool,
}

impl<'a> MemoryCursor<'a> {
    /// Opens a cursor; fetches nothing until positioned.
    pub(crate) fn open(
        committed: Arc<RwLock<StorageState>>,
        pending_records: &'a BTreeMap<RecordKey, Option<Vec<u8>>>,
        pending_index: &'a BTreeMap<IndexKey, bool>,
        kind: CursorKind,
        range: EncodedRange,
        dir: Direction,
        key_only: bool,
    ) -> Self {
        Self {
            committed,
            pending_records,
            pending_index,
            kind,
            range,
            desc: dir.is_prev(),
            key_only,
            committed_record_pos: None,
            pending_record_pos: None,
            committed_index_pos: None,
            pending_index_pos: None,
            floor_record: None,
            floor_index: None,
            last_index_key: None,
            current: None,
            exhausted: true,
        }
    }

    /// Opens a store cursor.
    pub(crate) fn open_store(
        committed: Arc<RwLock<StorageState>>,
        pending_records: &'a BTreeMap<RecordKey, Option<Vec<u8>>>,
        pending_index: &'a BTreeMap<IndexKey, bool>,
        store: StoreId,
        range: &EncodedRange,
        dir: Direction,
        key_only: bool,
    ) -> Self {
        Self::open(
            committed,
            pending_records,
            pending_index,
            CursorKind::Store { store },
            range.clone(),
            dir,
            key_only,
        )
    }

    /// Opens an index cursor.
    ///
    /// Eight parameters mirror [`Self::open_store`] plus index identity;
    /// packing them into a struct would obscure the call sites for no gain.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_index(
        committed: Arc<RwLock<StorageState>>,
        pending_records: &'a BTreeMap<RecordKey, Option<Vec<u8>>>,
        pending_index: &'a BTreeMap<IndexKey, bool>,
        store: StoreId,
        index: IndexId,
        range: &EncodedRange,
        dir: Direction,
        key_only: bool,
    ) -> Self {
        Self::open(
            committed,
            pending_records,
            pending_index,
            CursorKind::Index {
                store,
                index,
                unique: dir.is_unique(),
            },
            range.clone(),
            dir,
            key_only,
        )
    }

    /// Start bound for forward record scans: resume position wins, else the
    /// seek floor clamped with the range lower bound (narrower of the two),
    /// else the range lower bound mapped into store scope.
    fn start_record_lower(&self, store: StoreId) -> Bound<RecordKey> {
        if let Some(pos) = &self.committed_record_pos {
            return Bound::Excluded(pos.clone());
        }
        // Narrower of seek floor and range lower wins; ties keep the
        // exclusive (open) flag.
        let mut key = (store, Vec::new());
        let mut inclusive = true;
        if let Some((lo, open)) = &self.range.lower {
            key = (store, lo.clone());
            inclusive = !open;
        }
        if let Some((floor, _)) = &self.floor_record {
            if floor.1 > key.1 {
                key = floor.clone();
                inclusive = true;
            }
        }
        if inclusive {
            Bound::Included(key)
        } else {
            Bound::Excluded(key)
        }
    }

    /// Start bound for backward record scans (mirror).
    fn start_record_upper(&self, store: StoreId) -> Bound<RecordKey> {
        if let Some(pos) = &self.committed_record_pos {
            return Bound::Excluded(pos.clone());
        }
        let mut key = (store, Vec::new());
        let mut inclusive = false;
        let mut bounded = false;
        if let Some((hi, open)) = &self.range.upper {
            key = (store, hi.clone());
            inclusive = !open;
            bounded = true;
        }
        if let Some((floor, _)) = &self.floor_record {
            // Unbounded scans always narrow to the floor; otherwise the
            // smaller key wins.
            if !bounded || floor.1 < key.1 {
                key = floor.clone();
                inclusive = true;
            }
        }
        if !bounded && self.floor_record.is_none() {
            return Bound::Unbounded;
        }
        if inclusive {
            Bound::Included(key)
        } else {
            Bound::Excluded(key)
        }
    }

    /// Pending start bound mirrors the committed one (same key space).
    fn pending_start_record(&self, store: StoreId) -> Bound<RecordKey> {
        if let Some(pos) = &self.pending_record_pos {
            return Bound::Excluded(pos.clone());
        }
        // Same clamp as committed: floor narrowed with the range lower.
        let mut key = (store, Vec::new());
        let mut inclusive = true;
        if let Some((lo, open)) = &self.range.lower {
            key = (store, lo.clone());
            inclusive = !open;
        }
        if let Some((floor, _)) = &self.floor_record {
            if floor.1 > key.1 {
                key = floor.clone();
                inclusive = true;
            }
        }
        if inclusive {
            Bound::Included(key)
        } else {
            Bound::Excluded(key)
        }
    }

    /// Pending upper bound mirrors the committed one.
    fn pending_end_record(&self, store: StoreId) -> Bound<RecordKey> {
        if let Some(pos) = &self.pending_record_pos {
            return Bound::Excluded(pos.clone());
        }
        let mut key = (store, Vec::new());
        let mut inclusive = false;
        let mut bounded = false;
        if let Some((hi, open)) = &self.range.upper {
            key = (store, hi.clone());
            inclusive = !open;
            bounded = true;
        }
        if let Some((floor, _)) = &self.floor_record {
            if !bounded || floor.1 < key.1 {
                key = floor.clone();
                inclusive = true;
            }
        }
        if !bounded && self.floor_record.is_none() {
            return Bound::Unbounded;
        }
        if inclusive {
            Bound::Included(key)
        } else {
            Bound::Excluded(key)
        }
    }

    /// Next committed store key at or after the resume point (`None` past
    /// the end). Pure peek: positions advance only on merge decisions, never
    /// here — peeking ahead must not consume rows the merge awards to the
    /// other source. Short read lock, never held across steps.
    fn next_committed_record(&self, store: StoreId) -> Option<RecordKey> {
        let state = self.committed.read();
        let found = if self.desc {
            let end = self.start_record_upper(store);
            state
                .records
                .range((Bound::Unbounded, end))
                .next_back()
                .map(|(k, _)| k.clone())
        } else {
            let start = self.start_record_lower(store);
            state
                .records
                .range((start, Bound::Unbounded))
                .next()
                .map(|(k, _)| k.clone())
        };
        drop(state);
        if let Some(ref key) = found {
            let (sid, _) = key;
            if *sid != store {
                return None;
            }
        }
        found
    }

    /// Next pending store slot at or after the resume point, tombstones
    /// included. Pure peek: the merge advances positions explicitly, so a
    /// peeked row is never consumed without a decision.
    fn peek_pending_record(&self, store: StoreId) -> Option<(RecordKey, Option<Vec<u8>>)> {
        let start = if self.desc {
            self.pending_end_record(store)
        } else {
            self.pending_start_record(store)
        };
        let found = if self.desc {
            self.pending_records
                .range((Bound::Unbounded, start))
                .next_back()
                .map(|(k, v)| (k.clone(), v.clone()))
        } else {
            self.pending_records
                .range((start, Bound::Unbounded))
                .next()
                .map(|(k, v)| (k.clone(), v.clone()))
        };
        let (key, slot) = found?;
        let (sid, _) = &key;
        if *sid != store {
            return None;
        }
        Some((key, slot))
    }

    /// Reads the merged value for a record key (pending overlay first).
    fn read_merged(&self, key: &RecordKey) -> Option<Vec<u8>> {
        if let Some(slot) = self.pending_records.get(key) {
            return slot.clone();
        }
        self.committed.read().records.get(key).cloned()
    }

    /// Advances a store cursor. Returns false when exhausted.
    ///
    /// Every loop pass advances at least one source position past the
    /// examined key (yielded, skipped or out-of-range), so the walk always
    /// terminates on finite maps. Peeks are pure; all consumption flows
    /// through the positions below.
    fn advance_store(&mut self, store: StoreId) -> Result<bool, BackendError> {
        // Merge source of the picked row (for position advancement).
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Source {
            Committed,
            Pending,
        }
        loop {
            let committed = self.next_committed_record(store);
            let pending = self.peek_pending_record(store);
            // (key, value-if-pending-live, source, tombstoned?)
            let pick: Option<(RecordKey, Option<Vec<u8>>, Source, bool)> =
                match (committed, pending) {
                    (None, None) => None,
                    (Some(ckey), None) => {
                        let tombstoned =
                            self.pending_records.get(&ckey).is_some_and(|s| s.is_none());
                        Some((ckey, None, Source::Committed, tombstoned))
                    }
                    (None, Some((pkey, slot))) => {
                        let tombstoned = slot.is_none();
                        Some((pkey, slot, Source::Pending, tombstoned))
                    }
                    (Some(ckey), Some((pkey, slot))) => {
                        use std::cmp::Ordering;
                        match pkey.1.cmp(&ckey.1) {
                            Ordering::Less if !self.desc => {
                                let tombstoned = slot.is_none();
                                Some((pkey, slot, Source::Pending, tombstoned))
                            }
                            Ordering::Greater if self.desc => {
                                let tombstoned = slot.is_none();
                                Some((pkey, slot, Source::Pending, tombstoned))
                            }
                            Ordering::Equal => {
                                // Same key: pending shadows committed. Both
                                // sources move past it exactly once; a
                                // tombstone shadow skips the key entirely.
                                self.committed_record_pos = Some(ckey);
                                self.pending_record_pos = Some(pkey.clone());
                                if slot.is_none() {
                                    continue;
                                }
                                Some((pkey, slot, Source::Pending, false))
                            }
                            _ => {
                                let tombstoned =
                                    self.pending_records.get(&ckey).is_some_and(|s| s.is_none());
                                Some((ckey, None, Source::Committed, tombstoned))
                            }
                        }
                    }
                };
            let Some((key, value, source, tombstoned)) = pick else {
                self.exhausted = true;
                self.current = None;
                return Ok(false);
            };
            // Advance past the examined key on its source (both sources on
            // ties, handled above).
            match source {
                Source::Committed => {
                    self.committed_record_pos = Some(key.clone());
                }
                Source::Pending => {
                    self.pending_record_pos = Some(key.clone());
                }
            }
            if tombstoned {
                continue;
            }
            if !self.range.contains(&key.1) {
                // Below-lower (forward, only right after a below-range seek)
                // is skipped (position already advanced past it); above-upper
                // or cross-store ends the walk: ordering guarantees nothing
                // later matches.
                if self.past_upper(store, &key.1) {
                    self.exhausted = true;
                    self.current = None;
                    return Ok(false);
                }
                continue;
            }
            let value = if self.key_only {
                None
            } else if value.is_some() {
                value
            } else {
                self.read_merged(&key)
            };
            self.current = Some((key.1.clone(), key.1, value));
            return Ok(true);
        }
    }

    /// Whether `key` is past the range/store end for the direction.
    fn past_upper(&self, _store: StoreId, key: &[u8]) -> bool {
        if self.desc {
            match &self.range.lower {
                Some((lo, open)) => {
                    use std::cmp::Ordering;
                    match key.cmp(lo) {
                        Ordering::Less => true,
                        Ordering::Equal => *open,
                        Ordering::Greater => false,
                    }
                }
                None => false,
            }
        } else {
            match &self.range.upper {
                Some((hi, open)) => {
                    use std::cmp::Ordering;
                    match key.cmp(hi) {
                        Ordering::Greater => true,
                        Ordering::Equal => *open,
                        Ordering::Less => false,
                    }
                }
                None => false,
            }
        }
    }

    /// Lower index bound for forward scans (group and pkey aware).
    fn start_index_lower(&self, index: IndexId) -> Bound<IndexKey> {
        if let Some(pos) = &self.committed_index_pos {
            return Bound::Excluded(pos.clone());
        }
        let mut key = (index, Vec::new(), Vec::new());
        let mut inclusive = true;
        if let Some((lo, open)) = &self.range.lower {
            key = (index, lo.clone(), Vec::new());
            inclusive = !open;
        }
        if let Some((floor, _)) = &self.floor_index {
            if (floor.1.as_slice(), floor.2.as_slice()) > (key.1.as_slice(), key.2.as_slice()) {
                key = floor.clone();
                inclusive = true;
            }
        }
        if inclusive {
            Bound::Included(key)
        } else {
            Bound::Excluded(key)
        }
    }

    /// Upper index bound for backward scans (mirror).
    fn start_index_upper(&self, index: IndexId) -> Bound<IndexKey> {
        if let Some(pos) = &self.committed_index_pos {
            return Bound::Excluded(pos.clone());
        }
        // No maximum byte vector exists: leave unbounded and let the
        // per-candidate checks terminate the walk. The floor (seek target)
        // still narrows the start below.
        let mut bound: Bound<IndexKey> = Bound::Unbounded;
        let mut bounded = false;
        if let Some((hi, open)) = &self.range.upper {
            bound = if *open {
                Bound::Excluded((index, hi.clone(), Vec::new()))
            } else {
                // Inclusive upper must cover the whole group: fall back to
                // the unbounded scan with per-candidate checks below.
                Bound::Unbounded
            };
            bounded = !open;
            if *open {
                bounded = true;
            }
        }
        if let Some((floor, _)) = &self.floor_index {
            bound = Bound::Included(floor.clone());
            bounded = true;
        }
        if bounded { bound } else { Bound::Unbounded }
    }

    /// Pending index bounds mirror the committed ones.
    fn pending_start_index(&self, index: IndexId) -> Bound<IndexKey> {
        if let Some(pos) = &self.pending_index_pos {
            return Bound::Excluded(pos.clone());
        }
        let mut key = (index, Vec::new(), Vec::new());
        let mut inclusive = true;
        if let Some((lo, open)) = &self.range.lower {
            key = (index, lo.clone(), Vec::new());
            inclusive = !open;
        }
        if let Some((floor, _)) = &self.floor_index {
            if (floor.1.as_slice(), floor.2.as_slice()) > (key.1.as_slice(), key.2.as_slice()) {
                key = floor.clone();
                inclusive = true;
            }
        }
        if inclusive {
            Bound::Included(key)
        } else {
            Bound::Excluded(key)
        }
    }

    /// Pending upper bound mirrors the committed one.
    fn pending_end_index(&self, index: IndexId) -> Bound<IndexKey> {
        if let Some(pos) = &self.pending_index_pos {
            return Bound::Excluded(pos.clone());
        }
        let mut bound: Bound<IndexKey> = Bound::Unbounded;
        let mut bounded = false;
        if let Some((hi, open)) = &self.range.upper {
            if *open {
                bound = Bound::Excluded((index, hi.clone(), Vec::new()));
                bounded = true;
            }
        }
        if let Some((floor, _)) = &self.floor_index {
            bound = Bound::Included(floor.clone());
            bounded = true;
        }
        if bounded { bound } else { Bound::Unbounded }
    }

    /// Next committed index key at or after the resume point. Pure peek:
    /// the merge advances positions explicitly.
    fn peek_committed_index(&self, index: IndexId) -> Option<IndexKey> {
        let state = self.committed.read();
        let found = if self.desc {
            let end = self.start_index_upper(index);
            state
                .index_entries
                .range((Bound::Unbounded, end))
                .next_back()
                .map(|(k, _)| k.clone())
        } else {
            let start = self.start_index_lower(index);
            state
                .index_entries
                .range((start, Bound::Unbounded))
                .next()
                .map(|(k, _)| k.clone())
        };
        drop(state);
        if let Some(ref key) = found {
            let (iid, _, _) = key;
            if *iid != index {
                return None;
            }
        }
        found
    }

    /// Next pending index slot at or after the resume point, tombstones
    /// included. Pure peek: the merge advances positions explicitly.
    fn peek_pending_index(&self, index: IndexId) -> Option<(IndexKey, bool)> {
        let start = if self.desc {
            self.pending_end_index(index)
        } else {
            self.pending_start_index(index)
        };
        let found = if self.desc {
            self.pending_index
                .range((Bound::Unbounded, start))
                .next_back()
                .map(|(k, v)| (k.clone(), *v))
        } else {
            self.pending_index
                .range((start, Bound::Unbounded))
                .next()
                .map(|(k, v)| (k.clone(), *v))
        };
        let (key, live) = found?;
        let (iid, _, _) = &key;
        if *iid != index {
            return None;
        }
        Some((key, live))
    }

    /// Advances an index cursor. Returns false when exhausted.
    ///
    /// Same advancement discipline as [`Self::advance_store`]: every pass
    /// moves at least one source past the examined key, so the walk always
    /// terminates on finite maps.
    fn advance_index(
        &mut self,
        store: StoreId,
        index: IndexId,
        unique: bool,
    ) -> Result<bool, BackendError> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Source {
            Committed,
            Pending,
        }
        loop {
            let committed = self.peek_committed_index(index);
            let pending = self.peek_pending_index(index);
            // (index key, primary key, source, tombstoned?)
            let pick: Option<(Vec<u8>, Vec<u8>, Source, bool)> = match (committed, pending) {
                (None, None) => None,
                (Some(ckey), None) => {
                    let tombstoned = self.pending_index.get(&ckey).is_some_and(|live| !live);
                    Some((ckey.1, ckey.2, Source::Committed, tombstoned))
                }
                (None, Some((pkey, live))) => Some((pkey.1, pkey.2, Source::Pending, !live)),
                (Some(ckey), Some((pkey, live))) => {
                    use std::cmp::Ordering;
                    match (pkey.1.as_slice(), pkey.2.as_slice())
                        .cmp(&(ckey.1.as_slice(), ckey.2.as_slice()))
                    {
                        Ordering::Less if !self.desc => {
                            Some((pkey.1, pkey.2, Source::Pending, !live))
                        }
                        Ordering::Greater if self.desc => {
                            Some((pkey.1, pkey.2, Source::Pending, !live))
                        }
                        Ordering::Equal => {
                            // Same entry: pending shadows committed. Both
                            // sources move past it exactly once.
                            self.committed_index_pos = Some(ckey);
                            self.pending_index_pos = Some(pkey.clone());
                            if !live {
                                continue;
                            }
                            Some((pkey.1, pkey.2, Source::Pending, false))
                        }
                        _ => {
                            let tombstoned =
                                self.pending_index.get(&ckey).is_some_and(|live| !live);
                            Some((ckey.1, ckey.2, Source::Committed, tombstoned))
                        }
                    }
                }
            };
            let Some((idx_key, pkey, source, tombstoned)) = pick else {
                self.exhausted = true;
                self.current = None;
                return Ok(false);
            };
            // Advance past the examined entry on its source (both sources
            // on ties, handled above).
            match source {
                Source::Committed => {
                    self.committed_index_pos = Some((index, idx_key.clone(), pkey.clone()));
                }
                Source::Pending => {
                    self.pending_index_pos = Some((index, idx_key.clone(), pkey.clone()));
                }
            }
            if tombstoned {
                continue;
            }
            if !self.range.contains(&idx_key) {
                // Below-lower (forward, seek-only) skips with positions
                // already advanced; upper violations end the walk.
                if self.past_upper_index(&idx_key) {
                    self.exhausted = true;
                    self.current = None;
                    return Ok(false);
                }
                continue;
            }
            // Unique cursors yield one entry per index key. Ascending, the
            // first encounter is the smallest primary key; descending, the
            // representative is resolved by forward sub-seek below.
            if unique {
                if self
                    .last_index_key
                    .as_ref()
                    .is_some_and(|last| *last == idx_key)
                {
                    continue;
                }
                if self.desc {
                    let (min_pkey, value) = self.group_min(store, index, &idx_key)?;
                    self.last_index_key = Some(idx_key.clone());
                    // Jump both descents below the whole group: larger
                    // members were never examined and must not resurface.
                    self.committed_index_pos = Some((index, idx_key.clone(), min_pkey.clone()));
                    self.pending_index_pos = Some((index, idx_key.clone(), min_pkey.clone()));
                    let value = if self.key_only { None } else { value };
                    self.current = Some((idx_key, min_pkey, value));
                    return Ok(true);
                }
                self.last_index_key = Some(idx_key.clone());
            }
            let value = if self.key_only {
                None
            } else {
                self.read_merged(&(store, pkey.clone()))
            };
            self.current = Some((idx_key, pkey, value));
            return Ok(true);
        }
    }

    /// Whether an index key is past the range end for the direction.
    fn past_upper_index(&self, key: &[u8]) -> bool {
        if self.desc {
            match &self.range.lower {
                Some((lo, open)) => {
                    use std::cmp::Ordering;
                    match key.cmp(lo) {
                        Ordering::Less => true,
                        Ordering::Equal => *open,
                        Ordering::Greater => false,
                    }
                }
                None => false,
            }
        } else {
            match &self.range.upper {
                Some((hi, open)) => {
                    use std::cmp::Ordering;
                    match key.cmp(hi) {
                        Ordering::Greater => true,
                        Ordering::Equal => *open,
                        Ordering::Less => false,
                    }
                }
                None => false,
            }
        }
    }

    /// Smallest primary key (and value) of an index-key group, resolved by
    /// forward sub-seek across both sources. O(group) time, O(1) memory.
    fn group_min(
        &self,
        store: StoreId,
        index: IndexId,
        idx_key: &[u8],
    ) -> Result<(Vec<u8>, Option<Vec<u8>>), BackendError> {
        let bottom: Vec<u8> = Vec::new();
        // Committed member: smallest pkey at/after (idx, ⊥), tombstone-free.
        let mut committed_min: Option<(Vec<u8>, Option<Vec<u8>>)> = None;
        {
            let state = self.committed.read();
            let start: Bound<IndexKey> = Bound::Included((index, idx_key.to_vec(), bottom.clone()));
            for (key, _) in state.index_entries.range((start, Bound::Unbounded)) {
                let (iid, ik, pk) = key;
                if *iid != index || *ik != idx_key {
                    break;
                }
                if self.pending_index.get(key).is_some_and(|live| !live) {
                    continue;
                }
                let value = if self.key_only {
                    None
                } else {
                    self.read_merged(&(store, pk.clone()))
                };
                committed_min = Some((pk.clone(), value));
                break;
            }
        }
        // Pending member: smallest live pkey in the group.
        let mut pending_min: Option<(Vec<u8>, Option<Vec<u8>>)> = None;
        {
            let start: Bound<IndexKey> = Bound::Included((index, idx_key.to_vec(), bottom));
            for (key, live) in self.pending_index.range((start, Bound::Unbounded)) {
                let (iid, ik, pk) = key;
                if *iid != index || *ik != idx_key {
                    break;
                }
                if *live {
                    let value = if self.key_only {
                        None
                    } else {
                        self.read_merged(&(store, pk.clone()))
                    };
                    pending_min = Some((pk.clone(), value));
                    break;
                }
            }
        }
        match (committed_min, pending_min) {
            (Some((cpk, cval)), Some((ppk, pval))) => {
                if ppk < cpk {
                    Ok((ppk, pval))
                } else {
                    Ok((cpk, cval))
                }
            }
            (Some(min), None) | (None, Some(min)) => Ok(min),
            (None, None) => Err(BackendError::Internal(
                "unique group vanished mid-iteration".into(),
            )),
        }
    }

    /// Advances to the next matching row, buffering it. Returns false when
    /// exhausted. One buffered row: O(1) memory in the range size.
    fn advance(&mut self) -> Result<bool, BackendError> {
        match self.kind {
            CursorKind::Store { store } => self.advance_store(store),
            CursorKind::Index {
                store,
                index,
                unique,
            } => self.advance_index(store, index, unique),
        }
    }
}

impl<'a> BackendCursor for MemoryCursor<'a> {
    fn seek(&mut self, target: CursorSeek) -> Result<bool, BackendError> {
        // Reset merge state; floors select the first page, clamped with the
        // range by the bound helpers.
        self.committed_record_pos = None;
        self.pending_record_pos = None;
        self.committed_index_pos = None;
        self.pending_index_pos = None;
        self.floor_record = None;
        self.floor_index = None;
        self.last_index_key = None;
        self.exhausted = false;
        match target {
            CursorSeek::First => {}
            CursorSeek::Key(key) => match self.kind {
                CursorKind::Store { store } => {
                    self.floor_record = Some(((store, key), true));
                }
                CursorKind::Index { index, .. } => {
                    self.floor_index = Some(((index, key, Vec::new()), true));
                }
            },
            CursorSeek::KeyAndPrimaryKey { key, pkey } => match self.kind {
                CursorKind::Store { store } => {
                    // Store keys are primary keys.
                    self.floor_record = Some(((store, key), true));
                }
                CursorKind::Index { index, .. } => {
                    self.floor_index = Some(((index, key, pkey), true));
                }
            },
        }
        self.advance()
    }

    fn step(&mut self, count: u32) -> Result<bool, BackendError> {
        if self.exhausted {
            return Ok(false);
        }
        for _ in 0..count {
            if !self.advance()? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn current_key(&self) -> &[u8] {
        if self.exhausted {
            return &[];
        }
        self.current.as_ref().map_or(&[], |(k, _, _)| k.as_slice())
    }

    fn current_primary_key(&self) -> &[u8] {
        if self.exhausted || self.current.is_none() {
            return &[];
        }
        self.current
            .as_ref()
            .map_or(&[], |(_, pk, _)| pk.as_slice())
    }

    fn current_value(&self) -> Option<&[u8]> {
        if self.key_only || self.exhausted {
            return None;
        }
        self.current.as_ref().and_then(|(_, _, v)| v.as_deref())
    }
}
