use std::collections::HashSet;
use std::sync::Mutex;

/// Tracks in-flight refreshes so two callers for the same provider collapse
/// into one run. The set holds provider names currently being refreshed.
#[derive(Default)]
pub struct InFlight {
    inner: Mutex<HashSet<String>>,
}

impl InFlight {
    pub fn new() -> Self {
        Self::default()
    }

    /// Try to claim a slot for this provider. Returns a guard that releases
    /// on drop, or None if the provider is already being refreshed.
    pub fn try_claim(&self, provider: &str) -> Option<InFlightGuard<'_>> {
        let mut set = self.inner.lock().ok()?;
        if set.contains(provider) {
            return None;
        }
        set.insert(provider.to_string());
        Some(InFlightGuard {
            set: &self.inner,
            name: provider.to_string(),
        })
    }
}

pub struct InFlightGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    name: String,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_flight_single_claim() {
        let f = InFlight::new();
        let g = f.try_claim("claude").unwrap();
        assert!(f.try_claim("claude").is_none());
        drop(g);
        assert!(f.try_claim("claude").is_some());
    }

    #[test]
    fn in_flight_different_providers() {
        let f = InFlight::new();
        let _a = f.try_claim("claude").unwrap();
        let _b = f.try_claim("codex").unwrap();
    }
}
