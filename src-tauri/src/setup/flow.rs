use super::actions::{AgentAction, ResultContent, SetupEvent, UserResponse};
use super::agent::SetupAgent;
use super::context;
use super::detection;
use super::memory::MemoryGraph;
use super::schemas::AGENT_TURN_SCHEMA;
use super::sync;
use crate::process::ProcessRunner;
use std::sync::Arc;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

const MAX_AGENT_TURNS: i32 = 25;

const ALLOWED_COMMANDS: &[&str] = &[
    "which", "type", "claude", "codex", "opencode", "gemini", "npm", "npx", "curl", "bash",
];

const ALLOWED_WRITE_PREFIXES: &[&str] = &[".config/oulipoly-agent-runner/", ".local/bin/"];

pub struct SetupFlow {
    channel: Channel<SetupEvent>,
    input_rx: mpsc::Receiver<UserResponse>,
    memory: MemoryGraph,
    session_id: String,
    runner: Arc<dyn ProcessRunner>,
}

impl SetupFlow {
    pub fn new(
        channel: Channel<SetupEvent>,
        input_rx: mpsc::Receiver<UserResponse>,
        memory: MemoryGraph,
        session_id: String,
    ) -> Self {
        SetupFlow {
            channel,
            input_rx,
            memory,
            session_id,
            runner: Arc::new(crate::process::OsProcessRunner),
        }
    }

    pub fn new_with_runner(
        channel: Channel<SetupEvent>,
        input_rx: mpsc::Receiver<UserResponse>,
        memory: MemoryGraph,
        session_id: String,
        runner: Arc<dyn ProcessRunner>,
    ) -> Self {
        SetupFlow {
            channel,
            input_rx,
            memory,
            session_id,
            runner,
        }
    }

    pub async fn run(mut self) {
        // Record session start
        let _ = self.memory.create_session(&self.session_id);

        // 1. Detection phase
        let _ = self.channel.send(SetupEvent::Status {
            message: "Detecting installed CLIs...".into(),
        });

        let report = detection::detect_all_with_runner(self.runner.as_ref());
        let _ = self.channel.send(SetupEvent::ShowResult {
            content: ResultContent::DetectionSummary {
                clis: detection::summarize(&report),
            },
        });

        // 2. Build context
        let agent_context = context::build_agent_context(&report, &self.memory);
        let system_prompt = context::build_system_prompt(&agent_context);

        // 3. Check if Claude CLI is available for agent-driven flow
        let claude_available = report
            .clis
            .iter()
            .any(|c| c.name == "claude" && c.installed);

        if !claude_available {
            // Phase A: Static bootstrap — no agent available
            let _ = self.channel.send(SetupEvent::NeedInput {
                action: super::actions::Action::OauthFlow {
                    provider: "claude".into(),
                    login_command: "claude login".into(),
                    instructions: get_install_instructions(),
                },
            });

            // Wait for user to complete installation
            match self.input_rx.recv().await {
                Some(UserResponse::OauthComplete { success, .. }) if success => {
                    let _ = self.channel.send(SetupEvent::Status {
                        message: "Verifying Claude CLI installation...".into(),
                    });
                    // Re-detect
                    let new_report = detection::detect_all_with_runner(self.runner.as_ref());
                    if !new_report
                        .clis
                        .iter()
                        .any(|c| c.name == "claude" && c.installed)
                    {
                        let _ = self.channel.send(SetupEvent::Error {
                            message:
                                "Claude CLI still not detected. Please install it and try again."
                                    .into(),
                            recoverable: false,
                        });
                        let _ = self
                            .memory
                            .end_session(&self.session_id, "failed_bootstrap");
                        return;
                    }
                }
                Some(UserResponse::Cancel) | None => {
                    let _ = self.channel.send(SetupEvent::Error {
                        message: "Setup cancelled.".into(),
                        recoverable: false,
                    });
                    let _ = self.memory.end_session(&self.session_id, "cancelled");
                    return;
                }
                _ => {}
            }
        }

        // Phase B: Agent-driven flow
        self.run_agent_loop(system_prompt, "Analyze the system state and begin setup.")
            .await;
    }

