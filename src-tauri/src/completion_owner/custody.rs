//! A gated, independently reaping per-operation custodian. Logical acceptance,
//! result integration and actual child-tree drain remain different observations.
use oulipoly_state::completion_continuation::{
    AdmittedSourceBinding, MAX_REGISTRATION_BYTES, open_source_file, read_source_file, sha256,
};
use oulipoly_state::mailbox::{ContinuationAttempt, MailboxDb};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

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
            .arg(CUSTODIAN_ARG)
            .arg(gate_fd.to_string())
            .arg(request_fd.to_string())
            .exec();
        unsafe { libc::_exit(70) }
    }
    drop(gate);
    drop(request_file);
    let identity = super::linux::identity(i64::from(pid))?;
    let mut mailbox = MailboxDb::open(path)?;
    mailbox.attach_continuation_custodian(attempt, &identity)?;
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
    let file = unsafe { std::fs::File::from_raw_fd(request_fd) };
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("custodian request too large".into());
    }
    let request: CustodianRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
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
    loop {
        // Keep the wait result through DB/result integration failures;
        // never repeat a wait for an already reaped root.
        if let Some(process) = child.as_mut()
            && let Some(status) = process.try_wait().map_err(|e| e.to_string())?
        {
            root_status = status.code();
            root_wait_status = Some(status.into_raw());
            child = None;
        }
        if child.is_none() && owned_tree_empty()? {
            break;
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
    let receipt=serde_json::json!({"attempt_id":attempt.attempt_id,"custodian":identity,"root_exit_code":root_status,"root_wait_status":root_wait_status,"spawn_failed":spawn_failed,"spawn_error":spawn_error,"response":classification,"owned_children":"ECHILD","result_retained":true}).to_string();
    // Persist the complete wait/drain result before its DB integration.
    persist_result_until_retained(Path::new(&attempt.result_path), receipt.as_bytes());
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
        Some("source_ready") => {
            let source = binding.registration()?;
            let directory = Path::new(&source.handle_dir);
            let snapshot =
                read_source_file(directory, &source.snapshot_relative, 16 * 1024 * 1024)?;
            let outcome =
                read_source_file(directory, &source.outcome_relative, MAX_REGISTRATION_BYTES)?;
            let evidence = oulipoly_state::completion_continuation::VerifiedCompletion::from_bytes(
                &binding, &snapshot, &outcome,
            )?;
            if value["snapshot_sha256"] != evidence.snapshot_sha256
                || value["outcome_sha256"] != evidence.outcome_sha256
            {
                return Err("source reply evidence hash conflict".into());
            }
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
