//! Root PID 1 of one root's PID namespace, and the per-work PID 1 it starts
//! for each harness. Started only by the root's owner (the
//! `oulipoly-root-supervisor` process); see the library's custody docs.
//!
//! Root PID 1 is the actual parent of every per-work PID 1, and holds the
//! owner-side ends of each harness's stdio pipes so that the owner's death
//! is not end of stream for a harness. A per-work PID 1 is the actual
//! parent of its one harness and reports that harness's wait status. Both
//! are single-threaded. Neither has a timer or a parent-death signal, and
//! neither kills anything except on an attested owner's explicit request.
//!
//! Descriptor 3 is the starting owner's channel. Its first message is
//! `{"op":"init","store","incarnation","token","generation","workload"}`.
//! `workload` is `null` or `{"uid","gid","groups"}`, a non-root host
//! identity: then each harness and Bash run is started as that identity
//! (supplementary groups, then gid, then uid, all real/effective/saved)
//! by its work PID 1 just before it changes directory and execs, while
//! root PID 1 and the work PID 1s stay as they were started. Nothing sets
//! `no_new_privs`, so the identity keeps its host setuid/sudo capability.
//! A drop that fails is reported as the work's exec error; nothing runs.
//! Later
//! owners connect to `pid1-<incarnation>.sock` in the store directory and
//! must first send `{"op":"hello","token","generation"}`.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;

use oulipoly_root_supervisor::sys;
use serde_json::{Value, json};

const INITIAL_CHANNEL: RawFd = 3;

struct Work {
    id: String,
    /// Pid in this PID namespace.
    pid: i32,
    /// Owner-side ends, held so that the owner's death closes neither.
    stdin: OwnedFd,
    stdout: OwnedFd,
    /// Write end of the per-work PID 1's control pipe.
    control: OwnedFd,
    receipt: Option<OwnedFd>,
    pending: Vec<u8>,
    harness_host_pid: Option<i32>,
    /// `[st_dev, st_ino]` of the work's own PID namespace, as its per-work
    /// PID 1 (that namespace's init) read it before starting the harness.
    pidns: Value,
    harness: Option<String>,
    /// Whether this work's PID 1 was asked to stop its namespace, and its
    /// report that nothing is left in it (`drained`): separate facts from
    /// the harness's own wait status.
    stop_requested: bool,
    drained: Option<Value>,
}

/// The host identity work is started as (see the module docs).
#[derive(Clone)]
struct WorkIdentity {
    uid: libc::uid_t,
    gid: libc::gid_t,
    groups: Vec<libc::gid_t>,
}

impl WorkIdentity {
    /// `Ok(None)` for `null`; a malformed or root identity is refused.
    fn parse(value: &Value) -> Result<Option<Self>, ()> {
        if value.is_null() {
            return Ok(None);
        }
        let id = |value: &Value| value.as_u64().and_then(|id| u32::try_from(id).ok());
        let uid = id(&value["uid"]).ok_or(())?;
        let gid = id(&value["gid"]).ok_or(())?;
        let groups = value["groups"]
            .as_array()
            .ok_or(())?
            .iter()
            .map(|group| id(group).ok_or(()))
            .collect::<Result<Vec<_>, ()>>()?;
        if uid == 0 {
            return Err(());
        }
        Ok(Some(Self { uid, gid, groups }))
    }
}

struct Owner {
    socket: OwnedFd,
    generation: i64,
    attested: bool,
}

struct Pid1 {
    store: OwnedFd,
    incarnation: i64,
    token: String,
    max_generation: i64,
    signals: OwnedFd,
    listener: OwnedFd,
    owners: Vec<Owner>,
    works: Vec<Work>,
    receipts: Vec<Value>,
    others_reaped: u64,
    workload: Option<WorkIdentity>,
}

