//! IndexedDB extension registration.

use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::proto::StorageKey;
use std::sync::Arc;

use crate::runtime::IdbRuntime;

/// Builder for `IndexedDbExtension`.
#[derive(Default)]
pub struct IndexedDbExtensionBuilder {
    storage_key: Option<StorageKey>,
    backend_factory: Option<Arc<dyn BackendFactory>>,
}

impl IndexedDbExtensionBuilder {
    /// Sets the storage key.
    pub fn storage_key(mut self, key: StorageKey) -> Self {
        self.storage_key = Some(key);
        self
    }

    /// Sets the backend factory.
    pub fn backend_factory(mut self, factory: Arc<dyn BackendFactory>) -> Self {
        self.backend_factory = Some(factory);
        self
    }

    /// Builds the extension.
    pub fn build(self) -> JsResult<IndexedDbExtension> {
        let storage_key = self
            .storage_key
            .ok_or_else(|| JsNativeError::error().with_message("storage_key is required"))?;
        let backend_factory = self
            .backend_factory
            .ok_or_else(|| JsNativeError::error().with_message("backend_factory is required"))?;
        Ok(IndexedDbExtension {
            storage_key,
            backend_factory,
        })
    }
}

/// IndexedDB extension for Boa.
pub struct IndexedDbExtension {
    storage_key: StorageKey,
    backend_factory: Arc<dyn BackendFactory>,
}

impl IndexedDbExtension {
    /// Creates a new builder.
    pub fn builder() -> IndexedDbExtensionBuilder {
        IndexedDbExtensionBuilder::default()
    }

    /// Registers the extension in a Boa context.
    pub fn register(&self, context: &mut Context) -> JsResult<()> {
        // 1. Initialize IdbRuntime and insert into HostDefined
        let runtime = IdbRuntime::new(self.storage_key.clone(), self.backend_factory.clone());
        context.insert_data(runtime);

        // 2. Register 'indexedDB' on globalThis
        let factory_obj = JsObject::with_null_proto();
        context.global_object().set(
            js_string!("indexedDB"),
            JsValue::from(factory_obj),
            false,
            context,
        )?;

        Ok(())
    }
}
