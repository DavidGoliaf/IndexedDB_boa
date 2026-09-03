//! IndexedDB extension registration.

use boa_engine::class::Class;
use boa_engine::{Context, JsNativeError, JsResult, JsValue, js_string};
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::proto::StorageKey;
use std::sync::Arc;

use crate::api::factory::IdBFactory;
use crate::runtime::IdbRuntime;

/// Builder for `IndexedDbExtension`.
#[derive(Default)]
pub struct IndexedDbExtensionBuilder {
    storage_key: Option<StorageKey>,
    backend_factory: Option<Arc<dyn BackendFactory>>,
}

impl IndexedDbExtensionBuilder {
    pub fn storage_key(mut self, key: StorageKey) -> Self {
        self.storage_key = Some(key);
        self
    }

    pub fn backend_factory(mut self, factory: Arc<dyn BackendFactory>) -> Self {
        self.backend_factory = Some(factory);
        self
    }

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
    pub fn builder() -> IndexedDbExtensionBuilder {
        IndexedDbExtensionBuilder::default()
    }

    /// Registers the extension in a Boa context.
    pub fn register(&self, context: &mut Context) -> JsResult<()> {
        // 1. Idempotency check
        if context.get_data::<IdbRuntime>().is_some() {
            return Err(JsNativeError::error()
                .with_message("IndexedDbExtension is already registered in this Context")
                .into());
        }

        // 2. Initialize IdbRuntime and insert into HostDefined
        let runtime = IdbRuntime::new(self.storage_key.clone(), self.backend_factory.clone());
        context.insert_data(runtime);

        // 3. Register DOM shim classes
        context.register_global_class::<crate::dom::exception::DomException>()?;
        context.register_global_class::<crate::dom::event::EventDataHelper>()?;
        context.register_global_class::<crate::dom::string_list::DomStringListData>()?;

        // 4. Register IDB API classes
        context.register_global_class::<crate::api::request::IdBRequest>()?;
        context.register_global_class::<crate::api::request::IdBOpenDBRequest>()?;
        context.register_global_class::<crate::api::factory::IdBFactory>()?;
        context.register_global_class::<crate::api::database::IdBDatabase>()?;
        context.register_global_class::<crate::api::transaction::IdBTransaction>()?;
        context.register_global_class::<crate::api::object_store::IdBObjectStore>()?;
        context.register_global_class::<crate::api::index::IdBIndexData>()?;
        context.register_global_class::<crate::api::key_range::IdBKeyRange>()?;
        context.register_global_class::<crate::api::record::IdBRecordData>()?;
        context.register_global_class::<crate::api::cursor::IdBCursorData>()?;
        context.register_global_class::<crate::api::cursor::IdBCursorWithValueData>()?;
        context
            .register_global_class::<crate::api::version_change_event::IdBVersionChangeEventData>(
            )?;

        // 5. Set 'indexedDB' on globalThis as a singleton IDBFactory
        let factory_obj = IdBFactory::from_data(IdBFactory, context)?;

        context.global_object().set(
            js_string!("indexedDB"),
            JsValue::from(factory_obj),
            false,
            context,
        )?;

        Ok(())
    }
}
