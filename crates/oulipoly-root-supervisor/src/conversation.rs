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

use std::collections::VecDeque;
use std::io;
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
}

struct State {
    queue: VecDeque<FollowUp>,
    /// Whether the worker is in a conversation that can take input.
    accepting: bool,
}

pub(crate) struct Inbox {
    state: Mutex<State>,
    bell_read: OwnedFd,
    bell_write: OwnedFd,
}

impl Inbox {
    pub(crate) fn new() -> io::Result<Self> {
        let (bell_read, bell_write) = crate::sys::pipe()?;
        for fd in [&bell_read, &bell_write] {
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
            }),
            bell_read,
            bell_write,
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

    /// Wakes a worker waiting between turns (also used for `close`).
    pub(crate) fn ring(&self) {
        // A full pipe already holds a wake; nothing else can fail here
        // that a later ring would not repeat.
        // SAFETY: write of one byte from a valid buffer to an owned fd.
        let _ = unsafe { libc::write(self.bell_write.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
    }

    /// The descriptor that is readable while a wake is pending.
    pub(crate) fn bell(&self) -> &OwnedFd {
        &self.bell_read
    }

    /// Drains pending wakes, then takes what is queued.
    pub(crate) fn take(&self) -> Vec<FollowUp> {
        let mut buf = [0u8; 64];
        // SAFETY: non-blocking reads into a valid buffer from an owned fd.
        while unsafe {
            libc::read(
                self.bell_read.as_raw_fd(),
                buf.as_mut_ptr().cast(),
                buf.len(),
            )
        } > 0
        {}
        self.state.lock().expect("inbox").queue.drain(..).collect()
    }

    /// Opens input (the worker entered a conversation).
    pub(crate) fn open(&self) {
        self.state.lock().expect("inbox").accepting = true;
    }

    /// Closes input and returns what was queued but not taken: the worker
    /// left its conversation, so the caller is told each was not admitted.
    pub(crate) fn shut(&self) -> Vec<FollowUp> {
        let mut state = self.state.lock().expect("inbox");
        state.accepting = false;
        state.queue.drain(..).collect()
    }
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
