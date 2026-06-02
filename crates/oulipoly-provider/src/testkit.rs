#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct FakeProvider {
    root: PathBuf,
    path: PathBuf,
    marker: PathBuf,
}

impl FakeProvider {
    pub fn compile(source: impl AsRef<Path>) -> Self {
        let root = unique_temp_dir("fake-provider");
        fs::create_dir_all(&root).expect("fake-provider temp dir should be created");
        let binary = binary_path(&root);
        let status = Command::new("rustc")
            .arg("--edition=2024")
            .arg(source.as_ref())
            .arg("-o")
            .arg(&binary)
            .status()
            .expect("rustc should run for fake-provider fixture");
        assert!(status.success(), "fake-provider fixture should compile");
        let marker = root.join("spawned.marker");
        let path = wrapper_path(&root, &binary, &marker);
        Self { root, path, marker }
    }

    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    pub fn is_executable(&self) -> bool {
        is_executable(&self.path)
    }

    pub fn cleanup(self) {
        let _ = fs::remove_dir_all(self.root);
    }

    pub fn was_spawned(&self) -> bool {
        self.marker.exists()
    }

    pub fn run(&self, mode: FakeProviderMode, subcommand: &str, stdin: &str) -> Output {
        self.run_with_env(mode, subcommand, stdin, std::iter::empty::<(&str, &str)>())
    }

    pub fn run_with_env<I, K, V>(
        &self,
        mode: FakeProviderMode,
        subcommand: &str,
        stdin: &str,
        envs: I,
    ) -> Output
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let env_vec = envs
            .into_iter()
            .map(|(key, value)| (key.as_ref().to_os_string(), value.as_ref().to_os_string()))
            .collect::<Vec<_>>();
        let mut child = retry_spawn(|| {
            let mut command = Command::new(&self.path);
            command
                .arg(subcommand)
                .envs(mode.env())
                .envs(env_vec.iter().map(|(key, value)| (key, value)))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        })
        .expect("fake-provider should spawn");
        {
            use std::io::Write;
            let mut child_stdin = child.stdin.take().expect("stdin should be piped");
            child_stdin
                .write_all(stdin.as_bytes())
                .expect("stdin should be written");
        }
        child
            .wait_with_output()
            .expect("fake-provider output should be collected")
    }

    pub fn spawn<I, K, V>(&self, envs: I) -> Child
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let env_vec = envs
            .into_iter()
            .map(|(key, value)| (key.as_ref().to_os_string(), value.as_ref().to_os_string()))
            .collect::<Vec<_>>();
        retry_spawn(|| {
            let mut command = Command::new(&self.path);
            command
                .arg("describe")
                .envs(env_vec.iter().map(|(key, value)| (key, value)))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            configure_test_process_group(&mut command);
            command
        })
        .expect("fake-provider child should spawn")
    }
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        true
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeProviderMode {
    RecordArgvStdin,
    StdinEof,
    Success,
    SuccessStderr,
    ProviderError,
    ProviderTimeoutError,
    ProviderErrorNonzero,
    ExitNonzeroNoEnvelope,
    SuccessThenNonzero,
    SchemaInvalidSuccess,
    InvalidUtf8,
    NonObjectArray,
    NonObjectString,
    NonObjectNumber,
    MissingOk,
    InvalidJson,
    EmptyStdout,
    MultipleJson,
    LeadingLog,
    TrailingJunk,
    StderrEnvelopeOnly,
    MismatchedContract,
    MismatchedRequestId,
    LargeStdoutStderr,
    PipePressure,
    Sleep,
    SleepWithCancellation,
    ChildGrandchild,
    SigtermResistantChildGrandchild,
    EarlyStdinSuccess,
    EarlyStdinError,
    EarlyStdinEmpty,
    LaunchValid,
    LaunchModelNonzero,
    LaunchProviderNonzeroAfterFinal,
    LaunchProviderNonzeroNoFinal,
    LaunchCancelledFinalEvent,
    LaunchMalformedLine,
    LaunchMalformedLineNonzero,
    LaunchMalformedLineStderr,
    LaunchBlankLine,
    LaunchExitThenLargeStdout,
    LaunchInvalidBase64,
    LaunchDuplicateExit,
    LaunchEventAfterExit,
    LaunchPartialHang,
}

impl FakeProviderMode {
    pub fn env(self) -> Vec<(String, String)> {
        vec![("FAKE_PROVIDER_MODE".to_owned(), self.as_str().to_owned())]
    }

    pub fn env_with_probe(self, probe: &LeakProbe) -> Vec<(String, String)> {
        let mut env = self.env();
        env.push((
            "FAKE_PROVIDER_PROBE_DIR".to_owned(),
            probe.root.display().to_string(),
        ));
        env
    }

