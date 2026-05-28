use super::artifact_key::ArtifactKey;
use oulipoly_provider::generated::DescribeResult;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct DescribeCache {
    entries: Mutex<HashMap<ArtifactKey, DescribeResult>>,
}

impl DescribeCache {
    pub fn get(&self, key: &str) -> Option<DescribeResult> {
        self.entries
            .lock()
            .expect("provider registry cache mutex should not be poisoned")
            .get(key)
            .cloned()
    }

    pub fn insert(&self, key: ArtifactKey, result: DescribeResult) {
        self.entries
            .lock()
            .expect("provider registry cache mutex should not be poisoned")
            .insert(key, result);
    }
}
