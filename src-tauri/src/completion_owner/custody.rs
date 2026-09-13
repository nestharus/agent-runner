//! A gated, independently reaping per-operation custodian. Logical acceptance,
//! result integration and actual child-tree drain remain different observations.
use oulipoly_state::completion_continuation::{
    AdmittedSourceBinding, MAX_REGISTRATION_BYTES, open_source_file, read_source_file, sha256,
};
use oulipoly_state::mailbox::{ContinuationAttempt, MailboxDb};
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

pub(super) const ADOPTER_ARG: &str = "__completion-continuation-adopter-v2";

pub(super) const CUSTODIAN_ARG: &str = "__completion-continuation-custodian-v2";

#[derive(serde::Serialize, serde::Deserialize)]
enum LaunchRecipe {
    Source(AdmittedSourceBinding),
    Native {
        args: Vec<Vec<u8>>,
        environment: Vec<(Vec<u8>, Option<Vec<u8>>)>,
        directory: Option<Vec<u8>>,
    },
}
#[derive(serde::Serialize, serde::Deserialize)]
struct CustodianRequest {
    path: std::path::PathBuf,
    attempt: ContinuationAttempt,
    recipe: LaunchRecipe,
}

pub(super) fn spawn_source(
    path: &Path,
    attempt: &ContinuationAttempt,
    binding: &AdmittedSourceBinding,
) -> Result<i64, String> {
    spawn(path, attempt, LaunchRecipe::Source(binding.clone()))
}

pub(super) fn spawn_activation<F>(
    path: &Path,
    attempt: &ContinuationAttempt,
    command: F,
) -> Result<i64, String>
where
    F: FnOnce() -> Result<Command, String>,
{
    use std::os::unix::ffi::OsStrExt;
    let command = command()?;
    let recipe = LaunchRecipe::Native {
        args: command.get_args().map(|v| v.as_bytes().to_vec()).collect(),
        environment: command
            .get_envs()
            .map(|(k, v)| (k.as_bytes().to_vec(), v.map(|v| v.as_bytes().to_vec())))
            .collect(),
        directory: command
            .get_current_dir()
            .map(|v| v.as_os_str().as_bytes().to_vec()),
    };
    spawn(path, attempt, recipe)
}

fn launch_command(
    recipe: &LaunchRecipe,
    attempt: &ContinuationAttempt,
) -> Result<std::process::Child, String> {
    use std::os::unix::ffi::OsStringExt;
    let mut command = match recipe {
        LaunchRecipe::Source(binding) => source_command(binding, attempt)?,
        LaunchRecipe::Native {
            args,
            environment,
            directory,
        } => {
            // The custodian re-executed the exact current runner image. Native
            // launch uses that image, never an installed-path lookup.
            let mut command = Command::new("/proc/self/exe");
            command.args(args.iter().cloned().map(std::ffi::OsString::from_vec));
            for (key, value) in environment {
                let key = std::ffi::OsString::from_vec(key.clone());
                if let Some(value) = value {
                    command.env(key, std::ffi::OsString::from_vec(value.clone()));
                } else {
                    command.env_remove(key);
                }
            }
            if let Some(directory) = directory {
                command.current_dir(std::ffi::OsString::from_vec(directory.clone()));
            }
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            command
        }
    };
    let directory = Path::new(&attempt.result_path)
        .parent()
        .ok_or("attempt result has no parent")?;
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let (out, err) = if matches!(recipe, LaunchRecipe::Source(_)) {
        ("stdout.json", "stderr.log")
    } else {
        ("launcher.stdout", "launcher.stderr")
    };
    command
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(directory.join(out)).map_err(|e| e.to_string())?)
        .stderr(std::fs::File::create(directory.join(err)).map_err(|e| e.to_string())?);
    command.spawn().map_err(|e| e.to_string())
}