    pub fn env_with_record(self, record: &Path) -> Vec<(String, OsString)> {
        if let Some(parent) = record.parent() {
            fs::create_dir_all(parent).expect("record directory should be created");
        }
        vec![
            (
                "FAKE_PROVIDER_MODE".to_owned(),
                OsString::from(self.as_str()),
            ),
            (
                "FAKE_PROVIDER_RECORD_PATH".to_owned(),
                record.as_os_str().to_os_string(),
            ),
        ]
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::RecordArgvStdin => "record-argv-stdin",
            Self::StdinEof => "stdin-eof",
            Self::Success => "success",
            Self::SuccessStderr => "success-stderr",
            Self::ProviderError => "provider-error",
            Self::ProviderTimeoutError => "provider-timeout-error",
            Self::ProviderErrorNonzero => "provider-error-nonzero",
            Self::ExitNonzeroNoEnvelope => "exit-nonzero-no-envelope",
            Self::SuccessThenNonzero => "success-then-nonzero",
            Self::SchemaInvalidSuccess => "schema-invalid-success",
            Self::InvalidUtf8 => "invalid-utf8",
            Self::NonObjectArray => "non-object-array",
            Self::NonObjectString => "non-object-string",
            Self::NonObjectNumber => "non-object-number",
            Self::MissingOk => "missing-ok",
            Self::InvalidJson => "invalid-json",
            Self::EmptyStdout => "empty-stdout",
            Self::MultipleJson => "multiple-json",
            Self::LeadingLog => "leading-log",
            Self::TrailingJunk => "trailing-junk",
            Self::StderrEnvelopeOnly => "stderr-envelope-only",
            Self::MismatchedContract => "mismatched-contract",
            Self::MismatchedRequestId => "mismatched-request-id",
            Self::LargeStdoutStderr => "large-stdout-stderr",
            Self::PipePressure => "pipe-pressure",
            Self::Sleep | Self::SleepWithCancellation => "sleep",
            Self::ChildGrandchild => "child-grandchild",
            Self::SigtermResistantChildGrandchild => "sigterm-resistant-child-grandchild",
            Self::EarlyStdinSuccess => "early-stdin-success",
            Self::EarlyStdinError => "early-stdin-error",
            Self::EarlyStdinEmpty => "early-stdin-empty",
            Self::LaunchValid => "launch-valid",
            Self::LaunchModelNonzero => "launch-model-nonzero",
            Self::LaunchProviderNonzeroAfterFinal => "launch-provider-nonzero-after-final",
            Self::LaunchProviderNonzeroNoFinal => "launch-provider-nonzero-no-final",
            Self::LaunchCancelledFinalEvent => "launch-cancelled-final-event",
            Self::LaunchMalformedLine => "launch-malformed-line",
            Self::LaunchMalformedLineNonzero => "launch-malformed-line-nonzero",
            Self::LaunchMalformedLineStderr => "launch-malformed-line-stderr",
            Self::LaunchBlankLine => "launch-blank-line",
            Self::LaunchExitThenLargeStdout => "launch-exit-then-large-stdout",
            Self::LaunchInvalidBase64 => "launch-invalid-base64",
            Self::LaunchDuplicateExit => "launch-duplicate-exit",
            Self::LaunchEventAfterExit => "launch-event-after-exit",
            Self::LaunchPartialHang => "launch-partial-hang",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LeakProbe {
    root: PathBuf,
}

impl LeakProbe {
    pub fn new() -> Self {
        let root = unique_temp_dir("leak-probe");
        fs::create_dir_all(&root).expect("leak probe directory should be created");
        Self { root }
    }

    pub fn wait_for_descendants(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if self.observed_pids().len() >= 2 {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "expected fake provider to report at least two descendant pids, observed {:?}",
            self.observed_pids()
        );
    }

    pub fn terminate_process_tree(&self, child: &mut Child) {
        terminate_process_tree(child);
    }

    pub fn assert_no_descendants(&self) {
        let pids = self.observed_pids();
        assert!(
            !pids.is_empty(),
            "leak probe must observe real descendant pids before asserting cleanup"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let alive = pids
                .iter()
                .copied()
                .filter(|pid| process_alive(*pid))
                .collect::<Vec<_>>();
            if alive.is_empty() {
                let _ = fs::remove_dir_all(&self.root);
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let alive = pids
            .iter()
            .copied()
            .filter(|pid| process_alive(*pid))
            .collect::<Vec<_>>();
        for pid in &alive {
            terminate_pid(*pid);
        }
        let _ = fs::remove_dir_all(&self.root);
        assert!(
            alive.is_empty(),
            "descendant pids were still alive after process-tree cleanup: {alive:?}"
        );
    }

    fn observed_pids(&self) -> Vec<u32> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read_to_string(entry.path()).ok())
            .filter_map(|text| text.trim().parse::<u32>().ok())
            .collect()
    }
}

impl Default for LeakProbe {
    fn default() -> Self {
        Self::new()
    }
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("oulipoly-provider-{label}-{nanos}"))
}

fn binary_path(root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        root.join("fake-provider-bin.exe")
    }
    #[cfg(not(windows))]
    {
        root.join("fake-provider-bin")
    }
}

fn retry_spawn(mut build: impl FnMut() -> Command) -> std::io::Result<Child> {
    let mut last_error = None;
    for _ in 0..10 {
        match build().spawn() {
            Ok(child) => return Ok(child),
            Err(error) if error.raw_os_error() == Some(26) => {
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.expect("retry loop should retain last executable-busy error"))
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    Command::new("cmd")
        .args([
            "/C",
            &format!("tasklist /FI \"PID eq {pid}\" | findstr {pid}"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn terminate_pid(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
}

#[cfg(windows)]
fn terminate_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

#[cfg(not(any(unix, windows)))]
fn terminate_pid(_pid: u32) {}

#[cfg(unix)]
fn wrapper_path(root: &Path, binary: &Path, marker: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let wrapper = root.join("fake-provider");
    let script = format!(
        "#!/bin/sh\nprintf spawned > '{}'\nexec '{}' \"$@\"\n",
        marker.display(),
        binary.display()
    );
    fs::write(&wrapper, script).expect("fake-provider wrapper should be written");
    let mut permissions = fs::metadata(&wrapper)
        .expect("wrapper metadata should be readable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wrapper, permissions).expect("wrapper should be executable");
    wrapper
}

#[cfg(windows)]
fn wrapper_path(_root: &Path, binary: &Path, _marker: &Path) -> PathBuf {
    binary.to_path_buf()
}

#[cfg(unix)]
fn terminate_process_tree(child: &mut Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGTERM);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(windows)]
fn terminate_process_tree(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn configure_test_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_test_process_group(_command: &mut Command) {}
