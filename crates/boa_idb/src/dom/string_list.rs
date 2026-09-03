//! `DOMStringList` implementation.

use boa_gc::{Finalize, Trace};

/// Native data for `DOMStringList`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct DomStringListData {
    /// The list of strings.
    #[unsafe_ignore_trace]
    pub items: Vec<String>,
}

impl DomStringListData {
    /// Creates a new `DOMStringList` with the given items.
    pub fn new(items: Vec<String>) -> Self {
        Self { items }
    }

    /// Returns the length of the list.
    pub fn length(&self) -> u32 {
        self.items.len() as u32
    }

    /// Returns the item at the given index.
    pub fn item(&self, index: u32) -> Option<&str> {
        self.items.get(index as usize).map(|s| s.as_str())
    }

    /// Returns true if the list contains the given string.
    pub fn contains(&self, value: &str) -> bool {
        self.items.iter().any(|s| s == value)
    }
}
