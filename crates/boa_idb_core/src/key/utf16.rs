//! UTF-16 string type capable of storing unpaired surrogates.

use smallvec::SmallVec;
use std::fmt;

/// A string of UTF-16 code units that can store unpaired surrogates.
///
/// This type is used instead of `String` because JavaScript strings can contain
/// unpaired surrogates (`U+D800..=U+DFFF`), and Rust's `String` enforces valid UTF-8
/// which would corrupt such sequences.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Utf16String {
    units: SmallVec<[u16; 16]>,
}

impl Utf16String {
    /// Creates a new empty `Utf16String`.
    pub fn new() -> Self {
        Self {
            units: SmallVec::new(),
        }
    }

    /// Creates a `Utf16String` from a slice of UTF-16 code units.
    pub fn from_slice(slice: &[u16]) -> Self {
        Self {
            units: SmallVec::from_slice(slice),
        }
    }

    /// Creates a `Utf16String` from a Rust `&str`.
    ///
    /// Named `from_str` per the project specification (TASK-01 §4.3); it does
    /// not implement [`std::str::FromStr`] because the conversion is infallible.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        Self {
            units: s.encode_utf16().collect(),
        }
    }

    /// Creates a `Utf16String` from a Rust `&str`.
    ///
    /// Alias of [`Utf16String::from_str`] kept for backward compatibility.
    pub fn from_rust_str(s: &str) -> Self {
        Self::from_str(s)
    }

    /// Returns the underlying UTF-16 code units as a slice.
    pub fn as_slice(&self) -> &[u16] {
        &self.units
    }

    /// Returns the number of UTF-16 code units.
    pub fn len(&self) -> usize {
        self.units.len()
    }

    /// Returns `true` if the string is empty.
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// Appends a UTF-16 code unit.
    pub fn push(&mut self, unit: u16) {
        self.units.push(unit);
    }

    /// Extends the string with a slice of UTF-16 code units.
    pub fn extend_from_slice(&mut self, slice: &[u16]) {
        self.units.extend_from_slice(slice);
    }
}

impl fmt::Debug for Utf16String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Utf16String(\"{}\")",
            String::from_utf16_lossy(&self.units)
        )
    }
}

impl fmt::Display for Utf16String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf16_lossy(&self.units))
    }
}

impl From<&str> for Utf16String {
    fn from(s: &str) -> Self {
        Self::from_rust_str(s)
    }
}

impl From<String> for Utf16String {
    fn from(s: String) -> Self {
        Self::from_rust_str(&s)
    }
}

impl From<&[u16]> for Utf16String {
    fn from(slice: &[u16]) -> Self {
        Self::from_slice(slice)
    }
}

impl From<Vec<u16>> for Utf16String {
    fn from(vec: Vec<u16>) -> Self {
        Self {
            units: SmallVec::from_vec(vec),
        }
    }
}