fn main() {
    let signals = block_sigchld().unwrap_or_else(|error| die("signalfd", &error));
    // SAFETY: descriptor 3 was set up by the owner for this process alone.
    let channel = unsafe { OwnedFd::from_raw_fd(INITIAL_CHANNEL) };
    set_cloexec(channel.as_raw_fd());
    let init = match sys::recv(&channel) {
        Ok(Some((bytes, _))) => serde_json::from_slice::<Value>(&bytes).ok(),
        _ => None,
    };
    let Some(init) = init.filter(|init| init["op"] == "init") else {
        // The starting owner died before initializing: nothing to hold.
        std::process::exit(0);
    };
    let (Some(store), Some(incarnation), Some(token), Some(generation)) = (
        init["store"].as_str(),
        init["incarnation"].as_i64(),
        init["token"].as_str(),
        init["generation"].as_i64(),
    ) else {
        let _ = reply(
            &channel,
            &json!({ "event": "refused", "reason": "bad-init" }),
            &[],
        );
        std::process::exit(64);
    };
    let Ok(workload) = WorkIdentity::parse(&init["workload"]) else {
        let _ = reply(
            &channel,
            &json!({ "event": "refused", "reason": "bad-init-workload" }),
            &[],
        );
        std::process::exit(64);
    };
    let store = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_PATH)
        .open(store)
        .map(OwnedFd::from)
        .unwrap_or_else(|error| die("store", &error));
    let socket_path = sys::path_in(&store, &format!("pid1-{incarnation}.sock"));
    let _ = std::fs::remove_file(&socket_path);
    let listener = sys::listen(&socket_path).unwrap_or_else(|error| die("listen", &error));
    let mut pid1 = Pid1 {
        store,
        incarnation,
        token: token.to_owned(),
        max_generation: generation,
        signals,
        listener,
        owners: vec![Owner {
            socket: channel,
            generation,
            attested: true,
        }],
        works: Vec::new(),
        receipts: Vec::new(),
        others_reaped: 0,
        workload,
    };
    let _ = reply(
        &pid1.owners[0].socket,
        &json!({
            "event": "ready",
            "pid_in_namespace": std::process::id(),
            "workload_uid": pid1.workload.as_ref().map(|identity| identity.uid),
        }),
        &[],
    );
    pid1.serve();
}

fn die(what: &str, error: &io::Error) -> ! {
    eprintln!("oulipoly-root-pid1: {what}: {error}");
    std::process::exit(70);
}

fn set_cloexec(fd: RawFd) {
    // SAFETY: fcntl on a descriptor this process owns.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
}

fn block_sigchld() -> io::Result<OwnedFd> {
    // SAFETY: sigset manipulation on a local set, then signalfd on it.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        libc::sigaddset(&raw mut set, libc::SIGCHLD);
        if libc::sigprocmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = libc::signalfd(-1, &raw const set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnedFd::from_raw_fd(fd))
    }
}

fn drain_signalfd(fd: &OwnedFd) {
    let mut buffer = [0u8; 128];
    // SAFETY: reading into a local buffer from a non-blocking fd we own.
    while unsafe { libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) } > 0 {}
}

fn reply(socket: &OwnedFd, value: &Value, fds: &[RawFd]) -> io::Result<()> {
    sys::send(socket, value.to_string().as_bytes(), fds)
}

/// Non-blocking reap of one child, if any has exited.
fn reap_one() -> Option<(i32, i32)> {
    let mut status = 0;
    // SAFETY: waitpid writes the status of one of our own children.
    let pid = unsafe { libc::waitpid(-1, &raw mut status, libc::WNOHANG) };
    (pid > 0).then_some((pid, status))
}

