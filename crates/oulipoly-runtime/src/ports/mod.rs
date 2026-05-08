use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::SystemTime;
use uuid::Uuid;

/// Injectable wall-clock port.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Injectable UUID-v4 source.
pub trait UuidGenerator: Send + Sync {
    fn new_v4(&self) -> Uuid;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultUuidGenerator;

impl UuidGenerator for DefaultUuidGenerator {
    fn new_v4(&self) -> Uuid {
        Uuid::new_v4()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub stdin: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutcome {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub stdin: Option<Vec<u8>>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

/// Blocking OS subprocess runner. Existing executor call sites remain on
/// `std::process::Command` until later AGE-8 service WUs.
pub trait ProcessRunner: Send + Sync {
    fn run(&self, spec: ProcessSpec) -> Result<ProcessOutcome, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultProcessRunner;

impl ProcessRunner for DefaultProcessRunner {
    fn run(&self, spec: ProcessSpec) -> Result<ProcessOutcome, String> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        if let Some(current_dir) = &spec.current_dir {
            command.current_dir(current_dir);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        if spec.stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {e}", spec.program))?;
        if let Some(stdin) = spec.stdin.as_deref()
            && let Some(mut child_stdin) = child.stdin.take()
        {
            child_stdin
                .write_all(stdin)
                .map_err(|e| format!("Failed to write stdin to {}: {e}", spec.program))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|e| format!("Failed to wait for {}: {e}", spec.program))?;

        Ok(ProcessOutcome {
            program: spec.program,
            args: spec.args,
            current_dir: spec.current_dir,
            env: spec.env,
            stdin: spec.stdin,
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

/// Injectable stdout presenter.
pub trait StdoutSink: Send + Sync {
    fn write_line(&self, line: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StdoutWriter;

impl StdoutSink for StdoutWriter {
    fn write_line(&self, line: &str) -> Result<(), String> {
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{line}").map_err(|e| format!("Failed to write stdout: {e}"))
    }
}

/// Injectable stderr presenter.
pub trait StderrSink: Send + Sync {
    fn write_bytes(&self, bytes: &[u8]) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StderrWriter;

impl StderrSink for StderrWriter {
    fn write_bytes(&self, bytes: &[u8]) -> Result<(), String> {
        let mut stderr = std::io::stderr().lock();
        stderr
            .write_all(bytes)
            .map_err(|e| format!("Failed to write stderr: {e}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionScriptRequest {
    pub provider_name: String,
    pub command: String,
    pub state_dir: PathBuf,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionScriptOutcome {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

/// Skeleton for provider session-script invocation; production impl deferred.
pub trait SessionScriptRunner: Send + Sync {
    fn run_script(&self, request: SessionScriptRequest) -> Result<SessionScriptOutcome, String>;
}

/// Skeleton over transcript file IO; production impl deferred.
pub trait TranscriptFilesystem: Send + Sync {
    fn read(&self, path: PathBuf) -> Result<Vec<u8>, String>;
    fn write(&self, path: PathBuf, bytes: Vec<u8>) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLockRequest {
    pub lock_dir: PathBuf,
    pub provider_name: String,
    pub session_id: String,
}

/// Skeleton over session-lock construction; production impl deferred.
pub trait SessionLockFactory: Send + Sync {
    fn create_lock(&self, request: SessionLockRequest) -> Result<(), String>;
}
