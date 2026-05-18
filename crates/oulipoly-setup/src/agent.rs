//! ## Declared roles
//!
//! Roles: `orchestration`, `formatter`, `parser`, `validator`, `accessor`.
//!
//! Justifications:
//! - orchestration: `SetupAgent::send_turn` and `spawn_and_drain` orchestrate one Claude child round-trip.
//! - formatter: `compose_prompt` formats prompts and the `format_*` helpers format error strings.
//! - parser: `parse_response` parses JSON output and `extract_session_id` parses stderr.
//! - validator: `validate_exit_status` validates child exit status.
//! - accessor: `SetupAgent::session_id(&self) -> Option<&str>` exposes the stored session id.
//!
//! ## Adapter declarations
//!
//! Component: `SetupAgent::send_turn`
//! Role: adapter
//! Translates:
//! - SetupAgent turn intent + JSON schema -> Claude CLI -p invocation argv (--output-format json, --model claude-sonnet-4-6, --allowedTools Read/Bash/Glob/Grep, --no-session-persistence, --json-schema <schema>, optional --resume <sid>, prompt).
//! - Child stdout JSON object -> `AgentTurnResult` struct via serde_json.
//! - Child stderr text -> session_id token via `extract_session_id` helper, pulling from the canonical-doc-declared shape at `conventions/claude-cli-output-format.md` § Schema § Session-token vocabulary (forms `Session: <id>` and `session_id: <id>`).
//! - Child exit status + wall-clock deadline + drain outcome -> `Result<AgentTurnResult, String>` with six preserved error-string formats.
//!
//! ## External-schema pulls (push-pull declared schema owners)
//!
//! Per `~/ai/conventions/code-quality.md` § Push-vs-pull system coupling and
//! `~/ai/agents/push-pull-auditor.md` § Metric Binding "LOW canonical-doc-as-schema proof",
//! the following pull sites pull from declared schema owners rather than from
//! private upstream layout:
//!
//! - `extract_session_id` (via `SetupAgent::update_session_id`) pulls from the
//!   Claude CLI stderr session-token vocabulary declared in
//!   `conventions/claude-cli-output-format.md` § Schema § Session-token
//!   vocabulary. That document is the canonical schema owner for this
//!   stderr subset; this file pulls only from the two declared forms
//!   (`Session: <id>` and `session_id: <id>`).

use super::actions::AgentTurnResult;
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct SetupAgent {
    session_id: Option<String>,
    system_prompt: String,
    #[cfg(feature = "__test_fixtures")]
    command_builder: Option<Box<dyn Fn() -> Command + Send + Sync>>,
    #[cfg(feature = "__test_fixtures")]
    effective_timeout: Option<Duration>,
}

impl SetupAgent {
    pub fn new(system_prompt: String) -> Self {
        SetupAgent {
            session_id: None,
            system_prompt,
            #[cfg(feature = "__test_fixtures")]
            command_builder: None,
            #[cfg(feature = "__test_fixtures")]
            effective_timeout: None,
        }
    }

    #[cfg(feature = "__test_fixtures")]
    pub fn set_command_builder_for_test(
        &mut self,
        builder: Box<dyn Fn() -> Command + Send + Sync>,
    ) {
        self.command_builder = Some(builder);
    }

    #[cfg(feature = "__test_fixtures")]
    pub fn set_effective_timeout_for_test(&mut self, timeout: Duration) {
        self.effective_timeout = Some(timeout);
    }

    /// Send a turn to Claude Code. Returns the parsed structured response.
    pub fn send_turn(&mut self, message: &str, schema: &str) -> Result<AgentTurnResult, String> {
        let prompt = self.compose_prompt(message);
        let mut cmd = self.build_command(schema);
        cmd.arg(&prompt);

        let (status, stdout, stderr) = spawn_and_drain(cmd, self.effective_timeout())?;
        if let Err(status) = validate_exit_status(status) {
            return Err(format_exit_status_error(status, &stderr));
        }

        let result = parse_response(&stdout).map_err(|err| format_parse_error(err, &stdout))?;
        self.update_session_id(&stderr);

        Ok(result)
    }

    fn build_command(&self, schema: &str) -> Command {
        let mut cmd = {
            #[cfg(feature = "__test_fixtures")]
            {
                if let Some(builder) = self.command_builder.as_ref() {
                    builder()
                } else {
                    Command::new("claude")
                }
            }
            #[cfg(not(feature = "__test_fixtures"))]
            {
                Command::new("claude")
            }
        };

        cmd.arg("-p")
            .arg("--output-format")
            .arg("json")
            .arg("--model")
            .arg("claude-sonnet-4-6")
            .arg("--allowedTools")
            .arg("Read,Bash,Glob,Grep")
            .arg("--no-session-persistence");

        // Add JSON schema constraint
        cmd.arg("--json-schema").arg(schema);

        if let Some(ref sid) = self.session_id {
            cmd.arg("--resume").arg(sid);
        }

        cmd
    }

    fn compose_prompt(&self, message: &str) -> String {
        if self.session_id.is_none() {
            format!("{}\n\n---\n\n{}", self.system_prompt, message)
        } else {
            message.to_string()
        }
    }

    fn effective_timeout(&self) -> Duration {
        {
            #[cfg(feature = "__test_fixtures")]
            {
                self.effective_timeout.unwrap_or(Duration::from_secs(120))
            }
            #[cfg(not(feature = "__test_fixtures"))]
            {
                Duration::from_secs(120)
            }
        }
    }

