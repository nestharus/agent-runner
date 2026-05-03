use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
#[cfg(unix)]
use signal_hook::iterator::{Handle as SignalsHandle, Signals};
#[cfg(unix)]
use std::process::Child;
#[cfg(unix)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[cfg(unix)]
use std::thread;

pub trait ProcessRunner: Send + Sync {
    fn run(&self, spec: CommandSpec) -> Result<ProcessOutput, String>;
    fn run_interactive(&self, spec: InteractiveCommandSpec) -> Result<i32, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub stdin: StdinSpec,
    pub stdout: OutputSpec,
    pub stderr: OutputSpec,
    pub timeout: Option<Duration>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdinSpec {
    Null,
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSpec {
    Capture,
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractiveCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub description: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct OsProcessRunner;

impl ProcessRunner for OsProcessRunner {
    fn run(&self, spec: CommandSpec) -> Result<ProcessOutput, String> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }

        match &spec.stdin {
            StdinSpec::Null => {
                command.stdin(Stdio::null());
            }
            StdinSpec::Bytes(_) => {
                command.stdin(Stdio::piped());
            }
        }
        match spec.stdout {
            OutputSpec::Capture => {
                command.stdout(Stdio::piped());
            }
            OutputSpec::Null => {
                command.stdout(Stdio::null());
            }
        }
        match spec.stderr {
            OutputSpec::Capture => {
                command.stderr(Stdio::piped());
            }
            OutputSpec::Null => {
                command.stderr(Stdio::null());
            }
        }

        let mut child = command
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {e}", spec.description))?;

        if let StdinSpec::Bytes(bytes) = &spec.stdin
            && let Some(mut stdin) = child.stdin.take()
        {
            stdin
                .write_all(bytes)
                .map_err(|e| format!("Failed to write stdin for {}: {e}", spec.description))?;
        }

        let stdout_handle = child.stdout.take().map(|mut stdout| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stdout.read_to_end(&mut buf);
                buf
            })
        });
        let stderr_handle = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stderr.read_to_end(&mut buf);
                buf
            })
        });

        let status = if let Some(timeout) = spec.timeout {
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) => {
                        if start.elapsed() >= timeout {
                            let _ = child.kill();
                            let _ = child.wait();
                            let _ = join_reader(stdout_handle);
                            let _ = join_reader(stderr_handle);
                            return Err(format!(
                                "{} timed out after {}s",
                                spec.description,
                                timeout.as_secs()
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => {
                        let _ = child.kill();
                        return Err(format!("Failed to wait for {}: {e}", spec.description));
                    }
                }
            }
        } else {
            child
                .wait()
                .map_err(|e| format!("Failed to wait for {}: {e}", spec.description))?
        };

        let stdout = join_reader(stdout_handle)?;
        let stderr = join_reader(stderr_handle)?;

        Ok(ProcessOutput {
            stdout,
            stderr,
            exit_code: status.code().unwrap_or(-1),
            timed_out: false,
        })
    }

    fn run_interactive(&self, spec: InteractiveCommandSpec) -> Result<i32, String> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = command
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {e}", spec.description))?;

        #[cfg(unix)]
        let signal_guard = InteractiveSignalGuard::install(&mut child)?;

        let status = child
            .wait()
            .map_err(|e| format!("Failed to wait for {}: {e}", spec.description))?;

        #[cfg(unix)]
        drop(signal_guard);

        Ok(status.code().unwrap_or(-1))
    }
}

fn join_reader(handle: Option<std::thread::JoinHandle<Vec<u8>>>) -> Result<Vec<u8>, String> {
    match handle {
        Some(handle) => handle
            .join()
            .map_err(|_| "Failed to join process output reader".to_string()),
        None => Ok(Vec::new()),
    }
}

#[cfg(unix)]
struct InteractiveSignalGuard {
    handle: SignalsHandle,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl InteractiveSignalGuard {
    fn install(child: &mut Child) -> Result<Self, String> {
        let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])
            .map_err(|e| format!("Failed to install signal handlers: {e}"))?;
        let handle = signals.handle();
        let child_pid = child.id() as i32;
        let forwarded_sigterm = Arc::new(AtomicBool::new(false));
        let thread_forwarded_sigterm = Arc::clone(&forwarded_sigterm);

        let thread = thread::spawn(move || {
            for signal in signals.forever() {
                if signal == SIGTERM && !thread_forwarded_sigterm.swap(true, Ordering::SeqCst) {
                    send_sigterm(child_pid);
                }
            }
        });

        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}

#[cfg(unix)]
impl Drop for InteractiveSignalGuard {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(unix)]
fn send_sigterm(pid: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    let _ = unsafe { kill(pid, SIGTERM) };
}
