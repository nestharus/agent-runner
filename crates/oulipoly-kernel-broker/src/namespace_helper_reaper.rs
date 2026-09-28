//! One Broker thread waits for registered, exact outside helper children.
//! Stable helpers retain their own child waits and receipts after Broker exit.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
#[cfg(test)]
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

// The bound includes reservations made before fork. Exhaustion refuses the
// launch before any helper exists; kernel fd exhaustion is also fail closed.
const MAX_HELPERS: usize = 16_384;
const REGISTRATION_LOCK_TIMEOUT: Duration = Duration::from_secs(1);
static REAPER: OnceLock<Mutex<Option<Arc<Reaper>>>> = OnceLock::new();
#[cfg(test)]
static FAIL_REGISTRATION_PID: AtomicI32 = AtomicI32::new(0);

struct Child {
    pid: i32,
    _pidfd: File,
}

struct State {
    children: BTreeMap<u64, Child>,
    slots: usize,
    next_token: u64,
    #[cfg(test)]
    completed: usize,
}

struct Reaper {
    epoll: File,
    broker_pidfd: File,
    state: Mutex<State>,
}

pub(super) struct Reservation {
    reaper: Arc<Reaper>,
    registered: bool,
}

fn registration_lock<T>(mutex: &Mutex<T>) -> io::Result<MutexGuard<'_, T>> {
    let deadline = Instant::now() + REGISTRATION_LOCK_TIMEOUT;
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(_)) => {
                return Err(io::Error::other("namespace helper reaper lock poisoned"));
            }
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::yield_now();
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::other(
                    "namespace helper reaper registration blocked",
                ));
            }
        }
    }
}

fn reaper() -> io::Result<Arc<Reaper>> {
    let mut global = registration_lock(REAPER.get_or_init(|| Mutex::new(None)))?;
    if let Some(reaper) = global.as_ref() {
        return Ok(Arc::clone(reaper));
    }
    let fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let broker_fd = unsafe { libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0) as i32 };
    if broker_fd < 0 {
        let error = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(error);
    }
    let reaper = Arc::new(Reaper {
        epoll: unsafe { File::from_raw_fd(fd) },
        broker_pidfd: unsafe { File::from_raw_fd(broker_fd) },
        state: Mutex::new(State {
            children: BTreeMap::new(),
            slots: 0,
            next_token: 1,
            #[cfg(test)]
            completed: 0,
        }),
    });
    let worker = Arc::clone(&reaper);
    std::thread::Builder::new()
        .name("namespace-helper-reaper".into())
        .spawn(move || worker.run())?;
    *global = Some(Arc::clone(&reaper));
    Ok(reaper)
}

impl Reaper {
    fn run(&self) {
        let mut events = [libc::epoll_event { events: 0, u64: 0 }; 64];
        loop {
            let count = unsafe {
                libc::epoll_wait(
                    self.epoll.as_raw_fd(),
                    events.as_mut_ptr(),
                    events.len() as i32,
                    -1,
                )
            };
            if count < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                // A dead worker would leave every later helper a Broker zombie.
                std::process::abort();
            }
            for event in &events[..count as usize] {
                let token = event.u64;
                let mut state = self.state.lock().unwrap_or_else(|_| std::process::abort());
                let Some(child) = state.children.get(&token) else {
                    continue;
                };
                let mut status = 0;
                let waited = loop {
                    let waited = unsafe { libc::waitpid(child.pid, &mut status, libc::WNOHANG) };
                    if waited >= 0
                        || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
                    {
                        break waited;
                    }
                };
                if waited != child.pid {
                    // An epoll-ready pinned child must be waitable by this
                    // Broker. Never continue accepting launches if it is not.
                    std::process::abort();
                }
                let child = state
                    .children
                    .remove(&token)
                    .unwrap_or_else(|| std::process::abort());
                unsafe {
                    libc::epoll_ctl(
                        self.epoll.as_raw_fd(),
                        libc::EPOLL_CTL_DEL,
                        child._pidfd.as_raw_fd(),
                        std::ptr::null_mut(),
                    )
                };
                state.slots -= 1;
                #[cfg(test)]
                {
                    state.completed += 1;
                }
            }
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.registered {
            let mut state = self
                .reaper
                .state
                .lock()
                .unwrap_or_else(|_| std::process::abort());
            state.slots -= 1;
        }
    }
}

