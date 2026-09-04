//! Shared API helpers: request issuance, query conversion, transaction guards.

use boa_engine::object::builtins::{JsArray, JsArrayBuffer, JsDate, JsTypedArray};
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_idb_core::key::value::Key;
use boa_idb_core::proto::Direction;
use boa_idb_core::proto::SourceRef;

use crate::api::key_range::IdBKeyRange;
use crate::api::request::IdBRequest;
use crate::api::transaction::IdBTransaction;
use crate::convert::key::value_to_key;
use crate::driver::{PendingOp, RangeData};

/// Issues a request: allocates an id, creates the shell, enqueues the op,
/// and schedules the pump. Execution happens asynchronously in the pump job.
pub fn issue_request(
    context: &mut Context,
    txn_id: u64,
    op: PendingOp,
    source: Option<JsObject>,
) -> JsResult<JsObject> {
    let request_id = crate::driver::alloc_request_id(context);
    let txn_obj = crate::runtime::txn_object(context, txn_id);
    let req_obj = IdBRequest::create_object(request_id, source, txn_obj, context)?;
    crate::driver::enqueue_op(context, txn_id, request_id, op);
    crate::runtime::schedule_pump(context);
    Ok(req_obj)
}

/// Reads `(txn_id, active, finished, explicit_commit)` from a transaction object.
fn txn_flags(obj: &JsObject) -> Option<(u64, bool, bool, bool)> {
    obj.downcast_ref::<IdBTransaction>()
        .map(|d| (d.txn_id, d.active, d.finished, d.explicit_commit))
}

/// Requires the transaction of a store/index/cursor handle to accept new requests.
pub fn active_txn_id(context: &mut Context, txn_obj: &JsObject) -> JsResult<u64> {
    match txn_flags(txn_obj) {
        Some((id, true, false, false)) => Ok(id),
        Some(_) => crate::dom::exception::throw_transaction_inactive_error(
            "The transaction is inactive or finished.",
            context,
        )
        .map(|_| 0),
        None => Err(JsNativeError::typ()
            .with_message("Transaction object expected")
            .into()),
    }
}

/// Requires a store handle to still name a live store (`InvalidStateError`).
///
/// Check order matters (mirrors the spec steps of every store/index method):
/// a handle whose upgrade transaction aborted or committed is dead even
/// though the JS transaction object still exists — the schema it pointed at
/// is gone. Only when the handle is live does the transaction-activity
/// check (`active_txn_id`, `TransactionInactiveError`) run.
pub fn require_live_store(
    context: &mut Context,
    txn_obj: &JsObject,
    store_id: u64,
) -> JsResult<()> {
    let (finished, is_upgrade) =
        txn_obj
            .downcast_ref::<IdBTransaction>()
            .map_or((false, false), |d| {
                (
                    d.finished,
                    d.mode == boa_idb_core::proto::TxnMode::VersionChange,
                )
            });
    if finished && is_upgrade {
        // Aborted/committed upgrade: the schema this handle pointed at is gone.
        return crate::dom::exception::throw_invalid_state_error(
            "The object store no longer exists.",
            context,
        )
        .map(|_| ());
    }
    if !finished {
        let live = context
            .get_data::<crate::runtime::IdbRuntime>()
            .is_some_and(|r| {
                let txn_id = txn_obj
                    .downcast_ref::<IdBTransaction>()
                    .map_or(u64::MAX, |d| d.txn_id);
                let d = crate::runtime::lock_mutex(&r.driver);
                d.txns
                    .get(&txn_id)
                    .is_some_and(|h| h.meta.stores.iter().any(|s| s.id == store_id && !s.deleted))
            });
        if !live {
            return crate::dom::exception::throw_invalid_state_error(
                "The object store has been deleted.",
                context,
            )
            .map(|_| ());
        }
    }
    Ok(())
}