fn spawn(path: &Path, attempt: &ContinuationAttempt, recipe: LaunchRecipe) -> Result<i64, String> {
    // This retained request is intent only. Database acceptance still precedes
    // fork and the launch gate; an unadmitted request cannot launch anything.
    let request = CustodianRequest {
        path: path.into(),
        attempt: attempt.clone(),
        recipe,
    };
    let request_path = Path::new(&attempt.result_path).with_file_name("custodian-request.json");
    durable_write(
        &request_path,
        &serde_json::to_vec(&request).map_err(|e| e.to_string())?,
    )?;
    let request_file = std::fs::File::open(&request_path).map_err(|e| e.to_string())?;
    MailboxDb::open(path)?.accept_continuation_attempt(attempt)?;
    let (mut release, gate) = match UnixStream::pair() {
        Ok(pair) => pair,
        Err(e) => {
            let reason = format!("custodian gate creation failed: {e}");
            MailboxDb::open(path)?.record_continuation_never_forked(attempt, &reason)?;
            return Err(reason);
        }
    };
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let reason = format!("custodian fork failed: {}", std::io::Error::last_os_error());
        MailboxDb::open(path)?.record_continuation_never_forked(attempt, &reason)?;
        return Err(reason);
    }
    if pid == 0 {
        drop(release);
        let gate_fd = gate.as_raw_fd();
        let request_fd = request_file.as_raw_fd();
        super::linux::close_except(&[gate_fd, request_fd]);
        for fd in [gate_fd, request_fd] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
            {
                unsafe { libc::_exit(70) }
            }
        }
        // No SQLite operation occurs in this fork image. Inherited SQLite
        // process-global WAL bookkeeping is discarded by exec, not reused.
        let _error = Command::new("/proc/self/exe")
            .arg(ADOPTER_ARG)
            .arg(gate_fd.to_string())
            .arg(request_fd.to_string())
            .exec();
        unsafe { libc::_exit(70) }
    }
    drop(gate);
    drop(request_file);
    let adopter = super::linux::identity(i64::from(pid))?;
    let mut worker = [0; 4];
    release.read_exact(&mut worker).map_err(|e| e.to_string())?;
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier("attempt-before-attachment");
    let identity = super::linux::identity(i64::from(i32::from_ne_bytes(worker)))?;
    let mut mailbox = MailboxDb::open(path)?;
    mailbox.attach_continuation_custodian_with_adopter(attempt, &identity, Some(&adopter))?;
    mailbox.advance_continuation_attempt(attempt, 3, "accepted", "starting", &identity)?;
    release.write_all(&[1]).map_err(|e| e.to_string())?;
    Ok(i64::from(pid))
}

