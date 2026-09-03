//! IndexedDB key type per §2.4 of the specification.

use crate::error::KeyError;
use crate::key::utf16::Utf16String;
use crate::limits::LimitConfig;

/// An IndexedDB key according to §2.4 of the specification.
///
/// Key types are ordered: `Number < Date < String < Binary < Array`.
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    /// A numeric key (f64, not NaN).
    Number(f64),
    /// A date key (milliseconds since epoch, not NaN).
    Date(f64),
    /// A string key (arbitrary UTF-16 code units).
    String(Utf16String),
    /// A binary key (raw bytes).
    Binary(Vec<u8>),
    /// An array key (nested keys).
    Array(Vec<Key>),
}

impl Key {
    /// Validates the key before use (checks for NaN and nesting depth).
    pub fn validate(&self, limits: &LimitConfig) -> Result<(), KeyError> {
        Self::validate_recursive(self, 0, limits)
    }

    fn validate_recursive(key: &Key, depth: usize, limits: &LimitConfig) -> Result<(), KeyError> {
        if depth > limits.max_key_depth {
            return Err(KeyError::MaxDepthExceeded(limits.max_key_depth));
        }
        match key {
            Key::Number(n) => {
                if n.is_nan() {
                    return Err(KeyError::InvalidValue("Number key cannot be NaN".into()));
                }
            }
            Key::Date(d) => {
                if d.is_nan() {
                    return Err(KeyError::InvalidValue("Date key cannot be NaN".into()));
                }
            }
            Key::String(_) | Key::Binary(_) => {}
            Key::Array(arr) => {
                for item in arr {
                    Self::validate_recursive(item, depth + 1, limits)?;
                }
            }
        }
        Ok(())
    }
}