fn reserve() -> io::Result<Reservation> {
    let reaper = reaper()?;
    let mut state = registration_lock(&reaper.state)?;
    if state.slots >= MAX_HELPERS || state.next_token == u64::MAX {
        return Err(io::Error::other(
            "namespace helper reaper capacity exhausted",
        ));
    }
    state.slots += 1;
    drop(state);
    Ok(Reservation {
        reaper,
        registered: false,
    })
}

impl Reservation {
    pub(super) fn broker_pidfd(&self) -> i32 {
        self.reaper.broker_pidfd.as_raw_fd()
    }

    fn register(&mut self, pid: i32) -> io::Result<()> {
        #[cfg(test)]
        if FAIL_REGISTRATION_PID.load(Ordering::Relaxed) == pid {
            return Err(io::Error::other("injected pidfd registration failure"));
        }
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let pidfd = unsafe { File::from_raw_fd(fd) };
        let mut state = registration_lock(&self.reaper.state)?;
        let token = state.next_token;
        state.next_token += 1;
        let mut event = libc::epoll_event {
            events: libc::EPOLLIN as u32,
            u64: token,
        };
        if unsafe {
            libc::epoll_ctl(
                self.reaper.epoll.as_raw_fd(),
                libc::EPOLL_CTL_ADD,
                fd,
                &mut event,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        state.children.insert(token, Child { pid, _pidfd: pidfd });
        self.registered = true;
        Ok(())
    }
}

/// Reserve capacity before fork. The child must remain behind this gate until
/// its exact pidfd has been registered; an error closes it and permits only a
/// short, synchronous wait of the still-gated direct child.
pub(super) fn prepare() -> io::Result<(Reservation, UnixStream, UnixStream)> {
    let reservation = reserve()?;
    let (parent, child) = UnixStream::pair()?;
    Ok((reservation, parent, child))
}

pub(super) fn await_permit(mut gate: UnixStream, broker_pidfd: i32) -> bool {
    // A later helper may inherit a copy of the gate writer. The pinned Broker
    // pidfd still wakes this child if Broker dies before registration.
    let mut poll = [
        libc::pollfd {
            fd: gate.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: broker_pidfd,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        let ready = unsafe { libc::poll(poll.as_mut_ptr(), poll.len() as _, -1) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return false;
        }
        if poll[0].revents != 0 {
            let mut byte = [0];
            return gate.read_exact(&mut byte).is_ok() && byte == [b'P'];
        }
        if poll[1].revents != 0 {
            return false;
        }
    }
}

pub(super) fn activate(
    mut reservation: Reservation,
    pid: i32,
    mut gate: UnixStream,
) -> io::Result<()> {
    if let Err(error) = reservation.register(pid) {
        // shutdown applies to all fork-inherited duplicates of this endpoint.
        let _ = gate.shutdown(std::net::Shutdown::Write);
        drop(gate);
        let mut status = 0;
        loop {
            let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
            if waited == pid {
                break;
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return Err(io::Error::other(
                    "failed to reap unregistered namespace helper",
                ));
            }
        }
        return Err(error);
    }
    let result = gate.write_all(b"P");
    if result.is_err() {
        let _ = gate.shutdown(std::net::Shutdown::Write);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn thread_count() -> usize {
        std::fs::read_dir("/proc/self/task").unwrap().count()
    }

    #[test]
    fn blocked_registration_lock_returns_error() {
        let lock = Mutex::new(());
        let _held = lock.lock().unwrap();
        assert!(registration_lock(&lock).is_err());
    }

    #[test]
    fn dead_broker_pidfd_releases_gate_even_with_writer_open() {
        let (parent_gate, child_gate) = UnixStream::pair().unwrap();
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe { libc::_exit(0) };
        }
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
        assert!(fd >= 0);
        let pidfd = unsafe { File::from_raw_fd(fd) };
        assert!(!await_permit(child_gate, pidfd.as_raw_fd()));
        drop(parent_gate);
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    }

    #[test]
    fn many_exact_helpers_use_one_worker_and_leave_unrelated_child_waitable() {
        let before_threads = thread_count();
        let reaper = reaper().unwrap();
        let before_completed = reaper.state.lock().unwrap().completed;
        let (mut unrelated_parent, mut unrelated_child) = UnixStream::pair().unwrap();
        let unrelated = unsafe { libc::fork() };
        assert!(unrelated >= 0);
        if unrelated == 0 {
            drop(unrelated_parent);
            let mut byte = [0];
            let _ = unrelated_child.read_exact(&mut byte);
            unsafe { libc::_exit(0) };
        }
        drop(unrelated_child);

        let mut pending = Vec::new();
        for _ in 0..128 {
            let (reservation, parent_permit, child_permit) = prepare().unwrap();
            let (parent_work, mut child_work) = UnixStream::pair().unwrap();
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0);
            if pid == 0 {
                drop(parent_permit);
                drop(parent_work);
                if !await_permit(child_permit, reservation.broker_pidfd()) {
                    unsafe { libc::_exit(70) };
                }
                let mut byte = [0];
                let ok = child_work.read_exact(&mut byte).is_ok() && byte == [b'X'];
                unsafe { libc::_exit(if ok { 0 } else { 70 }) };
            }
            drop(child_permit);
            drop(child_work);
            pending.push((pid, reservation, parent_permit, parent_work));
        }
        let mut groups: Vec<Vec<_>> = (0..8).map(|_| Vec::new()).collect();
        for (index, helper) in pending.into_iter().enumerate() {
            groups[index % 8].push(helper);
        }
        let helpers = std::thread::scope(|scope| {
            let handles: Vec<_> = groups
                .into_iter()
                .map(|group| {
                    scope.spawn(move || {
                        group
                            .into_iter()
                            .map(|(pid, reservation, permit, work)| {
                                activate(reservation, pid, permit).unwrap();
                                (pid, work)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        let active_threads = thread_count();
        for (pid, _) in &helpers {
            let mut status = 0;
            assert_eq!(
                unsafe { libc::waitpid(*pid, &mut status, libc::WNOHANG) },
                0
            );
        }
        eprintln!(
            "helper reaper: helpers_alive={} unrelated_alive=1 broker_threads_before={} broker_threads_active={}",
            helpers.len(),
            before_threads,
            active_threads
        );
        assert!(
            active_threads <= before_threads + 1,
            "one worker per helper regressed"
        );
        for (_, mut gate) in helpers {
            gate.write_all(b"X").unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let state = reaper.state.lock().unwrap();
            if state.completed >= before_completed + 128 {
                assert_eq!(state.slots, 0);
                break;
            }
            drop(state);
            assert!(
                Instant::now() < deadline,
                "exact helper reaps did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(unrelated, &mut status, libc::WNOHANG) },
            0
        );
        unrelated_parent.write_all(b"X").unwrap();
        assert_eq!(
            unsafe { libc::waitpid(unrelated, &mut status, 0) },
            unrelated
        );
    }

    #[test]
    fn registration_failure_reaps_still_gated_exact_child() {
        let (reservation, parent_permit, child_permit) = prepare().unwrap();
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            drop(parent_permit);
            unsafe {
                libc::_exit(if await_permit(child_permit, reservation.broker_pidfd()) {
                    71
                } else {
                    70
                })
            };
        }
        drop(child_permit);
        FAIL_REGISTRATION_PID.store(pid, Ordering::Relaxed);
        let result = activate(reservation, pid, parent_permit);
        FAIL_REGISTRATION_PID.store(0, Ordering::Relaxed);
        assert!(result.is_err());
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn exited_before_registration_is_still_reaped_exactly() {
        let mut reservation = reserve().unwrap();
        let reaper = Arc::clone(&reservation.reaper);
        let before = reaper.state.lock().unwrap().completed;
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe { libc::_exit(0) };
        }
        std::thread::sleep(Duration::from_millis(20));
        reservation.register(pid).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while reaper.state.lock().unwrap().completed == before {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
