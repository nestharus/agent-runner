//! Original Runner actor's private PTY custody for the selected interactive K.
//! The broker receives a duplicate master only for its exact challenge; this
//! actor retains the original descriptor and listener until its provider call
//! returns. This module does not create a runtime generation or deliver F.

use oulipoly_kernel_broker::protocol::{self, FreshPlanRole, PrivateFreshPtyHandoff};
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
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
    resident: Arc<
        Mutex<Option<oulipoly_runtime::executor::cli::pty_broker::PtyControlGenerationIdentity>>,
    >,
    // Keep the selected context alive with the descriptor. The broker checks
    // the same fields against its durable h/f decision on each ^ submission.
    _binding: PrivateFreshPtyHandoff,
}

pub(super) struct RunningInteractive {
    live_reader: JoinHandle<Result<Vec<u8>, String>>,
    grant: String,
    k_error: Option<String>,
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
            let resident = Arc::new(Mutex::new(None));
            let server_resident = resident.clone();
            let server = std::thread::Builder::new()
                .name("root-pty-control".into())
                .spawn(move || {
                    serve(
                        listener,
                        held_master,
                        server_alive,
                        server_stop,
                        server_resident,
                    )
                })
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
                resident,
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
        let running = self.start_interactive(broker, plan_source)?;
        self.finish_interactive(broker, running, input)
    }

    pub(super) fn start_interactive(
        &mut self,
        broker: &Path,
        plan_source: [i32; 5],
    ) -> Result<RunningInteractive, String> {
        self.ensure_live()?;
        self.rechallenge(broker)?;
        let slave = self.slave.as_ref().ok_or("root PTY slave released")?;
        let mut output_master = self.master.try_clone().map_err(|e| e.to_string())?;
        let (_relay_rx, relay_tx) = UnixStream::pair().map_err(|e| e.to_string())?;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let live_reader = std::thread::Builder::new()
            .name("root-interactive-pty-output".into())
            .spawn(move || -> Result<Vec<u8>, String> {
                let mut bytes = Vec::new();
                let mut announced = false;
                let mut buffer = [0u8; 8192];
                loop {
                    let n = match output_master.read(&mut buffer) {
                        Ok(n) => n,
                        Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => return Err(error.to_string()),
                    };
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
        let grant = first
            .split_whitespace()
            .nth(1)
            .ok_or("interactive K grant absent")?
            .to_owned();
        Ok(RunningInteractive {
            live_reader,
            grant,
            k_error,
        })
    }

    pub(super) fn register_resident(&self, broker: &Path) -> Result<serde_json::Value, String> {
        self.ensure_live()?;
        let value = protocol::private_fresh_interactive_resident_at(
            broker,
            &self._binding,
            self.master.as_raw_fd(),
        )
        .map_err(|e| format!("interactive resident registration refused: {e}"))?;
        let record = &value["resident"]["registration"];
        let observer = oulipoly_state::pid_identity::procfs_observer_domain()?;
        let creator: oulipoly_state::pid_identity::ProcessIdentity =
            serde_json::from_value(record["creator"].clone()).map_err(|e| e.to_string())?;
        let provider: oulipoly_state::pid_identity::ProcessIdentity =
            serde_json::from_value(record["provider"].clone()).map_err(|e| e.to_string())?;
        if record["observer_domain"] != observer
            || record["session_id"] != self._binding.session_id
            || record["account"] != self._binding.account
            || record["control_path"] != self.path.display().to_string()
            || value["generation"]["generation_id"] != record["grant_id"]
            || value["generation"]["spawn_invocation_uuid"] != record["invocation_uuid"]
            || value["generation"]["runtime_mode"] != "pty_interactive"
            || value["generation"]["lifecycle_state"] != "running"
            || oulipoly_state::pid_identity::read_current_process_identity()? != creator
            || oulipoly_state::pid_identity::read_live_process_identity(provider.os_pid)?
                != Some(provider.clone())
        {
            return Err("interactive resident observer or generation changed".into());
        }
        let identity = oulipoly_runtime::executor::cli::pty_broker::PtyControlGenerationIdentity {
            challenge: String::new(),
            generation_id: record["grant_id"]
                .as_str()
                .ok_or("resident grant absent")?
                .into(),
            spawn_invocation_uuid: record["invocation_uuid"]
                .as_str()
                .ok_or("resident invocation absent")?
                .into(),
            creator_process: creator,
            provider_process: provider,
            provider_account: record["account"]
                .as_str()
                .ok_or("resident account absent")?
                .into(),
            provider_instance_id: String::new(),
            settings_id: String::new(),
            provider_session_id: record["session_id"]
                .as_str()
                .ok_or("resident session absent")?
                .into(),
        };
        *self.resident.lock().map_err(|e| e.to_string())? = Some(identity);
        self.ensure_live()?;
        Ok(value)
    }

    pub(super) fn set_selected_adapter(
        &self,
        instance: &str,
        settings: &str,
    ) -> Result<(), String> {
        if instance.is_empty() || settings.is_empty() {
            return Err("resident selected adapter identity absent".into());
        }
        let mut identity = self.resident.lock().map_err(|e| e.to_string())?;
        let value = identity
            .as_mut()
            .ok_or("resident generation not registered")?;
        value.provider_instance_id = instance.into();
        value.settings_id = settings.into();
        Ok(())
    }

    pub(super) fn challenge_resident(
        &self,
    ) -> Result<oulipoly_runtime::executor::cli::pty_broker::PtyControlGenerationIdentity, String>
    {
        self.ensure_live()?;
        let expected = self
            .resident
            .lock()
            .map_err(|e| e.to_string())?
            .clone()
            .ok_or("resident identity absent")?;
        let read = oulipoly_runtime::executor::cli::pty_broker::query_pty_generation_identity(
            &self.path,
            self.device,
            self.inode,
        )?;
        let mut expected = expected;
        expected.challenge = read.challenge.clone();
        if read != expected {
            return Err("resident live socket challenge changed".into());
        }
        Ok(read)
    }

    pub(super) fn broker_resident_readback(
        &self,
        broker: &Path,
    ) -> Result<serde_json::Value, String> {
        self.ensure_live()?;
        let read = protocol::private_fresh_interactive_resident_readback_at(
            broker,
            &self._binding,
            self.master.as_raw_fd(),
        )
        .map_err(|e| format!("broker resident challenged readback refused: {e}"))?;
        let challenged = self.challenge_resident()?;
        if read["socket"]["generation_id"] != challenged.generation_id
            || read["socket"]["creator_process"]
                != serde_json::to_value(&challenged.creator_process).map_err(|e| e.to_string())?
            || read["socket"]["provider_process"]
                != serde_json::to_value(&challenged.provider_process).map_err(|e| e.to_string())?
            || read["socket"]["provider_instance_id"] != challenged.provider_instance_id
            || read["socket"]["settings_id"] != challenged.settings_id
            || read["generation"]["generation_id"] != challenged.generation_id
        {
            return Err("broker and original-root resident socket readbacks differ".into());
        }
        Ok(read)
    }

    pub(super) fn probe_resident_refusal(
        &self,
        broker: &Path,
        case: &str,
    ) -> Result<String, String> {
        let mut request = self._binding.clone();
        let _replacement = match case {
            "absent_socket" | "replaced_socket" => {
                fs::remove_file(&self.path).map_err(|e| e.to_string())?;
                if case == "replaced_socket" {
                    Some(UnixListener::bind(&self.path).map_err(|e| e.to_string())?)
                } else {
                    None
                }
            }
            "wrong_account" => {
                request.account = "wrong-account".into();
                None
            }
            "wrong_session" => {
                request.session_id = uuid::Uuid::new_v4().to_string();
                None
            }
            "stale_provider" => None,
            _ => return Err("invalid resident refusal case".into()),
        };
        let result = protocol::private_fresh_interactive_resident_at(
            broker,
            &request,
            self.master.as_raw_fd(),
        );
        match result {
            Ok(_) => Err(format!("resident {case} unexpectedly registered")),
            Err(error) => Ok(error.to_string()),
        }
    }

    pub(super) fn finish_interactive(
        &mut self,
        broker: &Path,
        running: RunningInteractive,
        input: &[u8],
    ) -> Result<(String, Vec<u8>), String> {
        let RunningInteractive {
            live_reader,
            grant,
            k_error,
        } = running;
        if self.resident.lock().map_err(|e| e.to_string())?.is_some() {
            self.broker_resident_readback(broker)?;
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
        while !live_reader.is_finished() {
            if std::time::Instant::now() >= deadline {
                return Err(format!("interactive PTY drain unknown after K {grant}"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let streamed = live_reader
            .join()
            .map_err(|_| "interactive live PTY reader panicked".to_string())??;
        let mut transcript = tempfile::tempfile().map_err(|e| e.to_string())?;
        transcript.write_all(&streamed).map_err(|e| e.to_string())?;
        transcript.sync_all().map_err(|e| e.to_string())?;
        use sha2::Digest as _;
        let source_sha256 = format!("{:x}", sha2::Sha256::digest(&streamed));
        transcript.rewind().map_err(|e| e.to_string())?;
        if super::private_verified_output(
            transcript.try_clone().map_err(|e| e.to_string())?,
            streamed.len() as u64,
            &source_sha256,
        )? != streamed
        {
            return Err("interactive original-root transcript changed before finalization".into());
        }
        if std::env::var_os("AGE319_PRIVATE_ROOT_PTY_FINALIZER_WRONG_PAIR_V1").is_some() {
            let (wrong_master, _wrong_slave) = open_pty().map_err(|e| e.to_string())?;
            if protocol::private_fresh_interactive_finalize_at(
                broker,
                &self._binding,
                wrong_master.as_raw_fd(),
                transcript.as_raw_fd(),
            )
            .is_ok()
            {
                return Err("interactive finalizer accepted a replaced master".into());
            }
            let wrong_control = PrivateFreshPtyHandoff {
                control_path: self.path.with_file_name("wrong-control.sock"),
                ..self._binding.clone()
            };
            if protocol::private_fresh_interactive_finalize_at(
                broker,
                &wrong_control,
                self.master.as_raw_fd(),
                transcript.as_raw_fd(),
            )
            .is_ok()
            {
                return Err("interactive finalizer accepted a replaced control socket".into());
            }
            let state = protocol::private_fresh_interactive_q_at(broker, &self._binding)
                .map_err(|e| e.to_string())?;
            if !state.starts_with(&format!("fresh-interactive-unknown-or-pending {grant}")) {
                return Err(format!("wrong finalizer pair published Q: {state}"));
            }
        }
        loop {
            self.ensure_live()
                .map_err(|e| format!("interactive original control unknown after K: {e}"))?;
            let finalized = protocol::private_fresh_interactive_finalize_at(
                broker,
                &self._binding,
                self.master.as_raw_fd(),
                transcript.as_raw_fd(),
            );
            let state = match protocol::private_fresh_interactive_q_at(broker, &self._binding) {
                Ok(state) => state,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                Err(error) => {
                    return Err(format!(
                        "interactive Q unknown after K: {error}; finalize: {finalized:?}"
                    ));
                }
            };
            if state.starts_with("fresh-interactive-drained ") {
                if !matches!(
                    std::env::var("AGE319_PRIVATE_RESIDENT_FAILURE_V1").as_deref(),
                    Ok("absent_socket" | "replaced_socket")
                ) {
                    self.ensure_live()?;
                }
                let output = protocol::private_fresh_interactive_output_at(broker, &self._binding)
                    .map_err(|e| format!("interactive output transfer failed after Q: {e}"))?;
                let fields: Vec<_> = state.split_whitespace().collect();
                if fields.len() != 5
                    || fields[1] != grant
                    || fields[1] != output.grant_id
                    || fields[2] != output.wait_status.to_string()
                    || fields[3] != output.bytes.to_string()
                    || fields[4] != output.sha256
                {
                    return Err("interactive Q/output transfer differs from exact K".into());
                }
                let bytes =
                    super::private_verified_output(output.output, output.bytes, &output.sha256)?;
                if streamed != bytes {
                    return Err("interactive original-root PTY transcript differs from Q".into());
                }
                return Ok((state, bytes));
            }
            if !state.starts_with("fresh-interactive-unknown-or-pending ")
                || std::time::Instant::now() >= deadline
            {
                return Err(format!(
                    "interactive Q unknown after K: {state}; finalize: {finalized:?}; K reply: {k_error:?}"
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
        let not_tty = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .map_err(|e| e.to_string())?;
        if protocol::private_fresh_pty_handoff_at(
            broker,
            &self._binding,
            self.master.as_raw_fd(),
            not_tty.as_raw_fd(),
        )
        .is_ok()
        {
            return Err("non-TTY slave accepted for interactive handoff".into());
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

fn serve(
    listener: UnixListener,
    master: File,
    alive: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    resident: Arc<
        Mutex<Option<oulipoly_runtime::executor::cli::pty_broker::PtyControlGenerationIdentity>>,
    >,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                let mut first = [0u8; 4];
                if stream.read_exact(&mut first).is_ok() {
                    if &first == b"OPTY" {
                        let _ = answer_resident_identity(&mut stream, &resident);
                    } else {
                        let mut challenge = [0u8; 16];
                        challenge[..4].copy_from_slice(&first);
                        if stream.read_exact(&mut challenge[4..]).is_ok() {
                            let _ = send_master(&stream, &challenge, master.as_raw_fd());
                        }
                    }
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

fn answer_resident_identity(
    stream: &mut UnixStream,
    resident: &Mutex<
        Option<oulipoly_runtime::executor::cli::pty_broker::PtyControlGenerationIdentity>,
    >,
) -> io::Result<()> {
    let mut rest = [0u8; 8];
    stream.read_exact(&mut rest)?;
    let len = u32::from_be_bytes(rest[4..8].try_into().unwrap()) as usize;
    let answer = if rest[0] != 1 || rest[1] != 3 || rest[2..4] != [0, 0] || len != 36 {
        Err("resident identity request invalid".to_owned())
    } else {
        let mut challenge = vec![0u8; len];
        stream.read_exact(&mut challenge)?;
        let challenge = std::str::from_utf8(&challenge).map_err(io::Error::other)?;
        if uuid::Uuid::parse_str(challenge).is_err() {
            Err("resident challenge invalid".to_owned())
        } else {
            let held = resident
                .lock()
                .map_err(|_| io::Error::other("resident identity lock poisoned"))?;
            let mut identity = held.as_ref().ok_or("resident generation absent").cloned();
            if let Ok(value) = &mut identity {
                if value.provider_instance_id.is_empty()
                    || value.settings_id.is_empty()
                    || oulipoly_state::pid_identity::read_current_process_identity()
                        .ok()
                        .as_ref()
                        != Some(&value.creator_process)
                    || oulipoly_state::pid_identity::read_live_process_identity(
                        value.provider_process.os_pid,
                    )
                    .ok()
                    .flatten()
                    .as_ref()
                        != Some(&value.provider_process)
                {
                    identity = Err("resident process or selected adapter changed");
                } else {
                    value.challenge = challenge.to_owned();
                }
            }
            identity
                .and_then(|value| {
                    serde_json::to_string(&value).map_err(|_| "resident JSON invalid")
                })
                .map_err(str::to_owned)
        }
    };
    let (ack, body) = match answer {
        Ok(value) => (true, value),
        Err(error) => (false, error),
    };
    let mut header = [0u8; 12];
    header[..4].copy_from_slice(b"OPTY");
    header[4] = 1;
    header[5] = if ack { 0 } else { 1 };
    header[8..12].copy_from_slice(&(body.len() as u32).to_be_bytes());
    stream.write_all(&header)?;
    stream.write_all(body.as_bytes())
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