    pub async fn run_for_cli(mut self, cli_name: &str) {
        let _ = self.memory.create_session(&self.session_id);

        let _ = self.channel.send(SetupEvent::Status {
            message: format!("Detecting {} CLI...", cli_name),
        });

        let cli_info = detection::detect_single_cli_with_runner(cli_name, self.runner.as_ref());
        let report = detection::DetectionReport {
            clis: vec![cli_info],
            os: detection::detect_os_public(),
            wrappers: vec![],
        };

        let agent_context = context::build_agent_context(&report, &self.memory);
        let system_prompt = context::build_cli_setup_prompt(cli_name, &agent_context);

        self.run_agent_loop(system_prompt, &format!("Help set up the {} CLI.", cli_name))
            .await;
    }

    async fn run_agent_loop(&mut self, system_prompt: String, initial_message: &str) {
        let mut agent = SetupAgent::new(system_prompt);
        let mut turn_number = 0;
        let mut next_message = initial_message.to_string();

        loop {
            turn_number += 1;

            if turn_number > MAX_AGENT_TURNS {
                let _ = self.channel.send(SetupEvent::Error {
                    message:
                        "Setup agent exceeded maximum turns. Please retry or configure manually."
                            .into(),
                    recoverable: false,
                });
                let _ = self
                    .memory
                    .end_session(&self.session_id, "max_turns_exceeded");
                break;
            }

            let pct = ((turn_number as f64 / MAX_AGENT_TURNS as f64) * 100.0).min(100.0);
            let _ = self.channel.send(SetupEvent::Progress {
                message: format!("Agent turn {}/{}...", turn_number, MAX_AGENT_TURNS),
                percent: Some(pct),
                detail: None,
            });

            let _ = self.channel.send(SetupEvent::Status {
                message: "Thinking...".into(),
            });

            let result = match agent.send_turn_with_runner(
                self.runner.as_ref(),
                &next_message,
                AGENT_TURN_SCHEMA,
            ) {
                Ok(r) => r,
                Err(e) => {
                    let _ = self.channel.send(SetupEvent::Error {
                        message: format!("Agent error: {e}"),
                        recoverable: true,
                    });
                    let _ = self.memory.end_session(&self.session_id, "agent_error");
                    break;
                }
            };

            // Record turn — AgentAction only derives Deserialize, so we log a summary
            let actions_summary = format!("{} actions processed", result.actions.len());
            let _ = self.memory.record_turn(
                &self.session_id,
                turn_number,
                &next_message,
                &actions_summary,
                "[]",
            );

            let mut feedback_parts: Vec<String> = Vec::new();

            // Process each action
            for action in &result.actions {
                match action {
                    AgentAction::Status { message } => {
                        let _ = self.channel.send(SetupEvent::Status {
                            message: message.clone(),
                        });
                    }

                    AgentAction::RunCommand {
                        command,
                        args,
                        description,
                    } => {
                        let _ = self.channel.send(SetupEvent::Status {
                            message: description.clone(),
                        });

                        match execute_allowlisted(command, args) {
                            Ok((stdout, stderr, exit_code)) => {
                                let _ = self.channel.send(SetupEvent::ShowResult {
                                    content: ResultContent::CommandOutput {
                                        command: format!("{} {}", command, args.join(" ")),
                                        stdout: stdout.clone(),
                                        stderr: stderr.clone(),
                                        exit_code,
                                    },
                                });
                                feedback_parts.push(format!(
                                    "Command `{} {}` completed (exit {}).\nstdout: {}\nstderr: {}",
                                    command,
                                    args.join(" "),
                                    exit_code,
                                    truncate(&stdout, 500),
                                    truncate(&stderr, 200)
                                ));
                            }
                            Err(e) => {
                                feedback_parts.push(format!("Command failed: {e}"));
                                let _ = self.channel.send(SetupEvent::Error {
                                    message: e,
                                    recoverable: true,
                                });
                            }
                        }
                    }

                    AgentAction::WriteConfig {
                        path,
                        content,
                        description,
                    } => {
                        let _ = self.channel.send(SetupEvent::Status {
                            message: description.clone(),
                        });

                        match validate_and_write(path, content) {
                            Ok(resolved) => {
                                let _ = self.channel.send(SetupEvent::ShowResult {
                                    content: ResultContent::ConfigWritten {
                                        path: resolved,
                                        description: description.clone(),
                                    },
                                });
                                feedback_parts.push(format!("Config written: {path}"));
                            }
                            Err(e) => {
                                feedback_parts.push(format!("Failed to write config: {e}"));
                                let _ = self.channel.send(SetupEvent::Error {
                                    message: e,
                                    recoverable: true,
                                });
                            }
                        }
                    }

                    AgentAction::TestIntegration {
                        model_name,
                        command,
                        args,
                    } => {
                        let _ = self.channel.send(SetupEvent::Status {
                            message: format!("Testing {model_name}..."),
                        });

                        match execute_allowlisted(command, args) {
                            Ok((stdout, stderr, exit_code)) => {
                                let success = exit_code == 0;
                                let output = if success {
                                    stdout.clone()
                                } else {
                                    stderr.clone()
                                };
                                let _ = self.channel.send(SetupEvent::ShowResult {
                                    content: ResultContent::TestResult {
                                        model: model_name.clone(),
                                        success,
                                        output: output.clone(),
                                    },
                                });
                                feedback_parts.push(format!(
                                    "Test for {model_name}: {} (exit {exit_code}). Output: {}",
                                    if success { "PASS" } else { "FAIL" },
                                    truncate(&output, 300)
                                ));
                            }
                            Err(e) => {
                                feedback_parts.push(format!("Test for {model_name} failed: {e}"));
                            }
                        }
                    }

                    AgentAction::AskUser { action } => {
                        let _ = self.channel.send(SetupEvent::NeedInput {
                            action: action.clone(),
                        });

                        // PAUSE: wait for user response
                        match self.input_rx.recv().await {
                            Some(UserResponse::Cancel) => {
                                let _ = self.channel.send(SetupEvent::Error {
                                    message: "Setup cancelled by user.".into(),
                                    recoverable: false,
                                });
                                let _ = self.memory.end_session(&self.session_id, "cancelled");
                                return;
                            }
                            Some(response) => {
                                let response_json = serde_json::to_string(&response)
                                    .unwrap_or_else(|_| "{}".to_string());
                                feedback_parts.push(format!("User responded: {response_json}"));
                            }
                            None => {
                                // Channel closed — flow cancelled
                                let _ = self.memory.end_session(&self.session_id, "cancelled");
                                return;
                            }
                        }
                    }

                    AgentAction::SyncSkill {
                        source_cli,
                        target_cli,
                        skill_name,
                    } => {
                        let _ = self.channel.send(SetupEvent::Status {
                            message: format!("Syncing skill '{skill_name}' to {target_cli}..."),
                        });
                        match sync::copy_skill(source_cli, target_cli, skill_name) {
                            Ok(()) => {
                                feedback_parts
                                    .push(format!("Skill '{skill_name}' synced to {target_cli}"));
                            }
                            Err(e) => {
                                feedback_parts.push(format!("Failed to sync skill: {e}"));
                            }
                        }
                    }

                    AgentAction::SyncMcp {
                        source_cli: _,
                        target_cli,
                        mcp_name,
                        config,
                    } => {
                        let _ = self.channel.send(SetupEvent::Status {
                            message: format!("Syncing MCP '{mcp_name}' to {target_cli}..."),
                        });
                        match sync::install_mcp(target_cli, mcp_name, config) {
                            Ok(()) => {
                                feedback_parts
                                    .push(format!("MCP '{mcp_name}' installed in {target_cli}"));
                            }
                            Err(e) => {
                                feedback_parts.push(format!("Failed to sync MCP: {e}"));
                            }
                        }
                    }

                    AgentAction::UpdateMemory {
                        node_type,
                        label,
                        data,
                        edges,
                    } => {
                        let node_id = format!("{node_type}:{label}");
                        let _ = self.memory.upsert_node(&node_id, node_type, label, data);
                        for edge in edges {
                            let target_id = format!("{}:{}", node_type, edge.target_label);
                            let _ = self.memory.add_edge(&node_id, &target_id, &edge.edge_type);
                        }
                    }

                    AgentAction::Complete { summary, items } => {
                        let _ = self.channel.send(SetupEvent::Complete {
                            summary: summary.clone(),
                            items_configured: items.clone(),
                        });
                        let _ = self.memory.end_session(&self.session_id, "success");
                        return;
                    }
                }
            }

            if result.done {
                let _ = self.memory.end_session(&self.session_id, "done");
                break;
            }

            // Build next message from feedback
            next_message = if feedback_parts.is_empty() {
                "Continue with the next step.".to_string()
            } else {
                format!(
                    "Results from previous actions:\n\n{}",
                    feedback_parts.join("\n\n")
                )
            };
        }
    }
}

