use super::artifact_key::ArtifactKey;
use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::generated::DescribeResult;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Successful describes keyed by artifact. Each entry keeps the client whose
/// pinned revision produced it, so a hit is served only while the configured
/// artifact still resolves to that revision.
#[derive(Debug, Default)]
pub struct DescribeCache {
    entries: Mutex<HashMap<ArtifactKey, (DescribeResult, Arc<ProviderClient>)>>,
}

impl DescribeCache {
    pub fn get(&self, key: &str) -> Option<DescribeResult> {
        let entry = self
            .entries
            .lock()
            .expect("provider registry cache mutex should not be poisoned")
            .get(key)
            .cloned()?;
        // Checked outside the lock: revision checks touch the filesystem.
        if entry.1.configured_artifact_unchanged() {
            return Some(entry.0);
        }
        let mut entries = self
            .entries
            .lock()
            .expect("provider registry cache mutex should not be poisoned");
        if entries
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(&current.1, &entry.1))
        {
            entries.remove(key);
        }
        None
    }

    pub fn insert(&self, key: ArtifactKey, result: DescribeResult, client: Arc<ProviderClient>) {
        self.entries
            .lock()
            .expect("provider registry cache mutex should not be poisoned")
            .insert(key, (result, client));
    }
}

type Slot<T> = Arc<Mutex<Option<Arc<T>>>>;

/// Per-key single-flight slots. The map lock is held only to find a slot, so
/// a describe for one key never blocks lookups or describes for other keys.
#[derive(Debug)]
pub struct EndpointSlots<T> {
    slots: Mutex<HashMap<String, Slot<T>>>,
}

impl<T> Default for EndpointSlots<T> {
    fn default() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }
}

impl<T> EndpointSlots<T> {
    pub fn slot(&self, key: &str) -> Slot<T> {
        self.slots
            .lock()
            .expect("provider endpoint cache mutex should not be poisoned")
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    pub fn populated(&self) -> Vec<Arc<T>> {
        let slots = self
            .slots
            .lock()
            .expect("provider endpoint cache mutex should not be poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        slots
            .iter()
            .filter_map(|slot| {
                slot.lock()
                    .expect("provider endpoint slot mutex should not be poisoned")
                    .clone()
            })
            .collect()
    }
}
