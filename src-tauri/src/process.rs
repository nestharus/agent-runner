use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
#[cfg(unix)]
use signal_hook::iterator::{Handle as SignalsHandle, Signals};

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

pub trait ProcessRunner: Send + Sync {
    fn run(&self, spec: CommandSpec) -> Result<ProcessOutput, String>;
    fn run_interactive(&self, spec: InteractiveCommandSpec) -> Result<i32, String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct OsProcessRunner;

impl ProcessRunner for OsProcessRunner {
    fn run(&self, spec: CommandSpec) -> Result<ProcessOutput, String> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        cmd.envs(&spec.env);
        cmd.stdin(match spec.stdin {
            StdinSpec::Null => Stdio::null(),
            StdinSpec::Bytes(_) => Stdio::piped(),
        });
        cmd.stdout(match spec.stdout {
            OutputSpec::Capture => Stdio::piped(),
            OutputSpec::Null => Stdio::null(),
        });
        cmd.stderr(match spec.stderr {
            OutputSpec::Capture => Stdio::piped(),
            OutputSpec::Null => Stdio::null(),
        });

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {e}", spec.description))?;

        let stdout_handle = match spec.stdout {
            OutputSpec::Capture => child.stdout.take().map(read_pipe),
            OutputSpec::Null => None,
        };
        let stderr_handle = match spec.stderr {
            OutputSpec::Capture => child.stderr.take().map(read_pipe),
            OutputSpec::Null => None,
        };

        if let StdinSpec::Bytes(bytes) = spec.stdin
            && let Some(mut stdin) = child.stdin.take()
        {
            stdin
                .write_all(&bytes)
                .map_err(|e| format!("Failed to write to stdin for {}: {e}", spec.description))?;
        }

        let status = if let Some(timeout) = spec.timeout {
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) if start.elapsed() >= timeout => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!(
                            "{} timed out after {}s",
                            spec.description,
                            timeout.as_secs()
                        ));
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(50)),
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

        Ok(ProcessOutput {
            stdout: join_pipe(stdout_handle)?,
            stderr: join_pipe(stderr_handle)?,
            exit_code: status.code().unwrap_or(-1),
            timed_out: false,
        })
    }

    fn run_interactive(&self, spec: InteractiveCommandSpec) -> Result<i32, String> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        cmd.envs(&spec.env);
        cmd.stdin(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());

        let mut child = cmd
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

fn read_pipe<R>(mut pipe: R) -> thread::JoinHandle<Result<Vec<u8>, String>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buf = Vec::new();
        pipe.read_to_end(&mut buf)
            .map_err(|e| format!("Failed to read process output: {e}"))?;
        Ok(buf)
    })
}

fn join_pipe(
    handle: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
) -> Result<Vec<u8>, String> {
    match handle {
        Some(handle) => handle
            .join()
            .map_err(|_| "Process output reader panicked".to_string())?,
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
    fn install(child: &mut std::process::Child) -> Result<Self, String> {
        let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])
            .map_err(|e| format!("Failed to install signal handlers: {e}"))?;
        let handle = signals.handle();
        let child_pid = child.id() as i32;

        let thread = thread::spawn(move || {
            for signal in signals.forever() {
                send_signal(child_pid, signal);
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
fn send_signal(pid: i32, signal: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    let _ = unsafe { kill(pid, signal) };
}
