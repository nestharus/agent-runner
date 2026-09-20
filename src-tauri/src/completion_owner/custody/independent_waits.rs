//! Exact direct children whose original birth duty already transferred or
//! integrated. This inventory is wait housekeeping, never operation settlement.
use oulipoly_state::completion_continuation::SourceProcessIdentity;

struct IndependentWait {
    original: u32,
    child: SourceProcessIdentity,
}

thread_local! {
    static WAITS: std::cell::RefCell<Vec<IndependentWait>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(super) fn retain(child: &SourceProcessIdentity) {
    WAITS.with_borrow_mut(|waits| enroll(waits, child));
}

fn enroll(waits: &mut Vec<IndependentWait>, child: &SourceProcessIdentity) {
    if waits.iter().any(|wait| wait.child == *child) {
        return;
    }
    waits.push(IndependentWait {
        original: std::process::id(),
        child: child.clone(),
    });
}

pub(super) fn discard_fork_copies() {
    WAITS.with_borrow_mut(|waits| waits.retain(is_original));
}

fn is_original(wait: &IndependentWait) -> bool {
    wait.original == std::process::id()
}

pub(super) fn reap(status: &mut i32) -> i32 {
    discard_fork_copies();
    WAITS.with_borrow_mut(|waits| reap_one(waits, status))
}

fn reap_one(waits: &mut Vec<IndependentWait>, status: &mut i32) -> i32 {
    let Some(index) = waits.iter().position(|wait| wait.reap(status)) else {
        return 0;
    };
    waits.remove(index).child.pid as i32
}

impl IndependentWait {
    fn reap(&self, status: &mut i32) -> bool {
        if super::super::linux::identity(self.child.pid).as_ref() != Ok(&self.child) {
            return false;
        }
        // Enrollment came from this process's successful fork and retained
        // original wait, not proc enumeration. Never infer global ECHILD from
        // this exact-child operation, and never discharge an attempt here.
        unsafe { libc::waitpid(self.child.pid as i32, status, libc::WNOHANG) > 0 }
    }
}

// A positive wait returned by the existing generic reaper is also actual
// housekeeping evidence. Forget its inventory entry before PID reuse.
pub(super) fn observe_wait(pid: i32) {
    if pid <= 0 {
        return;
    }
    WAITS.with_borrow_mut(|waits| waits.retain(|wait| wait.child.pid != i64::from(pid)));
}
