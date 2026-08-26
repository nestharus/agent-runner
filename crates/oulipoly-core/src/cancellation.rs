use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// Shared cooperative cancellation authority for workspace operations.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
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
