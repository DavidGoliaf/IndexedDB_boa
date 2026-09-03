//! Runtime state for IndexedDB.

use boa_engine::Context;
use boa_gc::{Finalize, Trace};
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::proto::StorageKey;
use std::cell::RefCell;
use std::sync::Arc;

use crate::engine::IdbEngine;

/// Runtime state for IndexedDB, stored in the Boa Context's HostDefined data.
#[derive(Trace, Finalize, boa_engine::JsData)]
pub struct IdbRuntime {
    /// Storage key for this context.
    #[unsafe_ignore_trace]
    pub storage_key: StorageKey,
    /// Backend factory for creating storage instances.
    #[unsafe_ignore_trace]
    pub backend_factory: Arc<dyn BackendFactory>,
    /// The engine (interior mutability for access from &self methods).
    #[unsafe_ignore_trace]
    pub engine: RefCell<Option<IdbEngine>>,
}

impl IdbRuntime {
    /// Creates a new `IdbRuntime`.
    pub fn new(storage_key: StorageKey, backend_factory: Arc<dyn BackendFactory>) -> Self {
        Self {
            storage_key,
            backend_factory,
            engine: RefCell::new(None),
        }
    }

    /// Initializes the engine if not already done.
    pub fn ensure_engine(&self) {
        let mut engine = self.engine.borrow_mut();
        if engine.is_none() {
            *engine = Some(IdbEngine::new(
                self.backend_factory.clone(),
                &self.storage_key,
            ));
        }
    }

    /// Executes a closure with mutable access to the engine.
    pub fn with_engine<R>(&self, f: impl FnOnce(&mut IdbEngine) -> R) -> R {
        self.ensure_engine();
        let mut engine = self.engine.borrow_mut();
        let engine = engine.as_mut().unwrap();
        f(engine)
    }
}

/// Hook called at the end of each task to deactivate transactions.
pub fn end_of_task(context: &mut Context) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        runtime.with_engine(|engine| {
            engine.cleanup_active_transactions();
        });
    }
}