impl Pid1 {
    fn serve(&mut self) -> ! {
        loop {
            if self.works.is_empty() && !self.owners.iter().any(|owner| owner.attested) {
                self.finish("no-attached-owner-and-no-live-work");
            }
            let mut fds = vec![self.signals.as_raw_fd(), self.listener.as_raw_fd()];
            fds.extend(self.owners.iter().map(|owner| owner.socket.as_raw_fd()));
            let receipt_base = fds.len();
            let receipt_works: Vec<usize> = (0..self.works.len())
                .filter(|&index| self.works[index].receipt.is_some())
                .collect();
            fds.extend(receipt_works.iter().map(|&index| {
                self.works[index]
                    .receipt
                    .as_ref()
                    .map_or(-1, AsRawFd::as_raw_fd)
            }));
            let mut polls: Vec<libc::pollfd> = fds
                .iter()
                .map(|&fd| libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();
            // SAFETY: poll over a vector of valid pollfds.
            let ready = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, -1) };
            if ready < 0 {
                continue;
            }
            for (slot, &index) in receipt_works.iter().enumerate() {
                if polls[receipt_base + slot].revents != 0 {
                    self.read_receipt(index);
                }
            }
            let owner_ready: Vec<RawFd> = polls[2..receipt_base]
                .iter()
                .filter(|poll| poll.revents != 0)
                .map(|poll| poll.fd)
                .collect();
            for fd in owner_ready {
                if let Some(index) = self
                    .owners
                    .iter()
                    .position(|owner| owner.socket.as_raw_fd() == fd)
                {
                    self.owner_message(index);
                }
            }
            if polls[1].revents != 0
                && let Ok(socket) = sys::accept(&self.listener)
            {
                self.owners.push(Owner {
                    socket,
                    generation: 0,
                    attested: false,
                });
            }
            if polls[0].revents != 0 {
                drain_signalfd(&self.signals);
                while let Some((pid, status)) = reap_one() {
                    self.reaped(pid, status);
                }
            }
        }
    }

    fn broadcast(&self, value: &Value) {
        for owner in self.owners.iter().filter(|owner| owner.attested) {
            let _ = reply(&owner.socket, value, &[]);
        }
    }

    fn owner_message(&mut self, index: usize) {
        let message = match sys::recv(&self.owners[index].socket) {
            Ok(Some((bytes, _))) => serde_json::from_slice::<Value>(&bytes).ok(),
            _ => {
                // The owner's end closed: it died or detached. Its work stays.
                self.owners.remove(index);
                return;
            }
        };
        let Some(message) = message else {
            self.refuse(index, "malformed");
            return;
        };
        if !self.owners[index].attested {
            self.hello(index, &message);
            return;
        }
        let req = message["req"].clone();
        match message["op"].as_str() {
            Some("spawn") => self.spawn(index, &message),
            Some("kill") => {
                let id = message["work"].as_str().unwrap_or_default();
                let sent = self
                    .works
                    .iter()
                    .find(|work| work.id == id)
                    .is_some_and(|work| {
                        // SAFETY: one byte to the control pipe we hold.
                        unsafe {
                            libc::write(work.control.as_raw_fd(), b"k".as_ptr().cast(), 1) == 1
                        }
                    });
                let _ = reply(
                    &self.owners[index].socket,
                    &json!({ "req": req, "event": "kill-requested", "work": id, "sent": sent }),
                    &[],
                );
            }
            Some("release") => {
                if self.works.is_empty() {
                    let _ = reply(
                        &self.owners[index].socket,
                        &json!({ "req": req, "event": "releasing" }),
                        &[],
                    );
                    self.finish("released-by-owner");
                }
                let live: Vec<&str> = self.works.iter().map(|work| work.id.as_str()).collect();
                let _ = reply(
                    &self.owners[index].socket,
                    &json!({ "req": req, "event": "refused", "reason": "works-live", "live": live }),
                    &[],
                );
            }
            _ => {
                let _ = reply(
                    &self.owners[index].socket,
                    &json!({ "req": req, "event": "refused", "reason": "unknown-op" }),
                    &[],
                );
            }
        }
    }

    fn refuse(&mut self, index: usize, reason: &str) {
        let owner = self.owners.remove(index);
        let _ = reply(
            &owner.socket,
            &json!({ "event": "refused", "reason": reason }),
            &[],
        );
    }

    /// Admits a connecting owner only by positive attribution: the same
    /// user as this PID 1, the incarnation's token from the root's private
    /// store, and a generation newer than any already admitted.
    fn hello(&mut self, index: usize, message: &Value) {
        if message["op"] != "hello" {
            self.refuse(index, "peer-unattributed");
            return;
        }
        // SAFETY: geteuid has no preconditions.
        let euid = unsafe { libc::geteuid() };
        if sys::peer_cred(&self.owners[index].socket)
            .ok()
            .map(|cred| cred.uid)
            != Some(euid)
        {
            self.refuse(index, "peer-unattributed");
            return;
        }
        if message["token"].as_str() != Some(self.token.as_str()) {
            self.refuse(index, "peer-unattributed");
            return;
        }
        let Some(generation) = message["generation"].as_i64() else {
            self.refuse(index, "peer-unattributed");
            return;
        };
        if generation <= self.max_generation {
            self.refuse(index, "stale-generation");
            return;
        }
        self.max_generation = generation;
        let mut owner = self.owners.remove(index);
        owner.attested = true;
        owner.generation = generation;
        // Every earlier owner is superseded: it may no longer act here.
        for old in self.owners.iter().filter(|old| old.attested) {
            let _ = reply(
                &old.socket,
                &json!({ "event": "superseded", "by": generation }),
                &[],
            );
        }
        self.owners.retain(|old| !old.attested);
        let works: Vec<Value> = self
            .works
            .iter()
            .map(|work| json!({ "work": work.id, "harness_host_pid": work.harness_host_pid, "pidns": work.pidns, "harness": work.harness }))
            .collect();
        let _ = reply(
            &owner.socket,
            &json!({ "event": "attached", "generation": generation, "works": works, "receipts": self.receipts,
                     "workload_uid": self.workload.as_ref().map(|identity| identity.uid) }),
            &[],
        );
        for work in &self.works {
            let _ = reply(
                &owner.socket,
                &json!({ "event": "work-fds", "work": work.id }),
                &[work.stdin.as_raw_fd(), work.stdout.as_raw_fd()],
            );
        }
        let _ = reply(&owner.socket, &json!({ "event": "attach-complete" }), &[]);
        self.owners.push(owner);
    }

    fn spawn(&mut self, index: usize, message: &Value) {
        let req = message["req"].clone();
        let id = message["work"].as_str().unwrap_or_default().to_owned();
        let argv: Vec<String> = message["argv"]
            .as_array()
            .map(|argv| {
                argv.iter()
                    .filter_map(|arg| arg.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let launch = Launch {
            argv,
            env: message["env"]
                .as_object()
                .map(|env| {
                    env.iter()
                        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
            cwd: message["cwd"].as_str().map(str::to_owned),
            merge_stderr: message["stderr"] == "stdout",
            null_stdin: message["stdin"] == "null",
            identity: self.workload.clone(),
        };
        let refuse = |pid1: &Self, reason: String, not_started: bool| {
            let _ = reply(
                &pid1.owners[index].socket,
                &json!({ "req": req, "event": "refused", "reason": reason, "not_started": not_started }),
                &[],
            );
        };
        if self.works.iter().any(|work| work.id == id) {
            // The previous work can already have effects, despite no new spawn.
            refuse(self, "work-already-exists".to_owned(), false);
            return;
        }
        if id.is_empty()
            || launch.argv.is_empty()
            || launch
                .env
                .iter()
                .any(|(key, _)| key.is_empty() || key.contains('='))
        {
            refuse(self, "bad-spawn".to_owned(), true);
            return;
        }
        let started = start_work(&launch);
        let Ok((mut work, first)) = started else {
            refuse(
                self,
                format!(
                    "spawn-failed: {}",
                    started.err().map(|e| e.to_string()).unwrap_or_default()
                ),
                // start_work can fail reading the first report after clone.
                // Absence of a reply is not evidence of absence of effects.
                false,
            );
            return;
        };
        work.id = id.clone();
        work.harness_host_pid = first["harness_host_pid"]
            .as_i64()
            .and_then(|pid| i32::try_from(pid).ok());
        work.pidns = first["pidns"].clone();
        let _ = reply(
            &self.owners[index].socket,
            &json!({
                "req": req,
                "event": "spawned",
                "work": id,
                "harness_host_pid": work.harness_host_pid,
                "pidns": work.pidns,
                "exec_error": first["exec_error"],
            }),
            &[work.stdin.as_raw_fd(), work.stdout.as_raw_fd()],
        );
        self.works.push(work);
    }

    fn read_receipt(&mut self, index: usize) {
        let work = &mut self.works[index];
        let Some(fd) = work.receipt.as_ref() else {
            return;
        };
        let mut buffer = [0u8; 4096];
        // SAFETY: read into a local buffer from a pipe we own.
        let read = unsafe { libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if read <= 0 {
            work.receipt = None;
        } else {
            work.pending.extend_from_slice(&buffer[..read as usize]);
        }
        parse_lines(work);
    }

    /// Exact wait of one of this PID 1's children. A per-work PID 1's own
    /// status comes from this wait; its harness's status only from the
    /// per-work PID 1's report of its own wait.
    fn reaped(&mut self, pid: i32, status: i32) {
        let Some(index) = self.works.iter().position(|work| work.pid == pid) else {
            self.others_reaped += 1;
            return;
        };
        let mut work = self.works.remove(index);
        if let Some(fd) = work.receipt.take() {
            // The per-work PID 1 exited, so its write end is closed.
            let mut rest = Vec::new();
            let _ = File::from(fd).read_to_end(&mut rest);
            work.pending.extend_from_slice(&rest);
        }
        parse_lines(&mut work);
        let receipt = json!({
            "event": "receipt",
            "work": work.id,
            "harness_host_pid": work.harness_host_pid,
            "harness": work.harness,
            "harness_observer": work.harness.as_ref().map(|_| "work-pid1-wait"),
            "work_pid1": sys::describe_status(status),
            "work_pid1_observer": "root-pid1-wait",
            // Reported by the work's PID 1 before it exited: its namespace
            // had no process left (`null`: no such report).
            "namespace": work.drained,
            "stop_requested": work.stop_requested,
        });
        self.broadcast(&receipt);
        self.receipts.push(receipt);
    }

    /// Records what this PID 1 observed where a later owner can read it,
    /// then exits. Its own exit status is visible only to its parent.
    fn finish(&self, why: &str) -> ! {
        let record = json!({
            "incarnation": self.incarnation,
            "exit_reason": why,
            "receipts": self.receipts,
            "others_reaped": self.others_reaped,
        });
        let name = format!("pid1-{}.receipts", self.incarnation);
        let part = sys::path_in(&self.store, &format!("{name}.part"));
        let written = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&part)
            .and_then(|mut file| {
                file.write_all(record.to_string().as_bytes())?;
                file.sync_all()
            })
            .and_then(|()| std::fs::rename(&part, sys::path_in(&self.store, &name)))
            .and_then(|()| {
                File::open(sys::path_in(&self.store, ".")).and_then(|dir| dir.sync_all())
            });
        if let Err(error) = written {
            eprintln!("oulipoly-root-pid1: receipts not recorded: {error}");
        }
        let _ = std::fs::remove_file(sys::path_in(
            &self.store,
            &format!("pid1-{}.sock", self.incarnation),
        ));
        std::process::exit(0);
    }
}

fn parse_lines(work: &mut Work) {
    while let Some(end) = work.pending.iter().position(|&byte| byte == b'\n') {
        let line: Vec<u8> = work.pending.drain(..=end).collect();
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if let Some(status) = value["harness"].as_str() {
            work.harness = Some(status.to_owned());
        }
        if value["stop"] == "namespace-kill" {
            work.stop_requested = true;
        }
        if value["drained"] == true {
            work.drained = Some(json!({
                "drained": true,
                "others_reaped": value["others_reaped"],
                "stop": value["stop"],
                "stop_kills": value["stop_kills"],
            }));
        }
    }
}

/// How to start one harness: its program and arguments, environment
/// entries added to (or replacing) root PID 1's own, its working
/// directory, whether its stderr joins its stdout (otherwise it is root
/// PID 1's own stderr), and whether its stdin is `/dev/null` (otherwise the
/// pipe whose write end root PID 1 holds for the owner).
struct Launch {
    argv: Vec<String>,
    env: Vec<(String, String)>,
    cwd: Option<String>,
    merge_stderr: bool,
    null_stdin: bool,
    /// Who the started process runs as; `None`: as this PID 1.
    identity: Option<WorkIdentity>,
}

/// Starts one per-work PID 1 in a new PID namespace and returns its record
/// and its first report (the harness's host pid, or an exec error).
fn start_work(launch: &Launch) -> io::Result<(Work, Value)> {
    let (stdin_r, stdin_w) = sys::pipe()?;
    let (stdout_r, stdout_w) = sys::pipe()?;
    let (control_r, control_w) = sys::pipe()?;
    let (receipt_r, receipt_w) = sys::pipe()?;
    // SAFETY: fork-like clone of this single-threaded process.
    let pid = unsafe {
        libc::syscall(
            libc::SYS_clone,
            libc::c_long::from(libc::CLONE_NEWPID | libc::SIGCHLD),
            0usize,
            0usize,
            0usize,
            0usize,
        )
    };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        let keep = [
            stdin_r.as_raw_fd(),
            stdout_w.as_raw_fd(),
            control_r.as_raw_fd(),
            receipt_w.as_raw_fd(),
        ];
        close_all_except(&keep);
        work_pid1(launch, &stdin_r, &stdout_w, &control_r, receipt_w);
    }
    let pid = i32::try_from(pid).map_err(|_| io::Error::other("pid out of range"))?;
    drop((stdin_r, stdout_w, control_r, receipt_w));
    let mut first = Vec::new();
    let mut byte = [0u8; 1];
    let mut reader = File::from(receipt_r);
    while reader.read(&mut byte)? == 1 && byte[0] != b'\n' {
        first.push(byte[0]);
    }
    let first: Value = serde_json::from_slice(&first).unwrap_or(Value::Null);
    Ok((
        Work {
            id: String::new(),
            pid,
            stdin: stdin_w,
            stdout: stdout_r,
            control: control_w,
            receipt: Some(OwnedFd::from(reader)),
            pending: Vec::new(),
            harness_host_pid: None,
            pidns: Value::Null,
            harness: None,
            stop_requested: false,
            drained: None,
        },
        first,
    ))
}

/// In a freshly cloned single-threaded child: closes every descriptor
/// above stderr except `keep`.
fn close_all_except(keep: &[RawFd]) {
    let open: Vec<RawFd> = std::fs::read_dir("/proc/self/fd")
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let keep: HashSet<RawFd> = keep.iter().copied().collect();
    for fd in open {
        if fd > 2 && !keep.contains(&fd) {
            // SAFETY: closing an inherited descriptor in this new process.
            unsafe { libc::close(fd) };
        }
    }
}

fn write_line(fd: &OwnedFd, value: &Value) {
    let mut line = value.to_string();
    line.push('\n');
    // SAFETY: write to a pipe we own; a short write only loses the report.
    unsafe { libc::write(fd.as_raw_fd(), line.as_ptr().cast(), line.len()) };
}

/// SIGKILL to every process in this PID namespace except this init (the
/// caller is always a work's PID 1). Whether any process was signalled;
/// `ESRCH` (none left) is not an error.
fn kill_namespace() -> bool {
    // SAFETY: kill(2) with pid -1 from a PID namespace's init signals the
    // processes visible in that namespace (and those nested in it) only.
    unsafe { libc::kill(-1, libc::SIGKILL) == 0 }
}

/// PID 1 of one work's PID namespace: the actual parent of its harness.
/// Reports the harness's host pid, then the harness's own wait status,
/// then reaps any other process left in the namespace before exiting.
fn work_pid1(
    launch: &Launch,
    stdin: &OwnedFd,
    stdout: &OwnedFd,
    control: &OwnedFd,
    receipt: OwnedFd,
) -> ! {
    use std::os::unix::ffi::OsStrExt;

    let signals = block_sigchld().unwrap_or_else(|error| die("work signalfd", &error));
    let args: Vec<std::ffi::CString> = launch
        .argv
        .iter()
        .filter_map(|arg| std::ffi::CString::new(arg.as_bytes()).ok())
        .collect();
    let mut pointers: Vec<*const libc::c_char> = args.iter().map(|arg| arg.as_ptr()).collect();
    pointers.push(std::ptr::null());
    let inherited = std::env::vars_os().filter(|(key, _)| {
        !launch
            .env
            .iter()
            .any(|(added, _)| added.as_bytes() == key.as_bytes())
    });
    let environment: Vec<std::ffi::CString> = inherited
        .map(|(key, value)| [key.as_bytes(), b"=", value.as_bytes()].concat())
        .chain(
            launch
                .env
                .iter()
                .map(|(key, value)| format!("{key}={value}").into_bytes()),
        )
        .filter_map(|entry| std::ffi::CString::new(entry).ok())
        .collect();
    let mut env_pointers: Vec<*const libc::c_char> =
        environment.iter().map(|entry| entry.as_ptr()).collect();
    env_pointers.push(std::ptr::null());
    let cwd = launch
        .cwd
        .as_ref()
        .and_then(|cwd| std::ffi::CString::new(cwd.as_bytes()).ok());
    let merge_stderr = launch.merge_stderr;
    let null_stdin = launch.null_stdin;
    let identity = launch.identity.clone();
    // This process is init of the work's new PID namespace, so its own
    // namespace is the work's: every process of the work is in it or below.
    let pidns = std::fs::metadata("/proc/self/ns/pid")
        .map(|ns| {
            use std::os::unix::fs::MetadataExt;
            json!([ns.dev(), ns.ino()])
        })
        .unwrap_or(Value::Null);
    let (error_r, error_w) = sys::pipe().unwrap_or_else(|error| die("work pipe", &error));
    // SAFETY: fork of this single-threaded process.
    let harness = unsafe { libc::fork() };
    if harness == 0 {
        // SAFETY: in the harness child: set up stdio, then exec or exit.
        unsafe {
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&raw mut empty);
            libc::sigprocmask(libc::SIG_SETMASK, &raw const empty, std::ptr::null_mut());
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            if null_stdin {
                let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
                libc::dup2(null, 0);
            } else {
                libc::dup2(stdin.as_raw_fd(), 0);
            }
            libc::dup2(stdout.as_raw_fd(), 1);
            if merge_stderr {
                libc::dup2(stdout.as_raw_fd(), 2);
            }
            libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            );
            // The work identity, before anything of the work's own runs.
            // A failure is reported negated, apart from an exec error.
            if let Some(identity) = &identity
                && (libc::setgroups(identity.groups.len(), identity.groups.as_ptr()) != 0
                    || libc::setresgid(identity.gid, identity.gid, identity.gid) != 0
                    || libc::setresuid(identity.uid, identity.uid, identity.uid) != 0)
            {
                let errno = -*libc::__errno_location();
                libc::write(error_w.as_raw_fd(), (&raw const errno).cast(), 4);
                libc::_exit(127);
            }
            if cwd
                .as_ref()
                .is_none_or(|cwd| libc::chdir(cwd.as_ptr()) == 0)
            {
                libc::execvpe(pointers[0], pointers.as_ptr(), env_pointers.as_ptr());
            }
            let errno = *libc::__errno_location();
            libc::write(error_w.as_raw_fd(), (&raw const errno).cast(), 4);
            libc::_exit(127);
        }
    }
    drop(error_w);
    // SAFETY: closing the harness-side ends in this process.
    unsafe {
        libc::close(stdin.as_raw_fd());
        libc::close(stdout.as_raw_fd());
    }
    let mut errno = [0u8; 4];
    let exec_error = (File::from(error_r).read(&mut errno).unwrap_or(0) == 4).then(|| {
        let errno = i32::from_ne_bytes(errno);
        if errno < 0 {
            format!(
                "workload-identity: {}",
                io::Error::from_raw_os_error(-errno)
            )
        } else {
            io::Error::from_raw_os_error(errno).to_string()
        }
    });
    let pidfd = (harness > 0)
        .then(|| sys::pidfd_open(harness).ok())
        .flatten();
    let host_pid = pidfd.as_ref().and_then(|fd| sys::pidfd_host_pid(fd).ok());
    write_line(
        &receipt,
        &json!({ "harness_host_pid": host_pid, "pidns": pidns, "exec_error": exec_error }),
    );
    if harness <= 0 {
        write_line(&receipt, &json!({ "drained": true, "others_reaped": 0 }));
        std::process::exit(0);
    }
    let mut harness_done = false;
    let mut others = 0u64;
    let mut control_open = true;
    // A stop request ends the whole work, not only its leader: once asked,
    // this PID 1 sends SIGKILL to every process of its own PID namespace
    // (`kill(-1)` from a namespace's init reaches exactly the processes
    // visible in that namespace, nested ones included, never itself or
    // anything outside), and again after each reap until none is left.
    let mut stopping = false;
    let mut stop_kills = 0u64;
    loop {
        let mut polls = [
            libc::pollfd {
                fd: signals.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if control_open {
                    control.as_raw_fd()
                } else {
                    -1
                },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: poll over two valid (or ignored, -1) pollfds.
        if unsafe { libc::poll(polls.as_mut_ptr(), 2, -1) } < 0 {
            continue;
        }
        if polls[1].revents != 0 {
            let mut byte = [0u8; 1];
            // SAFETY: one byte from the control pipe we own.
            let read = unsafe { libc::read(control.as_raw_fd(), byte.as_mut_ptr().cast(), 1) };
            if read <= 0 {
                control_open = false;
            } else if byte[0] == b'k' {
                if !stopping {
                    stopping = true;
                    // Recorded before the signal: a stop was asked for,
                    // which is not yet any process's end.
                    write_line(&receipt, &json!({ "stop": "namespace-kill" }));
                }
                if !harness_done && let Some(fd) = pidfd.as_ref() {
                    // The harness is unreaped, so the pidfd still names it.
                    let _ = sys::pidfd_kill(fd);
                }
                stop_kills += u64::from(kill_namespace());
            }
        }
        if polls[0].revents != 0 {
            drain_signalfd(&signals);
            loop {
                let mut status = 0;
                // SAFETY: waitpid on our own children.
                let pid = unsafe { libc::waitpid(-1, &raw mut status, libc::WNOHANG) };
                if pid > 0 && pid == harness {
                    harness_done = true;
                    write_line(
                        &receipt,
                        &json!({ "harness": sys::describe_status(status) }),
                    );
                } else if pid > 0 {
                    others += 1;
                } else if pid < 0 && harness_done {
                    // ECHILD: nothing left in this namespace.
                    write_line(
                        &receipt,
                        &json!({
                            "drained": true,
                            "others_reaped": others,
                            "stop": if stopping { "namespace-kill" } else { "none" },
                            "stop_kills": stop_kills,
                        }),
                    );
                    std::process::exit(0);
                } else {
                    break;
                }
            }
            if stopping {
                // A process forked while the earlier signal was delivered,
                // or reparented here since, is signalled too.
                stop_kills += u64::from(kill_namespace());
            }
        }
    }
}
