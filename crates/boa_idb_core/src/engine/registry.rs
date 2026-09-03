//! Database and store registry.

use crate::backend::types::DatabaseMeta;
use std::collections::HashMap;

/// Entry in the database registry.
#[derive(Debug)]
pub struct DatabaseEntry {
    /// Database metadata.
    pub meta: DatabaseMeta,
    /// Number of active connections.
    pub connection_count: u32,
}

/// Registry of databases and their metadata.
#[derive(Debug, Default)]
pub struct Registry {
    databases: HashMap<String, DatabaseEntry>,
}

impl Registry {
    /// Creates a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a database.
    pub fn register(&mut self, meta: DatabaseMeta) {
        let name = meta.name.to_string();
        self.databases.insert(
            name,
            DatabaseEntry {
                meta,
                connection_count: 0,
            },
        );
    }

    /// Returns metadata for a database.
    pub fn get(&self, name: &str) -> Option<&DatabaseMeta> {
        self.databases.get(name).map(|e| &e.meta)
    }

    /// Returns mutable metadata for a database.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut DatabaseMeta> {
        self.databases.get_mut(name).map(|e| &mut e.meta)
    }

    /// Increments the connection count for a database.
    pub fn add_connection(&mut self, name: &str) {
        if let Some(entry) = self.databases.get_mut(name) {
            entry.connection_count += 1;
        }
    }

    /// Decrements the connection count for a database.
    pub fn remove_connection(&mut self, name: &str) {
        if let Some(entry) = self.databases.get_mut(name) {
            entry.connection_count = entry.connection_count.saturating_sub(1);
        }
    }

    /// Returns the connection count for a database.
    pub fn connection_count(&self, name: &str) -> u32 {
        self.databases.get(name).map_or(0, |e| e.connection_count)
    }

    /// Removes a database from the registry.
    pub fn unregister(&mut self, name: &str) -> Option<DatabaseEntry> {
        self.databases.remove(name)
    }

    /// Lists all registered database names.
    pub fn list_names(&self) -> Vec<String> {
        self.databases.keys().cloned().collect()
    }
}
