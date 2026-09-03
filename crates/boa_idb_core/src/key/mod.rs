//! Key types, comparison, encoding, ranges, and key paths for IndexedDB.
//!
//! Keys follow the W3C IndexedDB §2.4 specification:
//! `Number < Date < String < Binary < Array`.

pub mod compare;
pub mod encode;
pub mod path;
pub mod range;
pub mod utf16;
pub mod value;
