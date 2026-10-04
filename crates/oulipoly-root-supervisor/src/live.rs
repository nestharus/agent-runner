//! The work this owner is driving, and whether this run has stopped, and
//! why. Launching and stopping both hold this lock, so no work can be
//! launched after a stop without being stopped too.

use std::collections::HashMap;
use std::sync::Arc;

use crate::custody::Root;
use crate::transport::StopSignal;

#[derive(Default)]
pub(crate) struct Custody {
    stopped: Option<&'static str>,
    next: u64,
    live: HashMap<u64, (Arc<Root>, i64)>,
    stop: Option<Arc<StopSignal>>,
}

impl Custody {
    pub(crate) fn new(stop: Arc<StopSignal>) -> Self {
        Self {
            stop: Some(stop),
            ..Self::default()
        }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.stopped.is_some()
    }

    /// Store authority loss outranks store failure, which outranks cancel.
    /// Root PID 1 superseding this owner is authority loss too, however it
    /// was learned: from a store refusal or from the root itself.
    pub(crate) fn reason(&self) -> Option<&'static str> {
        if self.stop.as_ref().is_some_and(|stop| stop.superseded()) {
            return Some("authority-lost");
        }
        self.stopped
    }

    /// Registers live work. Work registered after authority loss is already
    /// known (a survivor adopted late) is left to the successor at once, so
    /// nothing waits on work this owner may no longer act on.
    pub(crate) fn register(&mut self, root: Arc<Root>, work: i64) -> u64 {
        if self.reason() == Some("authority-lost") {
            root.detach();
        }
        self.next += 1;
        self.live.insert(self.next, (root, work));
        self.next
    }

    /// Called once the work's end has been reported, or it was left.
    pub(crate) fn release(&mut self, token: u64) {
        self.live.remove(&token);
    }

    /// Marks the run cancelled by the caller and has every live harness
    /// killed by its own work PID 1. Returns how many kills were requested.
    pub(crate) fn cancel(&mut self) -> usize {
        self.stop("cancelled")
    }

    /// Stops the run, retaining the strongest observed store loss even if
    /// cancel arrived first. On cancel or store failure, this owner has its
    /// live harnesses killed. On authority loss it kills nothing: a newer
    /// owner holds this root's lineage and its live work, so this owner
    /// only detaches.
    pub(crate) fn stop(&mut self, reason: &'static str) -> usize {
        self.stopped = Some(match (self.reason(), reason) {
            (Some("authority-lost"), _) | (_, "authority-lost") => "authority-lost",
            (Some("store-failed"), _) | (_, "store-failed") => "store-failed",
            (Some(earlier), _) => earlier,
            (None, reason) => reason,
        });
        if self.stopped == Some("authority-lost") {
            for (root, _) in self.live.values() {
                root.detach();
            }
            if let Some(stop) = &self.stop {
                stop.trigger();
            }
            return 0;
        }
        self.live
            .values()
            .filter(|(root, work)| root.kill(*work))
            .count()
    }
}
