use super::ProviderRegistry;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub struct ProviderRegistryHandle {
    inner: Arc<RwLock<Arc<ProviderRegistry>>>,
}

impl ProviderRegistryHandle {
    pub fn new(registry: Arc<ProviderRegistry>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(registry)),
        }
    }

    pub fn current(&self) -> Arc<ProviderRegistry> {
        self.inner
            .read()
            .expect("provider registry handle should not be poisoned")
            .clone()
    }

    pub fn replace(&self, registry: Arc<ProviderRegistry>) {
        *self
            .inner
            .write()
            .expect("provider registry handle should not be poisoned") = registry;
    }
}