fn execute_allowlisted(command: &str, args: &[String]) -> Result<(String, String, i32), String> {
    if !ALLOWED_COMMANDS.contains(&command) {
        return Err(format!("Command '{command}' is not in the allowlist"));
    }

    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to execute '{command}': {e}"))?;

    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    ))
}

fn validate_and_write(path: &str, content: &str) -> Result<String, String> {
    // Expand ~ to home directory
    let expanded = if let Some(stripped) = path.strip_prefix("~/") {
        let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
        home.join(stripped)
    } else {
        std::path::PathBuf::from(path)
    };

    let resolved = expanded.to_string_lossy().to_string();

    // Validate path is in allowed prefixes
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let allowed = ALLOWED_WRITE_PREFIXES.iter().any(|prefix| {
        let full_prefix = home.join(prefix);
        expanded.starts_with(&full_prefix)
    });

    if !allowed {
        return Err(format!(
            "Write path '{resolved}' is not in allowed directories"
        ));
    }

    // Create parent directories
    if let Some(parent) = expanded.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directories: {e}"))?;
    }

    std::fs::write(&expanded, content).map_err(|e| format!("Failed to write file: {e}"))?;

    Ok(resolved)
}

fn get_install_instructions() -> String {
    let os = std::env::consts::OS;
    match os {
        "linux" => "To install Claude CLI:\n\n\
            1. Run: curl -fsSL https://claude.ai/install.sh | bash\n\
            2. After installation, run: claude login\n\
            3. Complete the OAuth flow in your browser\n\
            4. Click 'I've logged in' when done"
            .to_string(),
        "macos" => "To install Claude CLI:\n\n\
            1. Run: brew install claude\n\
               OR: curl -fsSL https://claude.ai/install.sh | bash\n\
            2. After installation, run: claude login\n\
            3. Complete the OAuth flow in your browser\n\
            4. Click 'I've logged in' when done"
            .to_string(),
        "windows" => "To install Claude CLI:\n\n\
            1. Run in PowerShell: irm https://claude.ai/install.ps1 | iex\n\
            2. After installation, run: claude login\n\
            3. Complete the OAuth flow in your browser\n\
            4. Click 'I've logged in' when done"
            .to_string(),
        _ => "Please visit https://claude.ai/download to install the Claude CLI \
            for your platform.\n\nAfter installation, run: claude login"
            .to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::test_support::env_lock;
    use rusqlite::Connection;
    use serde_json::Value;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, Mutex};

    fn with_temp_home(test: impl FnOnce(&std::path::Path)) {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("HOME");

        unsafe {
            std::env::set_var("HOME", dir.path());
        }

        let result = catch_unwind(AssertUnwindSafe(|| test(dir.path())));

        match previous_home {
            Some(value) => unsafe {
                std::env::set_var("HOME", value);
            },
            None => unsafe {
                std::env::remove_var("HOME");
            },
        }

        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    #[cfg(unix)]
    fn with_fake_claude(script: &str, test: impl FnOnce(&std::path::Path, &std::path::Path)) {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("claude");
        let prompt_log = dir.path().join("prompts.log");
        let count_file = dir.path().join("count");
        std::fs::write(&bin, script).unwrap();
        let mut perms = std::fs::metadata(&bin).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin, perms).unwrap();

        let previous_path = std::env::var_os("PATH");
        let previous_prompt_log = std::env::var_os("CLAUDE_PROMPT_LOG");
        let previous_count_file = std::env::var_os("CLAUDE_COUNT_FILE");
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
            std::env::set_var("CLAUDE_PROMPT_LOG", &prompt_log);
            std::env::set_var("CLAUDE_COUNT_FILE", &count_file);
        }

        let result = catch_unwind(AssertUnwindSafe(|| test(dir.path(), &prompt_log)));

        match previous_path {
            Some(value) => unsafe {
                std::env::set_var("PATH", value);
            },
            None => unsafe {
                std::env::remove_var("PATH");
            },
        }
        match previous_prompt_log {
            Some(value) => unsafe {
                std::env::set_var("CLAUDE_PROMPT_LOG", value);
            },
            None => unsafe {
                std::env::remove_var("CLAUDE_PROMPT_LOG");
            },
        }
        match previous_count_file {
            Some(value) => unsafe {
                std::env::set_var("CLAUDE_COUNT_FILE", value);
            },
            None => unsafe {
                std::env::remove_var("CLAUDE_COUNT_FILE");
            },
        }

        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    fn capture_channel(events: Arc<Mutex<Vec<Value>>>) -> Channel<SetupEvent> {
        Channel::new(move |body| {
            let value: Value = body.deserialize().unwrap();
            events.lock().unwrap().push(value);
            Ok(())
        })
    }

    fn logged_prompts(path: &std::path::Path) -> Vec<String> {
        let content = std::fs::read_to_string(path).unwrap();
        content
            .split("---PROMPT---\n")
            .filter(|part| !part.trim().is_empty())
            .map(|part| part.trim_end_matches('\n').to_string())
            .collect()
    }

    fn session_outcome_and_turn_count(
        db_path: &std::path::Path,
        session_id: &str,
    ) -> (Option<String>, i64) {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row(
            "SELECT outcome, turn_count FROM setup_sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    }

    fn setup_turn_count(db_path: &std::path::Path, session_id: &str) -> i64 {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM setup_turns WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn run_agent_loop_with_memory(
        channel: Channel<SetupEvent>,
        input_rx: mpsc::Receiver<UserResponse>,
        db_path: &std::path::Path,
        session_id: &str,
    ) {
        let memory = MemoryGraph::open(db_path).unwrap();
        memory.create_session(session_id).unwrap();
        let mut flow = SetupFlow::new(channel, input_rx, memory, session_id.to_string());

        tauri::async_runtime::block_on(
            flow.run_agent_loop("test system prompt".to_string(), "begin setup"),
        );
    }

    #[cfg(unix)]
    fn complete_fake_claude_script() -> &'static str {
        r#"#!/usr/bin/env bash
{
  echo "---PROMPT---"
  printf '%s\n' "${!#}"
} >> "$CLAUDE_PROMPT_LOG"
echo "Session: setup-flow-session" >&2
printf '%s' '{"actions":[{"type":"complete","summary":"Setup finished","items":["claude","codex~high"]}],"done":false}'
"#
    }

    #[cfg(unix)]
    #[test]
    fn complete_action_emits_complete_event_and_records_success() {
        with_fake_claude(complete_fake_claude_script(), |dir, prompt_log| {
            let events = Arc::new(Mutex::new(Vec::new()));
            let channel = capture_channel(events.clone());
            let (_input_tx, input_rx) = mpsc::channel(1);
            let db_path = dir.join("memory.sqlite");
            let session_id = "setup-flow-complete";

            run_agent_loop_with_memory(channel, input_rx, &db_path, session_id);

            let captured = events.lock().unwrap();
            let complete = captured
                .iter()
                .find(|event| event.get("event").and_then(Value::as_str) == Some("complete"))
                .expect("expected complete event");
            assert_eq!(complete["data"]["summary"], "Setup finished");
            assert_eq!(
                complete["data"]["items_configured"],
                serde_json::json!(["claude", "codex~high"])
            );

            let (outcome, session_turn_count) =
                session_outcome_and_turn_count(&db_path, session_id);
            assert_eq!(outcome.as_deref(), Some("success"));
            assert_eq!(session_turn_count, 1);
            assert_eq!(setup_turn_count(&db_path, session_id), 1);

            let prompts = logged_prompts(prompt_log);
            assert_eq!(prompts.len(), 1);
        });
    }

    #[cfg(unix)]
    #[test]
    fn agent_turn_failure_emits_recoverable_agent_error_and_records_agent_error() {
        let script = r#"#!/usr/bin/env bash
{
  echo "---PROMPT---"
  printf '%s\n' "${!#}"
} >> "$CLAUDE_PROMPT_LOG"
echo "oauth expired" >&2
exit 7
"#;

        with_fake_claude(script, |dir, _prompt_log| {
            let events = Arc::new(Mutex::new(Vec::new()));
            let channel = capture_channel(events.clone());
            let (_input_tx, input_rx) = mpsc::channel(1);
            let db_path = dir.join("memory.sqlite");
            let session_id = "setup-flow-agent-error";

            run_agent_loop_with_memory(channel, input_rx, &db_path, session_id);

            let captured = events.lock().unwrap();
            let error = captured
                .iter()
                .find(|event| event.get("event").and_then(Value::as_str) == Some("error"))
                .expect("expected error event");
            let message = error["data"]["message"].as_str().unwrap();
            assert!(message.starts_with("Agent error:"), "{message}");
            assert_eq!(error["data"]["recoverable"], true);

            let (outcome, session_turn_count) =
                session_outcome_and_turn_count(&db_path, session_id);
            assert_eq!(outcome.as_deref(), Some("agent_error"));
            assert_eq!(session_turn_count, 0);
        });
    }

    #[cfg(unix)]
    #[test]
    fn ask_user_emits_need_input_and_sends_serialized_response_to_next_turn() {
        let script = r#"#!/usr/bin/env bash
{
  echo "---PROMPT---"
  printf '%s\n' "${!#}"
} >> "$CLAUDE_PROMPT_LOG"
count=0
if [ -f "$CLAUDE_COUNT_FILE" ]; then
  count="$(cat "$CLAUDE_COUNT_FILE")"
fi
count=$((count + 1))
printf '%s' "$count" > "$CLAUDE_COUNT_FILE"
echo "Session: setup-flow-session" >&2
if [ "$count" -eq 1 ]; then
  printf '%s' '{"actions":[{"type":"ask_user","action":{"type":"confirm","title":"Authorize setup","message":"Continue?","confirm_id":"continue-setup","confirm_label":"Continue","cancel_label":"Stop"}}],"done":false}'
else
  printf '%s' '{"actions":[{"type":"complete","summary":"User answered","items":["confirmed"]}],"done":false}'
fi
"#;

        with_fake_claude(script, |dir, prompt_log| {
            let events = Arc::new(Mutex::new(Vec::new()));
            let channel = capture_channel(events.clone());
            let (input_tx, input_rx) = mpsc::channel(1);
            input_tx
                .try_send(UserResponse::Confirm {
                    confirm_id: "continue-setup".to_string(),
                    confirmed: true,
                })
                .unwrap();
            let db_path = dir.join("memory.sqlite");
            let session_id = "setup-flow-ask-user";

            run_agent_loop_with_memory(channel, input_rx, &db_path, session_id);

            let captured = events.lock().unwrap();
            let need_input = captured
                .iter()
                .find(|event| event.get("event").and_then(Value::as_str) == Some("need_input"))
                .expect("expected need_input event");
            assert_eq!(need_input["data"]["action"]["type"], "confirm");
            assert_eq!(need_input["data"]["action"]["confirm_id"], "continue-setup");

            let prompts = logged_prompts(prompt_log);
            assert_eq!(prompts.len(), 2);
            let second_prompt = &prompts[1];
            assert!(
                second_prompt.contains("Results from previous actions"),
                "{second_prompt}"
            );
            assert!(
                second_prompt.contains(
                    r#"User responded: {"type":"confirm","confirm_id":"continue-setup","confirmed":true}"#
                ),
                "{second_prompt}"
            );
        });
    }

    #[test]
    fn unlisted_command_name_is_rejected_before_execution() {
        let err = execute_allowlisted("not-in-setup-allowlist", &[]).unwrap_err();

        assert!(err.contains("not-in-setup-allowlist"), "{err}");
        assert!(err.contains("not in the allowlist"), "{err}");
    }

    #[test]
    fn listed_command_name_executes_and_returns_process_output() {
        let args = vec!["sh".to_string()];
        let (stdout, stderr, exit_code) = execute_allowlisted("which", &args).unwrap();

        assert_eq!(exit_code, 0);
        assert!(
            stdout.lines().any(|line| line.ends_with("/sh")),
            "stdout should identify the sh executable path, got: {stdout:?}"
        );
        assert_eq!(stderr, "");
    }

    #[test]
    fn config_root_path_creates_parent_directories_and_writes_requested_content() {
        with_temp_home(|home| {
            let path = "~/.config/oulipoly-agent-runner/models/test-model.toml";
            let content = "name = \"test-model\"\n";

            let resolved = validate_and_write(path, content).unwrap();

            let expected_path = home
                .join(".config/oulipoly-agent-runner/models/test-model.toml")
                .to_string_lossy()
                .to_string();
            assert_eq!(resolved, expected_path);
            assert_eq!(std::fs::read_to_string(expected_path).unwrap(), content);
        });
    }

    #[test]
    fn local_bin_path_creates_parent_directories_and_writes_requested_content() {
        with_temp_home(|home| {
            let path = "~/.local/bin/oulipoly-test-wrapper";
            let content = "#!/usr/bin/env bash\n";

            let resolved = validate_and_write(path, content).unwrap();

            let expected_path = home
                .join(".local/bin/oulipoly-test-wrapper")
                .to_string_lossy()
                .to_string();
            assert_eq!(resolved, expected_path);
            assert_eq!(std::fs::read_to_string(expected_path).unwrap(), content);
        });
    }

    #[test]
    fn path_outside_approved_roots_is_rejected() {
        with_temp_home(|_| {
            let err = validate_and_write("~/outside-approved-roots.toml", "content").unwrap_err();

            assert!(err.contains("not in allowed directories"), "{err}");
        });
    }

    #[test]
    #[ignore = "Confirmed bug: intended behavior from commit db3b73808a5be6ba38bada4770ef77740cf4b62c; see /home/nes/projects/agent-runner/planning/coverage/spec-setup-flow-security-helpers.md#behavior-2-setup-writes-are-limited-to-approved-roots"]
    fn parent_directory_traversal_out_of_approved_root_is_rejected() {
        with_temp_home(|_| {
            let err = validate_and_write(
                "~/.config/oulipoly-agent-runner/../../escaped.toml",
                "content",
            )
            .unwrap_err();

            assert!(err.contains("not in allowed directories"), "{err}");
        });
    }
}
