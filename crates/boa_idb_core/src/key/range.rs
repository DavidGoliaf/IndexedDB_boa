//! Key range types for IndexedDB queries.

/// An encoded key range used for backend queries.
///
/// Each bound is an optional tuple of (encoded key bytes, is_open).
/// `None` bound means unbounded in that direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedRange {
    /// Lower bound: `None` = unbounded, `Some((bytes, is_open))` = bounded.
    pub lower: Option<(Vec<u8>, bool)>,
    /// Upper bound: `None` = unbounded, `Some((bytes, is_open))` = bounded.
    pub upper: Option<(Vec<u8>, bool)>,
}

impl EncodedRange {
    /// Creates a range that matches all keys.
    pub fn all() -> Self {
        Self {
            lower: None,
            upper: None,
        }
    }

    /// Creates a range that matches a single key (closed lower and upper at the same value).
    pub fn only(key: Vec<u8>) -> Self {
        Self {
            lower: Some((key.clone(), false)),
            upper: Some((key, false)),
        }
    }

    /// Creates a range with only a lower bound.
    pub fn lower_bound(key: Vec<u8>, open: bool) -> Self {
        Self {
            lower: Some((key, open)),
            upper: None,
        }
    }

    /// Creates a range with only an upper bound.
    pub fn upper_bound(key: Vec<u8>, open: bool) -> Self {
        Self {
            lower: None,
            upper: Some((key, open)),
        }
    }

    /// Creates a range with both lower and upper bounds.
    pub fn bound(lower: Vec<u8>, lower_open: bool, upper: Vec<u8>, upper_open: bool) -> Self {
        Self {
            lower: Some((lower, lower_open)),
            upper: Some((upper, upper_open)),
        }
    }
}
