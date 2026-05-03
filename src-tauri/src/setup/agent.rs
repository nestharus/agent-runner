use super::actions::AgentTurnResult;
use crate::process::{CommandSpec, OsProcessRunner, OutputSpec, ProcessRunner, StdinSpec};
use std::time::Duration;

pub struct SetupAgent {
    session_id: Option<String>,
    system_prompt: String,
}

impl SetupAgent {
    pub fn new(system_prompt: String) -> Self {
        SetupAgent {
            session_id: None,
            system_prompt,
        }
    }

    /// Send a turn to Claude Code. Returns the parsed structured response.
    pub fn send_turn(&mut self, message: &str, schema: &str) -> Result<AgentTurnResult, String> {
        self.send_turn_with_runner(&OsProcessRunner, message, schema)
    }

    pub fn send_turn_with_runner(
        &mut self,
        runner: &dyn ProcessRunner,
        message: &str,
        schema: &str,
    ) -> Result<AgentTurnResult, String> {
        let mut args = vec![
            "-p".to_string(),
            "--output-format".to_string(),
            "json".to_string(),
            "--model".to_string(),
            "claude-sonnet-4-6".to_string(),
            "--allowedTools".to_string(),
            "Read,Bash,Glob,Grep".to_string(),
            "--no-session-persistence".to_string(),
            "--json-schema".to_string(),
            schema.to_string(),
        ];
        if let Some(ref sid) = self.session_id {
            args.push("--resume".to_string());
            args.push(sid.clone());
        }

        let prompt = if self.session_id.is_none() {
            format!("{}\n\n---\n\n{}", self.system_prompt, message)
        } else {
            message.to_string()
        };
        args.push(prompt);

        let output = runner
            .run(CommandSpec {
                program: "claude".to_string(),
                args,
                cwd: None,
                env: Default::default(),
                stdin: StdinSpec::Null,
                stdout: OutputSpec::Capture,
                stderr: OutputSpec::Capture,
                timeout: Some(Duration::from_secs(120)),
                description: "claude setup agent".to_string(),
            })
            .map_err(|e| {
                if e.contains("timed out") && e.contains("120") {
                    "Claude CLI timed out after 120 seconds".to_string()
                } else if e.contains("Failed to spawn") {
                    format!("Failed to spawn claude CLI: {e}")
                } else {
                    e
                }
            })?;

        if output.exit_code != 0 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Claude CLI failed (exit {}): {}",
                output.exit_code,
                stderr.chars().take(500).collect::<String>()
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse the JSON response
        // Claude --output-format json wraps the result; we need to extract the structured content
        let result: AgentTurnResult = serde_json::from_str(&stdout).map_err(|e| {
            format!(
                "Failed to parse agent response: {e}\nRaw output: {}",
                stdout.chars().take(200).collect::<String>()
            )
        })?;

        // Try to extract session_id from stderr or response metadata
        // Claude CLI outputs session info to stderr
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        if let Some(sid) = extract_session_id(&stderr_str) {
            self.session_id = Some(sid);
        }

        Ok(result)
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

fn extract_session_id(stderr: &str) -> Option<String> {
    // Claude CLI may output session ID in stderr
    // Look for patterns like "Session: <uuid>" or "session_id: <uuid>"
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
    use crate::setup::actions::AgentAction;
    use crate::setup::test_support::env_lock;
    use std::panic::{AssertUnwindSafe, catch_unwind};

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

    #[cfg(unix)]
    fn with_fake_claude(script: &str, test: impl FnOnce(&std::path::Path, &std::path::Path)) {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("claude");
        let args_log = dir.path().join("args.log");
        std::fs::write(&bin, script).unwrap();
        let mut perms = std::fs::metadata(&bin).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin, perms).unwrap();

        let previous_path = std::env::var_os("PATH");
        let previous_args_log = std::env::var_os("CLAUDE_ARGS_LOG");
        let new_path = match previous_path.as_ref() {
            Some(path) => {
                let mut paths = vec![dir.path().to_path_buf()];
                paths.extend(std::env::split_paths(path));
                std::env::join_paths(paths).unwrap()
            }
            None => dir.path().as_os_str().to_os_string(),
        };

        unsafe {
            std::env::set_var("PATH", new_path);
            std::env::set_var("CLAUDE_ARGS_LOG", &args_log);
        }

        let result = catch_unwind(AssertUnwindSafe(|| test(dir.path(), &args_log)));

        match previous_path {
            Some(value) => unsafe {
                std::env::set_var("PATH", value);
            },
            None => unsafe {
                std::env::remove_var("PATH");
            },
        }
        match previous_args_log {
            Some(value) => unsafe {
                std::env::set_var("CLAUDE_ARGS_LOG", value);
            },
            None => unsafe {
                std::env::remove_var("CLAUDE_ARGS_LOG");
            },
        }

        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    fn successful_fake_claude_script() -> &'static str {
        r#"#!/usr/bin/env bash
{
  echo "---ARGV---"
  for arg in "$@"; do
    printf '%s\n' "$arg"
  done
} >> "$CLAUDE_ARGS_LOG"
echo "Session: setup-session-123" >&2
printf '%s' '{"actions":[{"type":"status","message":"Ready"}],"done":true}'
"#
    }

    fn logged_invocations(path: &std::path::Path) -> Vec<Vec<String>> {
        let content = std::fs::read_to_string(path).unwrap();
        content
            .split("---ARGV---\n")
            .filter(|part| !part.trim().is_empty())
            .map(|part| part.lines().map(ToOwned::to_owned).collect())
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn send_turn_uses_schema_constrained_json_mode_and_parses_actions() {
        with_fake_claude(successful_fake_claude_script(), |_, args_log| {
            let mut agent = SetupAgent::new("system prompt".to_string());

            let result = agent
                .send_turn("begin setup", "{\"type\":\"object\"}")
                .unwrap();

            assert!(result.done);
            assert_eq!(result.actions.len(), 1);
            match &result.actions[0] {
                AgentAction::Status { message } => assert_eq!(message, "Ready"),
                _ => panic!("expected status action"),
            }
            assert_eq!(agent.session_id(), Some("setup-session-123"));

            let invocations = logged_invocations(args_log);
            assert_eq!(invocations.len(), 1);
            let args = &invocations[0];
            assert_eq!(args[0], "-p");
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--output-format", "json"])
            );
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--model", "claude-sonnet-4-6"])
            );
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--allowedTools", "Read,Bash,Glob,Grep"])
            );
            assert!(args.iter().any(|arg| arg == "--no-session-persistence"));
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--json-schema", "{\"type\":\"object\"}"])
            );
            assert!(!args.iter().any(|arg| arg == "--resume"));

