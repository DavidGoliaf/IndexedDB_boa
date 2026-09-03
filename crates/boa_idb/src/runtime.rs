//! Runtime state for IndexedDB.

use boa_gc::{Finalize, Trace};
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::proto::StorageKey;
use std::sync::Arc;

/// Runtime state for IndexedDB, stored in the Boa Context's HostDefined data.
#[derive(Trace, Finalize, boa_engine::JsData)]
pub struct IdbRuntime {
    /// Storage key for this context.
    #[unsafe_ignore_trace]
    pub storage_key: StorageKey,
    /// Backend factory for creating storage instances.
    #[unsafe_ignore_trace]
    pub backend_factory: Arc<dyn BackendFactory>,
}

impl IdbRuntime {
    /// Creates a new `IdbRuntime`.
    pub fn new(storage_key: StorageKey, backend_factory: Arc<dyn BackendFactory>) -> Self {
        Self {
            storage_key,
            backend_factory,
        }
    }
}