/// Requires an index handle to still name a live index (`InvalidStateError`).
///
/// Same ordering as [`require_live_store`]: dead upgrades first, then the
/// live-schema check, leaving activity errors to `active_txn_id`.
pub fn require_live_index(
    context: &mut Context,
    txn_obj: &JsObject,
    store_id: u64,
    index_id: u64,
) -> JsResult<()> {
    let (finished, is_upgrade) =
        txn_obj
            .downcast_ref::<IdBTransaction>()
            .map_or((false, false), |d| {
                (
                    d.finished,
                    d.mode == boa_idb_core::proto::TxnMode::VersionChange,
                )
            });
    if finished && is_upgrade {
        return crate::dom::exception::throw_invalid_state_error(
            "The index no longer exists.",
            context,
        )
        .map(|_| ());
    }
    if !finished {
        let live = context
            .get_data::<crate::runtime::IdbRuntime>()
            .is_some_and(|r| {
                let txn_id = txn_obj
                    .downcast_ref::<IdBTransaction>()
                    .map_or(u64::MAX, |d| d.txn_id);
                let d = crate::runtime::lock_mutex(&r.driver);
                d.txns.get(&txn_id).is_some_and(|h| {
                    h.meta.stores.iter().any(|s| {
                        s.id == store_id
                            && !s.deleted
                            && s.indexes.iter().any(|i| i.id == index_id && !i.deleted)
                    })
                })
            });
        if !live {
            return crate::dom::exception::throw_invalid_state_error(
                "The index has been deleted.",
                context,
            )
            .map(|_| ());
        }
    }
    Ok(())
}

/// Requires a cursor's source schema to remain live during an upgrade.
pub fn require_live_cursor(context: &mut Context, cursor_id: u64, txn_id: u64) -> JsResult<()> {
    let source_live = context
        .get_data::<crate::runtime::IdbRuntime>()
        .and_then(|r| {
            let d = crate::runtime::lock_mutex(&r.driver);
            let cursor = d.cursors.get(&cursor_id)?;
            let Some(txn) = d.txns.get(&txn_id) else {
                return Some(true);
            };
            let Some(store) = txn.meta.stores.iter().find(|store| match cursor.source {
                SourceRef::Store(store_id) => store.id == store_id,
                SourceRef::Index {
                    store: store_id, ..
                } => store.id == store_id,
            }) else {
                return Some(false);
            };
            let source_live = if store.deleted {
                false
            } else {
                match cursor.source {
                    SourceRef::Store(_) => true,
                    SourceRef::Index { index, .. } => store
                        .indexes
                        .iter()
                        .any(|candidate| candidate.id == index && !candidate.deleted),
                }
            };
            Some(source_live)
        });
    if source_live.is_none() {
        return crate::dom::exception::throw_invalid_state_error(
            "The cursor source has been deleted.",
            context,
        )
        .map(|_| ());
    }
    if source_live == Some(false) {
        return crate::dom::exception::throw_invalid_state_error(
            "The cursor source has been deleted.",
            context,
        )
        .map(|_| ());
    }
    let txn_present = context
        .get_data::<crate::runtime::IdbRuntime>()
        .is_some_and(|r| {
            crate::runtime::lock_mutex(&r.driver)
                .txns
                .contains_key(&txn_id)
        });
    if !txn_present {
        return crate::dom::exception::throw_transaction_inactive_error(
            "The cursor transaction is inactive.",
            context,
        )
        .map(|_| ());
    }
    Ok(())
}

/// Converts a query argument (`key | KeyRange | undefined`) to driver range data.
pub fn query_to_range(query: &JsValue, context: &mut Context) -> JsResult<RangeData> {
    if query.is_undefined() {
        return Ok(RangeData::All);
    }
    if let Some(obj) = query.as_object() {
        let is_key_object = obj.is_array()
            || JsDate::from_object(obj.clone()).is_ok()
            || JsTypedArray::from_object(obj.clone()).is_ok()
            || JsArrayBuffer::from_object(obj.clone()).is_ok();
        if !is_key_object && is_key_range_object(context, &obj) {
            let kr = obj
                .downcast_ref::<IdBKeyRange>()
                .ok_or_else(|| JsNativeError::typ().with_message("Invalid IDBKeyRange object"))?;
            return Ok(range_data_of(&kr));
        }
    }
    match value_to_key(query, context) {
        Ok(key) => Ok(RangeData::Only(key)),
        Err(e) => Err(crate::convert::key::throw_key_conversion_error(e, context)),
    }
}

/// Converts a nullable cursor query. WebIDL maps `null` to the omitted query
/// for cursor-opening methods, while get/delete/count treat `null` as an
/// invalid key through [`query_to_range`].
pub fn nullable_query_to_range(query: &JsValue, context: &mut Context) -> JsResult<RangeData> {
    if query.is_null() {
        return Ok(RangeData::All);
    }
    query_to_range(query, context)
}

/// Converts native key-range data to driver range data.
pub fn range_data_of(kr: &IdBKeyRange) -> RangeData {
    match (&kr.lower, &kr.upper) {
        (None, None) => RangeData::All,
        _ => RangeData::Bounds {
            lower: kr.lower.clone().map(|k| (k, kr.lower_open)),
            upper: kr.upper.clone().map(|k| (k, kr.upper_open)),
        },
    }
}

