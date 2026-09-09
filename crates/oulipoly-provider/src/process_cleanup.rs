//! In-process retention for unconfirmed direct-child/worker cleanup.
//! Roles: orchestration. No admission, successor, or runtime authority.
use super::*;
use std::sync::OnceLock;

type Pending = Box<dyn FnMut() -> bool + Send>;
#[derive(Default)]
struct Retained {
    tasks: Vec<Pending>,
    running: bool,
}
fn retained() -> &'static Mutex<Retained> {
    static RETAINED: OnceLock<Mutex<Retained>> = OnceLock::new();
    RETAINED.get_or_init(Mutex::default)
}

pub(super) fn retain(
    mut child: Child,
    mut workers_finished: impl FnMut() -> bool + Send + 'static,
) {
    child.uncertain(); // sticky; later cleanup cannot retrospectively certify the returned outcome
    // A spawn-observer failure precedes worker creation. Release its still-owned
    // pipes as well; retained stdin must not itself prevent natural EOF/exit.
    drop(child.stdin.take());
    drop(child.stdout.take());
    drop(child.stderr.take());
    let task = Box::new(move || {
        let workers_done = workers_finished();
        if child.is_reaped() {
            return workers_done;
        }
        #[cfg(unix)]
        {
            // Revalidate while still holding the unreaped leader. Missing initial
            // identity stays unsignalable; only natural exit permits its reap.
            if child.can_signal_group() {
                kill_tree(&mut child);
            }
            if workers_done && child_exited_without_reaping(&child).unwrap_or(false) {
                return child.wait().is_ok();
            }
        }
        #[cfg(not(unix))]
        {
            kill_tree(&mut child);
            if workers_done && child.try_wait().ok().flatten().is_some() {
                return true;
            }
        }
        false
    });
    let mut state = retained().lock().unwrap_or_else(|e| e.into_inner());
    state.tasks.push(task);
    if state.running {
        return;
    }
    state.running = true;
    if thread::Builder::new()
        .name("provider-owned-cleanup".into())
        .spawn(drain)
        .is_err()
    {
        // The queue still owns every handle. A later enqueue retries worker
        // startup; no synchronous unbounded wait or dropped obligation.
        state.running = false;
    }
}
fn drain() {
    loop {
        // Do not hold the admission lock during OS observation or worker joins.
        let tasks = {
            let mut state = retained().lock().unwrap_or_else(|e| e.into_inner());
            if state.tasks.is_empty() {
                state.running = false;
                return;
            }
            std::mem::take(&mut state.tasks)
        };
        let mut pending = Vec::new();
        for mut task in tasks {
            if !task() {
                pending.push(task);
            }
        }
        retained()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tasks
            .extend(pending);
        thread::sleep(STATUS_POLL_INTERVAL);
    }
}