pub(super) fn entry() -> Result<(), String> {
    use std::os::fd::FromRawFd;
    let fd = |index| -> Result<i32, String> {
        std::env::args()
            .nth(index)
            .ok_or("missing custodian descriptor")?
            .parse()
            .map_err(|_| "invalid custodian descriptor".into())
    };
    let gate_fd = fd(2)?;
    let request_fd = fd(3)?;
    if gate_fd < 3 || request_fd < 3 || gate_fd == request_fd {
        return Err("invalid custodian descriptor identity".into());
    }
    let mut gate = unsafe { UnixStream::from_raw_fd(gate_fd) };
    let mut file = unsafe { std::fs::File::from_raw_fd(request_fd) };
    let mut bytes = Vec::new();
    (&mut file)
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("custodian request too large".into());
    }
    let request: CustodianRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if std::env::args().nth(1).as_deref() == Some(ADOPTER_ARG) {
        file.rewind().map_err(|e| e.to_string())?;
        return adopt(request, gate, file);
    }
    drop(file);
    let path = &request.path;
    let attempt = &request.attempt;
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let identity = super::linux::identity(i64::from(std::process::id()))?;
    let mut byte = [0];
    if gate.read_exact(&mut byte).is_err() || byte != [1] {
        let receipt=serde_json::json!({"attempt_id":attempt.attempt_id,"custodian":identity,"gate":"unreleased_eof","owned_children":"ECHILD"}).to_string();
        if owned_tree_empty()? {
            persist_result_until_retained(Path::new(&attempt.result_path), receipt.as_bytes());
            loop {
                if MailboxDb::open(path)
                    .and_then(|mut db| {
                        db.cancel_unreleased_continuation_gate(attempt, &identity, &receipt)
                    })
                    .is_ok()
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        return Ok(());
    }
    drop(gate);
    let launch = MailboxDb::open(path)
        .and_then(|db| db.require_continuation_launch_gate(attempt, &identity))
        .and_then(|()| launch_command(&request.recipe, attempt));
    let (mut child, spawn_error) = match launch {
        Ok(child) => (Some(child), None),
        Err(error) => (None, Some(error)),
    };
    let spawn_failed = child.is_none();
    let mut root_status = None;
    let mut root_wait_status = None;
    let mut cancellation: Option<(String, std::time::Instant)> = None;
    let observations = activation_observer(path, attempt);
    loop {
        if cancellation.is_none()
            && let Some(identity) =
                latest_activation_observation(&observations).and_then(|v| v.cancellation)
        {
            cancellation = Some((identity, std::time::Instant::now()));
        }
        if let Some((_, started)) = &cancellation {
            let signal =
                if started.elapsed() >= oulipoly_core::launch_custody::TERMINATION_GRACE_PERIOD {
                    libc::SIGKILL
                } else {
                    libc::SIGTERM
                };
            // All direct children belong to this exact activation AC. The
            // original Bash source tree is never in this custody boundary.
            let _ = oulipoly_core::launch_custody::signal_owned_children(signal);
        }
        // Persist each waitable incarnation before consuming it. A surviving
        // original adopter can complete the same record after AC loss.
        let (waited, status, _) = reap_adopted_child(Some((attempt, &identity)))?;
        if child.as_ref().is_some_and(|p| p.id() as i32 == waited) {
            let exit = std::process::ExitStatus::from_raw(status);
            if cancellation.is_none()
                && attempt.operation == "activation"
                && matches!(exit.signal(), Some(libc::SIGTERM | libc::SIGINT))
            {
                cancellation = Some((
                    format!("native_launcher_wait_signal:{}", exit.signal().unwrap()),
                    std::time::Instant::now(),
                ));
            }
            root_status = exit.code();
            root_wait_status = Some(status);
            child = None;
        }
        if waited < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                if child.is_some() {
                    return Err("original launcher wait missing".into());
                }
                break;
            }
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.to_string());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let classification = if attempt.operation == "source_recovery" && !spawn_failed {
        match classify_source_reply(attempt) {
            Ok(value) => value,
            Err(error) => {
                serde_json::json!({"classification":"uncertain_response","error":error})
            }
        }
    } else {
        serde_json::json!({"classification":"native_wait_result"})
    };
    let receipt=serde_json::json!({"attempt_id":attempt.attempt_id,"custodian":identity,"root_exit_code":root_status,"root_wait_status":root_wait_status,"spawn_failed":spawn_failed,"spawn_error":spawn_error,"accepted_cancellation":cancellation.as_ref().map(|v|&v.0),"response":classification,"owned_children":"ECHILD","result_retained":true}).to_string();
    // Persist the complete wait/drain result before its DB integration.
    persist_result_until_retained(Path::new(&attempt.result_path), receipt.as_bytes());
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier("ac-result-retained");
    loop {
        let result = MailboxDb::open(path).and_then(|mut mailbox| {
            if spawn_failed {
                mailbox.cancel_unreleased_continuation_gate(attempt, &identity, &receipt)
            } else {
                mailbox.discharge_continuation_attempt(attempt, &identity, &receipt)
            }
        });
        match result {
            Ok(()) => break,
            Err(error) => {
                // Persist uncertainty separately; a retained physical
                // receipt is not a successful database integration.
                let _ = durable_write(
                    &Path::new(&attempt.result_path).with_file_name("integration-error.txt"),
                    error.as_bytes(),
                );
            }
        }
        // No children remain, but retained result integration is still
        // owed. There is no provider/workload silence deadline.
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn source_command(
    binding: &AdmittedSourceBinding,
    attempt: &ContinuationAttempt,
) -> Result<Command, String> {
    let source = binding.registration()?;
    let directory = Path::new(&source.handle_dir);
    let registration = read_source_file(
        directory,
        &source.registration_relative,
        MAX_REGISTRATION_BYTES,
    )?;
    if registration != binding.registration_bytes() {
        return Err("recovery source incarnation conflict".into());
    }
    let environment = read_source_file(
        directory,
        "delivery-helper-environment.json",
        MAX_REGISTRATION_BYTES,
    )?;
    if sha256(&environment) != source.recovery.environment_sha256 {
        return Err("recovery environment digest conflict".into());
    }
    let environment: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&environment).map_err(|e| e.to_string())?;
    let relative = Path::new(&source.recovery.path)
        .strip_prefix(directory)
        .map_err(|e| e.to_string())?
        .to_str()
        .ok_or("non UTF-8 recovery path")?;
    let mut executable = open_source_file(directory, relative, 256 * 1024 * 1024)?;
    let metadata = executable.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 * 1024 {
        return Err("recovery image is not a bounded regular file".into());
    }
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    let mut buffer = [0; 65536];
    loop {
        let count = executable.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    if format!("{:x}", hasher.finalize()) != source.recovery.sha256 {
        return Err("recovery executable digest conflict".into());
    }
    let confirmation =
        Path::new(&attempt.result_path).with_file_name("registration-confirmation-v2.json");
    let mut receipt = crate::commands::notify_continuation::response(binding, "exact_committed")?;
    receipt["authority"] = "completion_only".into();
    receipt["registration_committed"] = true.into();
    durable_write(
        &confirmation,
        serde_json::to_string(&receipt)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )?;
    let descriptor = executable.as_raw_fd();
    // Command owns the pinned descriptor via pre_exec closure through exec.
    let mut command = Command::new(format!("/proc/self/fd/{descriptor}"));
    command
        .env_clear()
        .envs(environment)
        .arg("completion-reconcile-v2")
        .arg("--registration-file")
        .arg(directory.join(&source.registration_relative))
        .arg("--confirmation")
        .arg(confirmation)
        .arg("--json");
    unsafe {
        command.pre_exec(move || {
            let fd = executable.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command)
}

pub(super) fn owned_tree_empty() -> Result<bool, String> {
    loop {
        let pid = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
        if pid > 0 {
            continue;
        }
        if pid == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ECHILD) {
            return Ok(true);
        }
        if error.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error.to_string());
    }
}

pub(super) fn durable_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("durable result has no directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| e.to_string())?;
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

fn classify_source_reply(attempt: &ContinuationAttempt) -> Result<serde_json::Value, String> {
    let directory = Path::new(&attempt.result_path)
        .parent()
        .ok_or("attempt result directory absent")?;
    let bytes = read_source_file(directory, "stdout.json", MAX_REGISTRATION_BYTES)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let state = oulipoly_state::StateDb::open_read_only(&oulipoly_state::StateDb::default_path()?)
        .map_err(|e| format!("{e:?}"))?;
    let binding = state
        .admitted_completion_continuations()?
        .into_iter()
        .find(|binding| {
            binding.registration().ok().is_some_and(|source| {
                Some(source.registration_id) == attempt.source_registration_id
            })
        })
        .ok_or("source reply has no retained admission")?;
    let expected = serde_json::to_value(binding.identity()?).map_err(|e| e.to_string())?;
    for (key, expected) in expected.as_object().ok_or("invalid source identity")? {
        if value.get(key) != Some(expected) {
            return Err(format!("source reply identity conflict at {key}"));
        }
    }
    match value["status"].as_str() {
        Some("source_ready" | "source_output_missing") => {
            let evidence =
                oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                    &binding,
                )?;
            evidence.validate_source_reply(&value)?;
        }
        Some("pending" | "conflict" | "unavailable") => {}
        _ => return Err("source reply missing/unsupported status".into()),
    }
    Ok(
        serde_json::json!({"classification":"structured_source_response","stdout_sha256":sha256(&bytes),"stdout_byte_len":bytes.len(),"reply":value}),
    )
}

fn persist_result_until_retained(path: &Path, bytes: &[u8]) {
    while durable_write(path, bytes).is_err() {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[derive(Default)]
struct ActivationObservation {
    launcher: Option<oulipoly_state::completion_continuation::SourceProcessIdentity>,
    cancellation: Option<String>,
}

/// Database work cannot block physical reaping or lose a wait result. The
/// observer owns ordinary writer connections in a separate thread, not snapshot
/// helper children intermingled with the custodian's wait set. One latest-value
/// slot bounds transport memory; persisted cancellation is queried until read.
fn activation_observer(
    path: &Path,
    attempt: &ContinuationAttempt,
) -> Option<std::sync::mpsc::Receiver<ActivationObservation>> {
    if attempt.operation != "activation" {
        return None;
    }
    let path = path.to_path_buf();
    let attempt = attempt.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || observe_activation_database(path, attempt, sender));
    Some(receiver)
}

fn observe_activation_database(
    path: std::path::PathBuf,
    attempt: ContinuationAttempt,
    sender: std::sync::mpsc::SyncSender<ActivationObservation>,
) {
    loop {
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier("activation-observation");
        let observation = read_activation_observation(&path, &attempt).unwrap_or_default();
        if matches!(
            sender.try_send(observation),
            Err(std::sync::mpsc::TrySendError::Disconnected(_))
        ) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn read_activation_observation(
    path: &Path,
    attempt: &ContinuationAttempt,
) -> Result<ActivationObservation, String> {
    let mailbox = MailboxDb::open(path)?;
    let launcher = mailbox.continuation_launcher_identity(attempt)?;
    let Some((generation, invocation)) = mailbox.continuation_runtime_identity(attempt)? else {
        return Ok(ActivationObservation {
            launcher,
            cancellation: None,
        });
    };
    let cancellation = read_activation_cancellation(&generation, &invocation)
        .ok()
        .flatten();
    Ok(ActivationObservation {
        launcher,
        cancellation,
    })
}

fn read_activation_cancellation(
    generation: &str,
    invocation: &str,
) -> Result<Option<String>, String> {
    let state = oulipoly_state::StateDb::open_default()?;
    use rusqlite::OptionalExtension;
    state.connection().query_row("SELECT l.logical_launch_id || ':' || l.cancel_requested_at FROM provider_launch_attempts a JOIN provider_logical_launches l ON l.logical_launch_id=a.logical_launch_id WHERE a.runtime_generation_uuid=?1 AND a.invocation_uuid=?2 AND l.cancel_requested_at IS NOT NULL",rusqlite::params![generation,invocation],|r|r.get(0)).optional().map_err(|e|e.to_string())
}

fn latest_activation_observation(
    receiver: &Option<std::sync::mpsc::Receiver<ActivationObservation>>,
) -> Option<ActivationObservation> {
    receiver.as_ref()?.try_iter().last()
}

/// An original per-attempt adopting boundary exists before AC starts. This is
/// not a replacement's empty tree: every launched descendant stays beneath it
/// on AC loss, independently of domain guardian/driver replacement.
fn adopt(
    request: CustodianRequest,
    mut gate: UnixStream,
    file: std::fs::File,
) -> Result<(), String> {
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // Announcement and launch release have distinct peer lifetimes. AC must
    // not inherit the CD/adopter endpoint: otherwise adopter loss before its
    // PID announcement leaves CD waiting for PID and AC waiting for release,
    // each keeping the other's peer alive forever.
    let (mut ac_release, ac_gate) = UnixStream::pair().map_err(|e| e.to_string())?;
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier("adopter-before-ac-fork");
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        drop(gate);
        drop(ac_release);
        let gate_fd = ac_gate.as_raw_fd();
        let flags = unsafe { libc::fcntl(gate_fd, libc::F_GETFD) };
        if flags < 0
            || unsafe { libc::fcntl(gate_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
        {
            unsafe { libc::_exit(70) }
        }
        let _error = Command::new("/proc/self/exe")
            .arg(CUSTODIAN_ARG)
            .arg(gate_fd.to_string())
            .arg(file.as_raw_fd().to_string())
            .exec();
        unsafe { libc::_exit(70) }
    }
    drop(ac_gate);
    let custodian = super::linux::identity(i64::from(pid))?;
    let adopter = super::linux::identity(i64::from(std::process::id()))?;
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier("adopter-before-ac-announce");
    // Relay only the actual original driver's grant after attachment. EOF or
    // adopter death closes AC's separate gate; AC then retains its own exact
    // unreleased/ECHILD receipt. Neither a new owner nor a timeout grants work.
    let mut grant = [0];
    if gate.write_all(&pid.to_ne_bytes()).is_ok()
        && gate.read_exact(&mut grant).is_ok()
        && grant == [1]
    {
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier("adopter-before-ac-release");
        let _ = ac_release.write_all(&grant);
    }
    drop(ac_release);
    drop(gate);
    drop(file);
    let path = &request.path;
    let attempt = &request.attempt;
    let mut custodian_wait = None;
    let mut cancellation: Option<(String, std::time::Instant)> = None;
    let mut launcher = None;
    let observations = activation_observer(path, attempt);
    let mut signal_waits = Vec::new();
    loop {
        if let Some(observation) = latest_activation_observation(&observations) {
            launcher = observation.launcher.or(launcher);
            if cancellation.is_none() {
                cancellation = observation
                    .cancellation
                    .map(|id| (id, std::time::Instant::now()));
            }
        }
        if cancellation.is_none()
            && let Some(expected) = &launcher
            && let Some((_, status)) = signal_waits
                .iter()
                .find(|(identity, _)| identity == expected)
        {
            cancellation = Some((
                format!(
                    "adopted_native_launcher_wait_signal:{}",
                    libc::WTERMSIG(*status)
                ),
                std::time::Instant::now(),
            ));
        }
        if custodian_wait.is_some()
            && let Some((_, started)) = &cancellation
        {
            let signal =
                if started.elapsed() >= oulipoly_core::launch_custody::TERMINATION_GRACE_PERIOD {
                    libc::SIGKILL
                } else {
                    libc::SIGTERM
                };
            let _ = oulipoly_core::launch_custody::signal_owned_children(signal);
        }
        let (waited, status, wait_identity) = reap_adopted_child(Some((attempt, &adopter)))?;
        if waited == pid {
            custodian_wait = Some(status);
        }
        if let Some(identity) = wait_identity
            && libc::WIFSIGNALED(status)
            && matches!(libc::WTERMSIG(status), libc::SIGTERM | libc::SIGINT)
        {
            // Retain exact waits even when the DB identity/cancellation read is
            // unavailable at this instant. Later observation may join them.
            signal_waits.push((identity, status));
            #[cfg(feature = "age360-fault-fixtures")]
            oulipoly_state::completion_continuation::age360_fault_barrier("adopted-terminal-wait");
        }
        if waited < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                break;
            }
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.to_string());
            }
        }
        if waited <= 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    // Preserve the original receipt when AC already retained its actual result.
    if replay_result(path, attempt).is_ok() {
        return Ok(());
    }
    let receipt = serde_json::json!({"attempt_id":attempt.attempt_id,"custodian":custodian,"adopter":adopter,"custodian_wait_status":custodian_wait.ok_or("original custodian was not waited")?,"owned_children":"ECHILD","accepted_cancellation":cancellation.map(|v|v.0),"classification":"original_adopting_boundary_drained"}).to_string();
    let result_path = Path::new(&attempt.result_path).with_file_name("adopting-result.json");
    persist_result_until_retained(&result_path, receipt.as_bytes());
    loop {
        // An AC-integrated result wins over the later enclosing-boundary receipt.
        if !MailboxDb::open(path)?
            .pending_continuation_attempts()?
            .iter()
            .any(|v| v.attempt_id == attempt.attempt_id)
        {
            return Ok(());
        }
        if MailboxDb::open(path)
            .and_then(|mut db| db.discharge_adopted_continuation_attempt(attempt, &receipt))
            .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Inspect the waitable child before consuming its PID, preserving incarnation
/// identity even if the launcher binding cannot currently be read from SQLite.
fn reap_adopted_child(
    journal: Option<(
        &ContinuationAttempt,
        &oulipoly_state::completion_continuation::SourceProcessIdentity,
    )>,
) -> Result<
    (
        i32,
        i32,
        Option<oulipoly_state::completion_continuation::SourceProcessIdentity>,
    ),
    String,
> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_ALL,
            0,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result < 0 {
        return Ok((-1, 0, None));
    }
    let pid = unsafe { info.si_pid() };
    if pid == 0 {
        return Ok((0, 0, None));
    }
    let identity = super::linux::identity(i64::from(pid))?;
    if let Some((attempt, owner)) = journal {
        let status = if info.si_code == libc::CLD_EXITED {
            (unsafe { info.si_status() }) << 8
        } else {
            (unsafe { info.si_status() })
                | if info.si_code == libc::CLD_DUMPED {
                    128
                } else {
                    0
                }
        };
        let file = Path::new(&attempt.result_path)
            .with_file_name("owned-waits")
            .join(format!(
                "{}-{}-{}.json",
                identity.pid, identity.starttime_ticks, owner.pid
            ));
        // No aggregation at ECHILD. The receipt binds original owner, operation
        // boundary, incarnation and an actual WNOWAIT terminal observation.
        if durable_write(
            &file,
            &serde_json::to_vec(&serde_json::json!({
                "attempt_id": attempt.attempt_id, "owner": owner, "process": identity,
                "status": status, "observation": "waitid_wnowait"
            }))
            .map_err(|e| e.to_string())?,
        )
        .is_err()
        {
            // Preserve the waitable child and original owner on retention
            // failure. The caller continues TERM/KILL escalation each turn;
            // storage failure must not kill both original custody boundaries.
            return Ok((0, 0, None));
        }
    }
    let mut status = 0;
    let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    Ok((waited, status, Some(identity)))
}

/// Replay only retained wait/drain evidence joined by the DB to its original
/// producer. A lost process with no receipt is not a replayable result.
pub(super) fn replay_result(path: &Path, attempt: &ContinuationAttempt) -> Result<(), String> {
    let result = Path::new(&attempt.result_path);
    let directory = result.parent().ok_or("result parent absent")?;
    let primary = read_source_file(directory, "result.json", MAX_REGISTRATION_BYTES);
    if let Ok(bytes) = primary {
        let receipt = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if value["attempt_id"] != attempt.attempt_id || value["owned_children"] != "ECHILD" {
            return Err("retained result evidence conflict".into());
        }
        let custodian =
            serde_json::from_value(value["custodian"].clone()).map_err(|e| e.to_string())?;
        let mut db = MailboxDb::open(path)?;
        return if value["gate"] == "unreleased_eof" || value["spawn_failed"] == true {
            db.cancel_unreleased_continuation_gate(attempt, &custodian, receipt)
        } else if value["result_retained"] == true && value["root_wait_status"].as_i64().is_some() {
            db.discharge_continuation_attempt(attempt, &custodian, receipt)
        } else {
            Err("retained result lacks launch/wait evidence".into())
        };
    }
    let bytes = read_source_file(directory, "adopting-result.json", MAX_REGISTRATION_BYTES)?;
    MailboxDb::open(path)?.discharge_adopted_continuation_attempt(
        attempt,
        std::str::from_utf8(&bytes).map_err(|e| e.to_string())?,
    )
}
