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

    /// Checks if a key falls within this range.
    pub fn contains(&self, key: &[u8]) -> bool {
        // Check lower bound
        if let Some((ref lower, open)) = self.lower {
            match key.cmp(lower.as_slice()) {
                std::cmp::Ordering::Less => return false,
                std::cmp::Ordering::Equal if open => return false,
                _ => {}
            }
        }

        // Check upper bound
        if let Some((ref upper, open)) = self.upper {
            match key.cmp(upper.as_slice()) {
                std::cmp::Ordering::Greater => return false,
                std::cmp::Ordering::Equal if open => return false,
                _ => {}
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_range_all() {
        let range = EncodedRange::all();
        assert!(range.contains(b"anything"));
        assert!(range.contains(b""));
    }

    #[test]
    fn test_range_only() {
        let range = EncodedRange::only(b"key1".to_vec());
        assert!(range.contains(b"key1"));
        assert!(!range.contains(b"key0"));
        assert!(!range.contains(b"key2"));
    }

    #[test]
    fn test_range_lower_bound_closed() {
        let range = EncodedRange::lower_bound(b"key1".to_vec(), false);
        assert!(range.contains(b"key1"));
        assert!(range.contains(b"key2"));
        assert!(!range.contains(b"key0"));
    }

    #[test]
    fn test_range_lower_bound_open() {
        let range = EncodedRange::lower_bound(b"key1".to_vec(), true);
        assert!(!range.contains(b"key1"));
        assert!(range.contains(b"key2"));
        assert!(!range.contains(b"key0"));
    }

    #[test]
    fn test_range_upper_bound_closed() {
        let range = EncodedRange::upper_bound(b"key1".to_vec(), false);
        assert!(range.contains(b"key1"));
        assert!(range.contains(b"key0"));
        assert!(!range.contains(b"key2"));
    }

    #[test]
    fn test_range_upper_bound_open() {
        let range = EncodedRange::upper_bound(b"key1".to_vec(), true);
        assert!(!range.contains(b"key1"));
        assert!(range.contains(b"key0"));
        assert!(!range.contains(b"key2"));
    }

    #[test]
    fn test_range_bound() {
        let range = EncodedRange::bound(b"a".to_vec(), false, b"z".to_vec(), false);
        assert!(range.contains(b"a"));
        assert!(range.contains(b"m"));
        assert!(range.contains(b"z"));
        assert!(!range.contains(b"A"));
        assert!(!range.contains(b"zz"));
    }
}
