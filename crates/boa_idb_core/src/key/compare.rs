//! Key comparison per W3C IndexedDB §2.4.
//!
//! Type order: `Number < Date < String < Binary < Array`.
//! Within the same type, comparison is type-specific.

use crate::key::value::Key;
use std::cmp::Ordering;

fn type_order(key: &Key) -> u8 {
    match key {
        Key::Number(_) => 1,
        Key::Date(_) => 2,
        Key::String(_) => 3,
        Key::Binary(_) => 4,
        Key::Array(_) => 5,
    }
}

/// Compares two keys according to W3C IndexedDB §2.4.
///
/// Type order: `Number < Date < String < Binary < Array`.
/// Within the same type:
/// - Numbers/Dates: numeric comparison with `-0.0 == +0.0`.
/// - Strings: code unit comparison.
/// - Binary: unsigned byte comparison.
/// - Arrays: lexicographic element comparison, then by length.
pub fn compare_keys(a: &Key, b: &Key) -> Ordering {
    let order_a = type_order(a);
    let order_b = type_order(b);
    if order_a != order_b {
        return order_a.cmp(&order_b);
    }

    match (a, b) {
        (Key::Number(x), Key::Number(y)) | (Key::Date(x), Key::Date(y)) => {
            let x_norm = if *x == 0.0 { 0.0 } else { *x };
            let y_norm = if *y == 0.0 { 0.0 } else { *y };
            x_norm.partial_cmp(&y_norm).unwrap_or(Ordering::Equal)
        }
        (Key::String(x), Key::String(y)) => x.as_slice().cmp(y.as_slice()),
        (Key::Binary(x), Key::Binary(y)) => x.as_slice().cmp(y.as_slice()),
        (Key::Array(x), Key::Array(y)) => {
            let min_len = x.len().min(y.len());
            for i in 0..min_len {
                let ord = compare_keys(&x[i], &y[i]);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            x.len().cmp(&y.len())
        }
        // Cross-type pairs are excluded by the `type_order` check above, so
        // this arm is logically unreachable. It returns `Equal` instead of
        // panicking (`unreachable!` is forbidden in library code) as a
        // defensive fallback.
        _ => Ordering::Equal,
    }
}

impl Eq for Key {}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_keys(self, other)
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
