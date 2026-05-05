use super::actions::AgentTurnResult;
use std::process::Command;
use std::time::{Duration, Instant};

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
        let mut cmd = Command::new("claude");
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

        let prompt = if self.session_id.is_none() {
            format!("{}\n\n---\n\n{}", self.system_prompt, message)
        } else {
            message.to_string()
        };

        cmd.arg(&prompt);

        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn claude CLI: {e}"))?;

        let timeout = Duration::from_secs(120);
        let start = Instant::now();

        let output = loop {
            match child.try_wait() {
                Ok(Some(_status)) => {
                    break child
                        .wait_with_output()
                        .map_err(|e| format!("Failed to read claude CLI output: {e}"))?;
                }
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        return Err("Claude CLI timed out after 120 seconds".to_string());
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(e) => {
                    let _ = child.kill();
                    return Err(format!("Failed to check claude CLI status: {e}"));
                }
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Claude CLI failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
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
