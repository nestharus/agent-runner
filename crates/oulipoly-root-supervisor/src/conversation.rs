//! A live conversation's further caller input. The control reader hands
//! each `send` to the named harness's [`Inbox`] and rings its bell; the
//! harness's worker takes it only between turns, while it waits for the
//! agent's next idle, and decides there whether to admit it. Nothing here
//! is durable: a queued control is not admitted until the worker has
//! committed it as an owed message.
//!
//! `close` is root-wide: it ends input for every harness ([`Closing`]),
//! and each worker stops its harness once its admitted inputs' turns have
//! ended. It is not a cancel and not an end of processing.
//!
//! A recovered survivor that is held rather than conversed with says why
//! ([`Inbox::hold`]), so that a refused `send` names that state instead of
//! a bare `not-in-conversation`. The bell also rings a second, separate
//! descriptor that only a held survivor's close watcher reads, so it never
//! takes a wake meant for a conversation.

use std::collections::VecDeque;
use std::io;

use serde_json::Value;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// One further caller input, as received.
#[derive(Debug, Clone)]
pub(crate) struct FollowUp {
    /// The control line's number on stdin (1-based, every line counted).
    pub(crate) control: u64,
    /// The caller's own correlation string, echoed, never interpreted.
    pub(crate) caller_ref: Option<String>,
    pub(crate) text: String,
    /// Set when this input is the owner's own completion of a background
    /// Bash run of this harness (its work id), not caller input: it is
    /// owed to the harness even after `close`, and is held (not refused)
    /// while an earlier input is open.
    pub(crate) completion: Option<i64>,
    /// Set (to the accepting owner generation) when that completion was
    /// accepted by an earlier owner and is being recovered: only a worker
    /// in a recovered live-usable conversation of the same requester work
    /// admits it, and never a second time.
    pub(crate) recovered: Option<i64>,
}

struct State {
    queue: VecDeque<FollowUp>,
    /// Whether the worker is in a conversation that can take input.
    accepting: bool,
    /// Why a recovered survivor is held without a conversation.
    held: Option<Value>,
}

pub(crate) struct Inbox {
    state: Mutex<State>,
    bell_read: OwnedFd,
    bell_write: OwnedFd,
    hold_read: OwnedFd,
    hold_write: OwnedFd,
}

impl Inbox {
    pub(crate) fn new() -> io::Result<Self> {
        let (bell_read, bell_write) = crate::sys::pipe()?;
        let (hold_read, hold_write) = crate::sys::pipe()?;
        for fd in [&bell_read, &bell_write, &hold_read, &hold_write] {
            // SAFETY: fcntl on a descriptor owned here.
            let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
            // SAFETY: as above.
            if flags < 0
                || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                    < 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                accepting: false,
                held: None,
            }),
            bell_read,
            bell_write,
            hold_read,
            hold_write,
        })
    }

    /// Queues `follow_up` if the worker is in a conversation, and rings.
    /// Otherwise gives it back: the harness is not in a live conversation
    /// now (not yet connected, between connections, held unconnected or
    /// ended).
    pub(crate) fn offer(&self, follow_up: FollowUp) -> Result<(), FollowUp> {
        let mut state = self.state.lock().expect("inbox");
        if !state.accepting {
            return Err(follow_up);
        }
        state.queue.push_back(follow_up);
        drop(state);
        self.ring();
        Ok(())
    }

    /// Whether the worker is now in a conversation that can take input.
    pub(crate) fn accepting(&self) -> bool {
        self.state.lock().expect("inbox").accepting
    }

    /// Wakes a worker waiting between turns (also used for `close`), and
    /// a held survivor's close watcher.
    pub(crate) fn ring(&self) {
        // A full pipe already holds a wake; nothing else can fail here
        // that a later ring would not repeat.
        for fd in [&self.bell_write, &self.hold_write] {
            // SAFETY: write of one byte from a valid buffer to an owned fd.
            let _ = unsafe { libc::write(fd.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        }
    }

    /// The descriptor that is readable while a wake is pending.
    pub(crate) fn bell(&self) -> &OwnedFd {
        &self.bell_read
    }

    /// The held survivor's watcher's own wake descriptor.
    pub(crate) fn hold_bell(&self) -> &OwnedFd {
        &self.hold_read
    }

    /// Drains the held survivor's watcher's pending wakes.
    pub(crate) fn drain_hold(&self) {
        drain(&self.hold_read);
    }

    /// Drains pending wakes, then takes what is queued.
    pub(crate) fn take(&self) -> Vec<FollowUp> {
        drain(&self.bell_read);
        self.state.lock().expect("inbox").queue.drain(..).collect()
    }

    /// Records why this recovered survivor is held without a conversation;
    /// a refused `send` reports it.
    pub(crate) fn hold(&self, conversation: Value) {
        self.state.lock().expect("inbox").held = Some(conversation);
    }

    /// Why this harness is held without a conversation, if it is.
    pub(crate) fn held(&self) -> Option<Value> {
        self.state.lock().expect("inbox").held.clone()
    }

    /// Opens input (the worker entered a conversation).
    pub(crate) fn open(&self) {
        let mut state = self.state.lock().expect("inbox");
        state.accepting = true;
        state.held = None;
    }

    /// Closes input and returns what was queued but not taken: the worker
    /// left its conversation, so the caller is told each was not admitted.
    pub(crate) fn shut(&self) -> Vec<FollowUp> {
        let mut state = self.state.lock().expect("inbox");
        state.accepting = false;
        state.queue.drain(..).collect()
    }
}

/// Reads every pending byte of a non-blocking wake pipe.
fn drain(fd: &OwnedFd) {
    let mut buf = [0u8; 64];
    // SAFETY: non-blocking reads into a valid buffer from an owned fd.
    while unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
}

/// The root-wide close request: refuse new input, then attempt to stop
/// live harnesses after tagged turn ends. In-flight admission can finish
/// after this flag is set; actual exits are waited separately.
#[derive(Default)]
pub(crate) struct Closing(AtomicBool);

impl Closing {
    pub(crate) fn request(&self) -> bool {
        !self.0.swap(true, Ordering::SeqCst)
    }

    pub(crate) fn requested(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
