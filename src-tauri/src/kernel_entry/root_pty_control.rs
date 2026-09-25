//! Original Runner actor's private PTY custody for the selected interactive K.
//! The broker receives a duplicate master only for its exact challenge; this
//! actor retains the original descriptor and listener until its provider call
//! returns. This module does not create a runtime generation or deliver F.

use oulipoly_kernel_broker::protocol::{self, FreshPlanRole, PrivateFreshPtyHandoff};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

pub(super) struct RootPtyControl {
    master: File,
    slave: Option<File>,
    path: PathBuf,
    device: u64,
    inode: u64,
    alive: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
    // Keep the selected context alive with the descriptor. The broker checks
    // the same fields against its durable h/f decision on each ^ submission.
    _binding: PrivateFreshPtyHandoff,
}

impl RootPtyControl {
    pub(super) fn offer(
        broker: &Path,
        d_key: &str,
        session_id: &str,
        account: &str,
        plan_sha256: &str,
        directory: &Path,
    ) -> Result<Self, String> {
        if !directory.is_absolute() || !directory.is_dir() {
            return Err("private PTY control directory invalid".into());
        }
        let (master, slave) = open_pty().map_err(|e| format!("root PTY open failed: {e}"))?;
        let path = directory.join(format!("root-pty-{}.sock", d_key));
        // An existing pathname is never unlinked or adopted. This also makes
        // a copied/sibling socket unable to masquerade as a fresh root offer.
        if fs::symlink_metadata(&path).is_ok() {
            return Err("root PTY control path already exists".into());
        }
        let listener =
            UnixListener::bind(&path).map_err(|e| format!("root PTY control bind failed: {e}"))?;
        let bound = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        let result = (|| {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
            listener.set_nonblocking(true).map_err(|e| e.to_string())?;
            let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if !metadata.file_type().is_socket() {
                return Err("root PTY control path changed during bind".into());
            }
            let binding = PrivateFreshPtyHandoff {
                d_key: d_key.into(),
                session_id: session_id.into(),
                role: FreshPlanRole::Interactive,
                account: account.into(),
                plan_sha256: plan_sha256.into(),
                control_path: path.clone(),
            };
            let alive = Arc::new(AtomicBool::new(true));
            let stop = Arc::new(AtomicBool::new(false));
            let held_master = master.try_clone().map_err(|e| e.to_string())?;
            let server_alive = alive.clone();
            let server_stop = stop.clone();
            let server = std::thread::Builder::new()
                .name("root-pty-control".into())
                .spawn(move || serve(listener, held_master, server_alive, server_stop))
                .map_err(|e| e.to_string())?;
            let control = Self {
                master,
                slave: Some(slave),
                path: path.clone(),
                device: metadata.dev(),
                inode: metadata.ino(),
                alive,
                stop,
                server: Some(server),
                _binding: binding,
            };
            control.ensure_live()?;
            control.rechallenge(broker)?;
            Ok(control)
        })();
        // The guard owns cleanup after successful construction. On an early
        // bind/setup failure no guard exists, so remove only our own socket.
        if result.is_err() {
            if let Ok(metadata) = fs::symlink_metadata(&path) {
                if metadata.file_type().is_socket()
                    && (metadata.dev(), metadata.ino()) == (bound.dev(), bound.ino())
                {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        result
    }

    pub(super) fn ensure_live(&self) -> Result<(), String> {
        if !self.alive.load(Ordering::Acquire)
            || self.server.as_ref().is_none_or(JoinHandle::is_finished)
        {
            return Err("root PTY control server lost".into());
        }
        let metadata = fs::symlink_metadata(&self.path)
            .map_err(|e| format!("root PTY control endpoint lost: {e}"))?;
        if !metadata.file_type().is_socket()
            || (metadata.dev(), metadata.ino()) != (self.device, self.inode)
        {
            return Err("root PTY control endpoint replaced".into());
        }
        let mut number = 0u32;
        if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCGPTN, &mut number) } != 0 {
            return Err("root PTY master custody lost".into());
        }
        Ok(())
    }

    pub(super) fn rechallenge(&self, broker: &Path) -> Result<(), String> {
        self.ensure_live()?;
        protocol::private_fresh_pty_handoff_at(
            broker,
            &self._binding,
            self.master.as_raw_fd(),
            self.slave
                .as_ref()
                .ok_or("root PTY slave released")?
                .as_raw_fd(),
        )
        .map_err(|e| format!("root PTY challenge refused: {e}"))?;
        self.ensure_live()
    }

    pub(super) fn prepare_interactive_k(
        &self,
        broker: &Path,
        plan_source: [i32; 5],
    ) -> Result<(), String> {
        self.ensure_live()?;
        protocol::private_fresh_interactive_k_preparation_at(
            broker,
            &self._binding,
            [
                plan_source[0],
                plan_source[1],
                plan_source[2],
                plan_source[3],
                plan_source[4],
                self.master.as_raw_fd(),
                self.slave
                    .as_ref()
                    .ok_or("root PTY slave released")?
                    .as_raw_fd(),
            ],
        )
        .map_err(|e| format!("interactive K preparation refused: {e}"))?;
        self.ensure_live()
    }

    pub(super) fn run_interactive(
        &mut self,
        broker: &Path,
        plan_source: [i32; 5],
        input: &[u8],
    ) -> Result<(String, Vec<u8>), String> {
        self.ensure_live()?;
        self.rechallenge(broker)?;
        let slave = self.slave.as_ref().ok_or("root PTY slave released")?;
        let (mut relay_rx, relay_tx) = UnixStream::pair().map_err(|e| e.to_string())?;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let live_reader = std::thread::Builder::new()
            .name("root-interactive-pty-output".into())
            .spawn(move || -> Result<Vec<u8>, String> {
                let mut bytes = Vec::new();
                let mut announced = false;
                let mut buffer = [0u8; 8192];
                loop {
                    let n = relay_rx.read(&mut buffer).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                    if !announced
                        && bytes
                            .windows(b"interactive-ready".len())
                            .any(|part| part == b"interactive-ready")
                    {
                        let _ = ready_tx.send(());
                        announced = true;
                    }
                }
                Ok(bytes)
            })
            .map_err(|e| e.to_string())?;
        let descriptors = [
            plan_source[0],
            plan_source[1],
            plan_source[2],
            plan_source[3],
            plan_source[4],
            self.master.as_raw_fd(),
            slave.as_raw_fd(),
            relay_tx.as_raw_fd(),
        ];
        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_PHYSICAL_NEGATIVE_V1").is_some() {
            for changed in [
                PrivateFreshPtyHandoff {
                    role: FreshPlanRole::Headless,
                    ..self._binding.clone()
                },
                PrivateFreshPtyHandoff {
                    plan_sha256: "0".repeat(64),
                    ..self._binding.clone()
                },
                PrivateFreshPtyHandoff {
                    account: "wrong-account".into(),
                    ..self._binding.clone()
                },
                PrivateFreshPtyHandoff {
                    control_path: self.path.with_file_name("wrong-control.sock"),
                    ..self._binding.clone()
                },
            ] {
                if protocol::private_fresh_interactive_k_at(broker, &changed, descriptors).is_ok() {
                    return Err(
                        "wrong interactive K plan, account, role or control accepted".into(),
                    );
                }
            }
            if protocol::private_fresh_interactive_q_at(broker, &self._binding)
                .map_err(|e| e.to_string())?
                != "fresh-interactive-k-absent\n"
            {
                return Err("wrong interactive K probe consumed grant".into());
            }
        }
        let submitted =
            protocol::private_fresh_interactive_k_at(broker, &self._binding, descriptors);
        drop(relay_tx);
        let mut k_error = None;
        let first = match submitted {
            Ok(reply) => reply,
            Err(error) => {
                k_error = Some(error.to_string());
                let readback = protocol::private_fresh_interactive_q_at(broker, &self._binding)
                    .map_err(|readback| {
                        format!("interactive K uncertain: {error}; readback: {readback}")
                    })?;
                if readback == "fresh-interactive-k-absent\n" {
                    return Err(format!("interactive K refused before consumption: {error}"));
                }
                readback
            }
        };
        if !first.starts_with("fresh-interactive-k ")
            && !first.starts_with("fresh-interactive-unknown-or-pending ")
            && !first.starts_with("fresh-interactive-drained ")
        {
            return Err(format!("interactive K readback invalid: {first}"));
        }
        ready_rx.recv_timeout(Duration::from_secs(5))
            .map_err(|e| format!("interactive prompt did not arrive on live PTY relay: {e}; K reply: {k_error:?}; same K readback: {first}"))?;
        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_RESTART_AFTER_K_V1").is_some() {
            let gate = PathBuf::from(
                std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                    .map_err(|e| e.to_string())?,
            );
            fs::write(gate.join("physical-k-ready"), b"ready").map_err(|e| e.to_string())?;
            let until = std::time::Instant::now() + Duration::from_secs(20);
            while !gate.join("physical-restarted").exists() {
                if std::time::Instant::now() >= until {
                    return Err("physical post-K restart gate expired".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let state = protocol::private_fresh_interactive_q_at(broker, &self._binding)
                .map_err(|e| format!("interactive post-K restart readback failed: {e}"))?;
            let grant = first
                .split_whitespace()
                .nth(1)
                .ok_or("interactive K grant absent")?;
            if !state.starts_with(&format!("fresh-interactive-unknown-or-pending {grant}\n")) {
                return Err(format!(
                    "interactive post-K restart changed original K: {state}"
                ));
            }
            return Err(format!("interactive post-K broker restart debt: {state}"));
        }
        self.master
            .try_clone()
            .map_err(|e| e.to_string())?
            .write_all(input)
            .map_err(|e| format!("interactive PTY input failed after K: {e}"))?;
        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_EXIT_AFTER_K_V1").is_some() {
            unsafe { libc::_exit(79) };
        }
        self.slave.take();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let state = protocol::private_fresh_interactive_q_at(broker, &self._binding)
                .map_err(|e| format!("interactive Q unknown after K: {e}"))?;
            if state.starts_with("fresh-interactive-drained ") {
                self.ensure_live()?;
                let output = protocol::private_fresh_interactive_output_at(broker, &self._binding)
                    .map_err(|e| format!("interactive output transfer failed after Q: {e}"))?;
                let fields: Vec<_> = state.split_whitespace().collect();
                if fields.len() != 5
                    || fields[1] != output.grant_id
                    || fields[2] != output.wait_status.to_string()
                    || fields[3] != output.bytes.to_string()
                    || fields[4] != output.sha256
                {
                    return Err("interactive Q/output transfer differs from exact K".into());
                }
                let bytes =
                    super::private_verified_output(output.output, output.bytes, &output.sha256)?;
                let streamed = live_reader
                    .join()
                    .map_err(|_| "interactive live PTY relay panicked".to_string())??;
                if streamed != bytes {
                    return Err("interactive live PTY relay differs from Q transcript".into());
                }
                return Ok((state, bytes));
            }
            if !state.starts_with("fresh-interactive-unknown-or-pending ")
                || std::time::Instant::now() >= deadline
            {
                return Err(format!(
                    "interactive Q unknown after K: {state}; K reply: {k_error:?}"
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub(super) fn probe_wrong_bindings(
        &self,
        broker: &Path,
        headless_sha256: &str,
    ) -> Result<(), String> {
        self.ensure_live()?;
        for changed in [
            PrivateFreshPtyHandoff {
                role: FreshPlanRole::Headless,
                ..self._binding.clone()
            },
            PrivateFreshPtyHandoff {
                plan_sha256: headless_sha256.into(),
                ..self._binding.clone()
            },
            PrivateFreshPtyHandoff {
                account: "wrong-account".into(),
                ..self._binding.clone()
            },
        ] {
            if protocol::private_fresh_pty_handoff_at(
                broker,
                &changed,
                self.master.as_raw_fd(),
                self.slave
                    .as_ref()
                    .ok_or("root PTY slave released")?
                    .as_raw_fd(),
            )
            .is_ok()
            {
                return Err("wrong PTY plan role, digest or account accepted".into());
            }
        }
        self.rechallenge(broker)
    }
}

impl Drop for RootPtyControl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake nonblocking accept promptly. The server always checks stop
        // before reading the incoming connection.
        let _ = UnixStream::connect(&self.path);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
        if fs::symlink_metadata(&self.path).ok().is_some_and(|m| {
            m.file_type().is_socket() && (m.dev(), m.ino()) == (self.device, self.inode)
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn open_pty() -> io::Result<(File, File)> {
    let mut master = -1;
    let mut slave = -1;
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let pair = (unsafe { File::from_raw_fd(master) }, unsafe {
        File::from_raw_fd(slave)
    });
    for file in [&pair.0, &pair.1] {
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(pair)
}

fn serve(listener: UnixListener, master: File, alive: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                let mut challenge = [0u8; 16];
                if stream.read_exact(&mut challenge).is_ok() {
                    let _ = send_master(&stream, &challenge, master.as_raw_fd());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
    alive.store(false, Ordering::Release);
}

fn send_master(stream: &UnixStream, challenge: &[u8; 16], master: i32) -> io::Result<()> {
    let mut iov = libc::iovec {
        iov_base: challenge.as_ptr().cast_mut().cast(),
        iov_len: challenge.len(),
    };
    let mut control = [0u8; 64];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = unsafe { libc::CMSG_SPACE(std::mem::size_of::<i32>() as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as _) as usize;
        *(libc::CMSG_DATA(header) as *mut i32) = master;
    }
    if unsafe { libc::sendmsg(stream.as_raw_fd(), &message, libc::MSG_NOSIGNAL) }
        != challenge.len() as isize
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
