//! Shared API helpers: request issuance, query conversion, transaction guards.

use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_idb_core::key::value::Key;
use boa_idb_core::proto::Direction;

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

/// Converts a query argument (`key | KeyRange | undefined`) to driver range data.
pub fn query_to_range(query: &JsValue, context: &mut Context) -> JsResult<RangeData> {
    if query.is_undefined() {
        return Ok(RangeData::All);
    }
    if let Some(obj) = query.as_object() {
        if let Some(kr) = obj.downcast_ref::<IdBKeyRange>() {
            return Ok(range_data_of(&kr));
        }
    }
    match value_to_key(query, context) {
        Ok(key) => Ok(RangeData::Only(key)),
        Err(e) => crate::dom::exception::throw_data_error(&format!("Invalid query: {e}"), context)
            .map(|_| RangeData::All),
    }
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
        if obj.downcast_ref::<IdBKeyRange>().is_none() {
            let has_query = obj
                .has_property(js_string!("query"), context)
                .unwrap_or(false);
            let has_count = obj
                .has_property(js_string!("count"), context)
                .unwrap_or(false);
            let has_direction = obj
                .has_property(js_string!("direction"), context)
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

/// Converts a driver key to a JS value, mapping failures to `DataError`.
pub fn key_to_js(key: &Key, context: &mut Context) -> JsResult<JsValue> {
    crate::convert::key::key_to_value(key, context)
}
