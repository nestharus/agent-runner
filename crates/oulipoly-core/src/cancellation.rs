use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;

/// Shared cooperative cancellation authority for workspace operations.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    state: Arc<CancellationState>,
}

struct CancellationState {
    cancelled: AtomicBool,
    observers: Mutex<Vec<Weak<CancellationObserver>>>,
}

struct CancellationObserver {
    callback: Box<dyn Fn() + Send + Sync>,
    notified: AtomicBool,
}

/// Keeps one cancellation notification registered until dropped.
pub struct CancellationRegistration {
    _observer: Arc<CancellationObserver>,
}

impl std::fmt::Debug for CancellationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancellationState")
            .field("cancelled", &self.cancelled)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for CancellationRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancellationRegistration")
            .finish_non_exhaustive()
    }
}

impl CancellationObserver {
    fn notify(&self) {
        if !self.notified.swap(true, Ordering::SeqCst) {
            (self.callback)();
        }
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            state: Arc::new(CancellationState {
                cancelled: AtomicBool::new(false),
                observers: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::SeqCst);
        let observers = {
            let mut registered = self
                .state
                .observers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut observers = Vec::with_capacity(registered.len());
            registered.retain(|observer| {
                let Some(observer) = observer.upgrade() else {
                    return false;
                };
                observers.push(observer);
                true
            });
            observers
        };
        for observer in observers {
            observer.notify();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    /// Registers a one-shot notification that remains active while the returned handle is held.
    pub fn register(
        &self,
        observer: impl Fn() + Send + Sync + 'static,
    ) -> CancellationRegistration {
        let observer = Arc::new(CancellationObserver {
            callback: Box::new(observer),
            notified: AtomicBool::new(false),
        });
        {
            let mut registered = self
                .state
                .observers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            registered.retain(|observer| observer.strong_count() > 0);
            registered.push(Arc::downgrade(&observer));
        }
        if self.is_cancelled() {
            observer.notify();
        }
        CancellationRegistration {
            _observer: observer,
        }
    }

    pub fn cancel_after(&self, duration: Duration) {
        let token = self.clone();
        thread::spawn(move || {
            thread::sleep(duration);
            token.cancel();
        });
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::CancellationToken;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn cancellation_notifies_each_live_registration_once() {
        let token = CancellationToken::new();
        let notifications = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&notifications);
        let _registration = token.register(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        });

        token.cancel();
        token.cancel();

        assert_eq!(notifications.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn registration_after_cancellation_is_notified_immediately() {
        let token = CancellationToken::new();
        token.cancel();
        let notifications = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&notifications);

        let _registration = token.register(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(notifications.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropped_registration_is_not_notified() {
        let token = CancellationToken::new();
        let notifications = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&notifications);
        let registration = token.register(move || {
            observed.fetch_add(1, Ordering::SeqCst);
        });
        drop(registration);

        token.cancel();

        assert_eq!(notifications.load(Ordering::SeqCst), 0);
    }
}