    /// Update the stored session id from child stderr.
    ///
    /// Pulls only from the canonical-doc-declared shape at
    /// `conventions/claude-cli-output-format.md` § Schema § Session-token
    /// vocabulary; see `extract_session_id` for the parse contract.
    fn update_session_id(&mut self, stderr: &[u8]) {
        let stderr_str = String::from_utf8_lossy(stderr);
        if let Some(sid) = extract_session_id(&stderr_str) {
            self.session_id = Some(sid);
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

fn spawn_and_drain(
    mut cmd: Command,
    timeout: Duration,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), String> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = spawn_child(cmd)?;
    let stdout_handle = spawn_reader_for_pipe(child.stdout.take());
    let stderr_handle = spawn_reader_for_pipe(child.stderr.take());

    if let Err(failure) = wait_for_child_exit(&mut child, timeout) {
        terminate_child_and_join(&mut child, stdout_handle, stderr_handle);
        return Err(format_wait_failure(failure));
    }

    let status = wait_child_status(&mut child).map_err(format_check_status_error)?;
    let stdout_bytes = join_reader(stdout_handle).map_err(format_read_error)?;
    let stderr_bytes = join_reader(stderr_handle).map_err(format_read_error)?;

    Ok((status, stdout_bytes, stderr_bytes))
}

fn spawn_child(mut cmd: Command) -> Result<Child, String> {
    cmd.spawn().map_err(format_spawn_error)
}

fn spawn_reader_for_pipe<R>(reader: Option<R>) -> JoinHandle<Result<Vec<u8>, io::Error>>
where
    R: Read + Send + 'static,
{
    reader
        .map(spawn_reader_thread)
        .unwrap_or_else(|| std::thread::spawn(|| Ok(Vec::new())))
}

fn spawn_reader_thread<R>(mut reader: R) -> JoinHandle<Result<Vec<u8>, io::Error>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        io::copy(&mut reader, &mut bytes)?;
        Ok(bytes)
    })
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> Result<(), ChildWaitFailure> {
    let start = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return Ok(()),
            Ok(None) => {
                if start.elapsed() > timeout {
                    return Err(ChildWaitFailure::Timeout);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(err) => return Err(ChildWaitFailure::CheckStatus(err)),
        }
    }
}

fn terminate_child_and_join(
    child: &mut Child,
    stdout_handle: JoinHandle<Result<Vec<u8>, io::Error>>,
    stderr_handle: JoinHandle<Result<Vec<u8>, io::Error>>,
) {
    let _ = child.kill();
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();
}

fn wait_child_status(child: &mut Child) -> io::Result<ExitStatus> {
    child.wait()
}

fn join_reader(handle: JoinHandle<Result<Vec<u8>, io::Error>>) -> io::Result<Vec<u8>> {
    handle.join().unwrap_or_else(|_| Ok(Vec::new()))
}

fn validate_exit_status(status: ExitStatus) -> Result<(), ExitStatus> {
    if status.success() {
        Ok(())
    } else {
        Err(status)
    }
}

fn parse_response(stdout: &[u8]) -> Result<AgentTurnResult, serde_json::Error> {
    let stdout = String::from_utf8_lossy(stdout);
    serde_json::from_str(&stdout)
}

enum ChildWaitFailure {
    Timeout,
    CheckStatus(io::Error),
}

fn format_spawn_error(err: io::Error) -> String {
    format!("Failed to spawn claude CLI: {err}")
}

fn format_check_status_error(err: io::Error) -> String {
    format!("Failed to check claude CLI status: {err}")
}

fn format_read_error(err: io::Error) -> String {
    format!("Failed to read claude CLI output: {err}")
}

fn format_wait_failure(failure: ChildWaitFailure) -> String {
    match failure {
        ChildWaitFailure::Timeout => "Claude CLI timed out after 120 seconds".to_string(),
        ChildWaitFailure::CheckStatus(err) => format_check_status_error(err),
    }
}

fn format_exit_status_error(status: ExitStatus, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    format!(
        "Claude CLI failed (exit {}): {}",
        status.code().unwrap_or(-1),
        stderr.chars().take(500).collect::<String>()
    )
}

fn format_parse_error(err: serde_json::Error, stdout: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    format!(
        "Failed to parse agent response: {err}\nRaw output: {}",
        stdout.chars().take(200).collect::<String>()
    )
}

/// Pull the Claude CLI session id from child stderr.
///
/// The shape this function pulls from is declared by
/// `conventions/claude-cli-output-format.md` § Schema § Session-token
/// vocabulary. That document is the canonical schema owner; it
/// declares the two recognized line-prefix forms (`Session: <id>` and
/// `session_id: <id>`) and the parse semantics (line-by-line,
/// trim-then-strip-prefix, first match wins, absent if no match).
///
/// This pull site MUST NOT read any other stderr substring as a
/// session id; doing so would split into an undeclared private-source
/// pull per `~/ai/agents/push-pull-auditor.md` § Metric Binding.
fn extract_session_id(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Session: ") {
            return Some(rest.trim().to_string());
        }
        if let Some(rest) = trimmed.strip_prefix("session_id: ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_creation() {
        let agent = SetupAgent::new("test prompt".to_string());
        assert!(agent.session_id().is_none());
    }

    #[test]
    fn extract_session_id_found() {
        let stderr = "Starting session...\nSession: abc-123-def\nReady.";
        assert_eq!(extract_session_id(stderr), Some("abc-123-def".to_string()));
    }

    #[test]
    fn extract_session_id_not_found() {
        assert_eq!(extract_session_id("no session info here"), None);
    }

    #[test]
    fn extract_session_id_alternative_format() {
        let stderr = "session_id: 550e8400-e29b-41d4-a716-446655440000\n";
        assert_eq!(
            extract_session_id(stderr),
            Some("550e8400-e29b-41d4-a716-446655440000".to_string())
        );
    }
}
