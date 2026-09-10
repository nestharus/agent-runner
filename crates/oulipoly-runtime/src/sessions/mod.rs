//! Provider-neutral transcript location through configured adapter scripts.
//!
//! ## Declared roles
//!
//! `accessor`, `formatter`, `orchestration`, `validator`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/sessions/mod.rs
//!     role: intrinsic-surface
//!     Domain: session_transcript_location
//!     Owns:
//!       - transcript locator script dispatch
//!       - bounded locator process lifetime
//!       - exact single-path stdout validation
//! ```

use oulipoly_config::{SessionSourceEntry, SessionsConfig};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;

const SCRIPT_TIMEOUT_SECS: u64 = 90;

pub fn locate_transcript(
    sessions_cfg: &SessionsConfig,
    provider_name: &str,
    session_id: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(entry) = sessions_cfg.get(provider_name) else {
        return Ok(None);
    };
    let Some(locator) = entry.transcript_locator.as_deref() else {
        return Ok(None);
    };

    let state_dir = resolve_state_dir(provider_name, entry)?;
    create_session_state_dir(&state_dir)?;
    let stdout = run_session_script(locator, &state_dir, Some(session_id), "transcript locator")?;
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let line = match lines.as_slice() {
        [] => return Err("transcript locator returned empty stdout".to_string()),
        [line] => *line,
        _ => return Err("transcript locator stdout was not a single line".to_string()),
    };
    Ok(Some(PathBuf::from(line)))
}

fn resolve_state_dir(provider_name: &str, entry: &SessionSourceEntry) -> Result<PathBuf, String> {
    if let Some(dir) = &entry.state_dir {
        return Ok(dir.clone());
    }
    oulipoly_state::paths::data_dir().map(|base| base.join("sessions").join(provider_name))
}

fn create_session_state_dir(state_dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(state_dir).map_err(|error| {
        format!(
            "could not create state_dir {}: {error}",
            state_dir.display()
        )
    })
}

fn run_session_script(
    script: &str,
    state_dir: &std::path::Path,
    session_id: Option<&str>,
    script_kind: &str,
) -> Result<String, String> {
    let mut cmd = session_script_command(script, state_dir, session_id);
    let mut child = cmd
        .spawn()
        .map_err(|error| format!("Failed to spawn {script_kind}: {error}"))?;

    let stdout_handle = spawn_script_reader(child.stdout.take().expect("piped"));
    let stderr_handle = spawn_script_reader(child.stderr.take().expect("piped"));
    let status = wait_for_session_script(&mut child, script_kind)?;
    let stdout = join_script_reader(stdout_handle);
    let stderr = join_script_reader(stderr_handle);
    if status.success() {
        Ok(stdout)
    } else {
        Err(format!(
            "{} exited {}: {}",
            capitalize_script_kind(script_kind),
            status.code().unwrap_or(-1),
            stderr.trim()
        ))
    }
}

fn session_script_command(
    script: &str,
    state_dir: &std::path::Path,
    session_id: Option<&str>,
) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    cmd.env("STATE_DIR", state_dir);
    if let Some(session_id) = session_id {
        cmd.env("SESSION_ID", session_id);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    configure_session_script_process_group(&mut cmd);
    cmd
}

#[cfg(unix)]
fn configure_session_script_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    cmd.process_group(0);
}

#[cfg(not(unix))]
fn configure_session_script_process_group(_cmd: &mut Command) {}

fn spawn_script_reader<R>(mut reader: R) -> JoinHandle<String>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut output = String::new();
        reader.read_to_string(&mut output).ok();
        output
    })
}

fn wait_for_session_script(child: &mut Child, script_kind: &str) -> Result<ExitStatus, String> {
    let timeout = std::time::Duration::from_secs(SCRIPT_TIMEOUT_SECS);
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(None) => {
                kill_session_script_process_group(child);
                return Err(format!(
                    "script_timeout: {script_kind} timed out after {SCRIPT_TIMEOUT_SECS}s"
                ));
            }
            Err(error) => {
                return Err(format!(
                    "{} wait failed: {error}",
                    capitalize_script_kind(script_kind)
                ));
            }
        }
    }
}

#[cfg(unix)]
fn kill_session_script_process_group(child: &mut Child) {
    let pgid = -(child.id() as libc::pid_t);
    // SAFETY: the child was placed in its own process group above.
    let _ = unsafe { libc::kill(pgid, libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn kill_session_script_process_group(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn join_script_reader(handle: JoinHandle<String>) -> String {
    handle.join().unwrap_or_default()
}

fn capitalize_script_kind(kind: &str) -> String {
    let mut chars = kind.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => kind.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        _dir: tempfile::TempDir,
        script: PathBuf,
        state_dir: PathBuf,
    }

    impl Fixture {
        fn new(body: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("locator.sh");
            std::fs::write(&script, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
            #[cfg(unix)]
            {
                let mut permissions = std::fs::metadata(&script).unwrap().permissions();
                permissions.set_mode(0o755);
                std::fs::set_permissions(&script, permissions).unwrap();
            }
            let state_dir = dir.path().join("state");
            Self {
                _dir: dir,
                script,
                state_dir,
            }
        }

        fn config(&self, with_locator: bool) -> SessionsConfig {
            let mut entries = HashMap::new();
            entries.insert(
                "provider".to_string(),
                SessionSourceEntry {
                    turn_script: "unused".to_string(),
                    transcript_locator: with_locator
                        .then(|| self.script.to_string_lossy().into_owned()),
                    state_dir: Some(self.state_dir.clone()),
                },
            );
            SessionsConfig { entries }
        }
    }

    #[test]
    fn locate_transcript_returns_none_when_no_locator_is_configured() {
        let fixture = Fixture::new("exit 0");

        assert_eq!(
            locate_transcript(&fixture.config(false), "provider", "session").unwrap(),
            None
        );
    }

    #[test]
    fn locate_transcript_returns_script_stdout_path() {
        let fixture = Fixture::new("printf '%s\\n' \"$SESSION_ID.jsonl\"");

        assert_eq!(
            locate_transcript(&fixture.config(true), "provider", "session").unwrap(),
            Some(PathBuf::from("session.jsonl"))
        );
        assert!(fixture.state_dir.is_dir());
    }

    #[test]
    fn locate_transcript_returns_error_on_nonzero_exit() {
        let fixture = Fixture::new("printf 'locator failed' >&2\nexit 7");

        let error = locate_transcript(&fixture.config(true), "provider", "session").unwrap_err();
        assert!(error.contains("Transcript locator exited 7: locator failed"));
    }

    #[test]
    fn locate_transcript_returns_error_on_empty_stdout() {
        let fixture = Fixture::new("exit 0");

        assert_eq!(
            locate_transcript(&fixture.config(true), "provider", "session").unwrap_err(),
            "transcript locator returned empty stdout"
        );
    }
}