/// Parses a cursor direction (`undefined` → `next`).
pub fn parse_direction(val: &JsValue, context: &mut Context) -> JsResult<Direction> {
    if val.is_undefined() {
        return Ok(Direction::Next);
    }
    let s = val.to_string(context)?.to_std_string_escaped();
    match s.as_str() {
        "next" => Ok(Direction::Next),
        "nextunique" => Ok(Direction::NextUnique),
        "prev" => Ok(Direction::Prev),
        "prevunique" => Ok(Direction::PrevUnique),
        _ => Err(JsNativeError::typ()
            .with_message(format!("Invalid cursor direction '{s}'"))
            .into()),
    }
}

/// Parses a `count` argument (`undefined`/0 → unlimited).
pub fn parse_count(val: Option<&JsValue>, context: &mut Context) -> JsResult<Option<u32>> {
    let Some(v) = val else { return Ok(None) };
    if v.is_undefined() {
        return Ok(None);
    }
    let count = crate::convert::webidl::to_unsigned_long_enforce_range(v, context)?;
    Ok(if count == 0 { None } else { Some(count) })
}

/// A parsed `getAll`-family call: `(query, count, direction)`.
pub struct GetAllArgs {
    /// Range data.
    pub range: RangeData,
    /// Row limit (`None` = unlimited).
    pub limit: Option<u32>,
    /// Cursor direction.
    pub direction: Direction,
}

/// Parses `(query, count)` or a single `IDBGetAllOptions` argument.
///
/// An options object is recognized by the presence of `query`, `count` or
/// `direction` own properties (a `Key` never has those).
pub fn parse_get_all_args(args: &[JsValue], context: &mut Context) -> JsResult<GetAllArgs> {
    let undefined = JsValue::undefined();
    let first = args.first().unwrap_or(&undefined);
    // Options-object shape?
    if let Some(obj) = first.as_object() {
        // Key-shaped objects (arrays, dates and buffers) are query keys, not
        // IDBGetAllOptions dictionaries. This distinction matters because an
        // array or a detached buffer has no dictionary members, but must still
        // synchronously fail key conversion with DataError.
        let is_key_object = obj.is_array()
            || JsDate::from_object(obj.clone()).is_ok()
            || JsTypedArray::from_object(obj.clone()).is_ok()
            || JsArrayBuffer::from_object(obj.clone()).is_ok();
        let is_key_range = is_key_range_object(context, &obj);
        if !is_key_range && !is_key_object {
            let has_query = obj
                .has_own_property(js_string!("query"), context)
                .unwrap_or(false);
            let has_count = obj
                .has_own_property(js_string!("count"), context)
                .unwrap_or(false);
            let has_direction = obj
                .has_own_property(js_string!("direction"), context)
                .unwrap_or(false);
            if has_query || has_count || has_direction {
                let query = obj
                    .get(js_string!("query"), context)
                    .unwrap_or(JsValue::undefined());
                let range = if has_query {
                    query_to_range(&query, context)?
                } else {
                    RangeData::All
                };
                let limit = if has_count {
                    parse_count(obj.get(js_string!("count"), context).ok().as_ref(), context)?
                } else {
                    None
                };
                let direction = if has_direction {
                    parse_direction(
                        &obj.get(js_string!("direction"), context)
                            .unwrap_or(JsValue::undefined()),
                        context,
                    )?
                } else {
                    Direction::Next
                };
                return Ok(GetAllArgs {
                    range,
                    limit,
                    direction,
                });
            }
        }
    }
    Ok(GetAllArgs {
        range: query_to_range(first, context)?,
        limit: parse_count(args.get(1), context)?,
        direction: Direction::Next,
    })
}

fn is_key_range_object(context: &mut Context, object: &JsObject) -> bool {
    let set = context
        .get_data::<crate::runtime::IdbRuntime>()
        .and_then(|runtime| runtime.key_range_set.borrow().clone());
    if let Some(set) = set
        && let Ok(has) = set.get(js_string!("has"), context)
        && let Some(has) = has.as_callable()
        && let Ok(result) = has.call(
            &JsValue::from(set),
            &[JsValue::from(object.clone())],
            context,
        )
    {
        return result.to_boolean();
    }
    false
}

/// Converts a driver key to a JS value, mapping failures to `DataError`.
pub fn key_to_js(key: &Key, context: &mut Context) -> JsResult<JsValue> {
    crate::convert::key::key_to_value(key, context)
}