            assert!(args.iter().any(|arg| arg == "system prompt"), "{args:?}");
            assert!(args.iter().any(|arg| arg == "---"), "{args:?}");
            assert!(args.iter().any(|arg| arg == "begin setup"), "{args:?}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn send_turn_resumes_with_learned_session_id_and_message_only_prompt() {
        with_fake_claude(successful_fake_claude_script(), |_, args_log| {
            let mut agent = SetupAgent::new("system prompt".to_string());

            agent.send_turn("first turn", "{}").unwrap();
            agent.send_turn("second turn", "{}").unwrap();

            let invocations = logged_invocations(args_log);
            assert_eq!(invocations.len(), 2);
            let second_args = &invocations[1];
            assert!(
                second_args
                    .windows(2)
                    .any(|pair| pair == ["--resume", "setup-session-123"])
            );
            assert_eq!(second_args.last().unwrap(), "second turn");
            assert!(!second_args.last().unwrap().contains("system prompt"));
        });
    }

    #[cfg(unix)]
    #[test]
    fn send_turn_reports_nonzero_claude_exit() {
        let script = r#"#!/usr/bin/env bash
echo "oauth expired" >&2
exit 7
"#;

        with_fake_claude(script, |_, _| {
            let mut agent = SetupAgent::new("system prompt".to_string());

            let err = match agent.send_turn("begin setup", "{}") {
                Ok(_) => panic!("expected nonzero Claude exit to fail"),
                Err(err) => err,
            };

            assert!(err.contains("Claude CLI failed (exit 7)"), "{err}");
            assert!(err.contains("oauth expired"), "{err}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn send_turn_rejects_malformed_agent_json() {
        let script = r#"#!/usr/bin/env bash
printf '%s' 'not-json'
"#;

        with_fake_claude(script, |_, _| {
            let mut agent = SetupAgent::new("system prompt".to_string());

            let err = match agent.send_turn("begin setup", "{}") {
                Ok(_) => panic!("expected malformed JSON to fail"),
                Err(err) => err,
            };

            assert!(err.contains("Failed to parse agent response"), "{err}");
            assert!(err.contains("Raw output: not-json"), "{err}");
        });
    }
}
