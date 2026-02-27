use super::actions::{AgentAction, ResultContent, SetupEvent, UserResponse};
use super::agent::SetupAgent;
use super::context;
use super::detection;
use super::memory::MemoryGraph;
use super::schemas::AGENT_TURN_SCHEMA;
use super::sync;
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
        }
    }

    pub async fn run(mut self) {
        // Record session start
        let _ = self.memory.create_session(&self.session_id);

        // 1. Detection phase
        let _ = self.channel.send(SetupEvent::Status {
            message: "Detecting installed CLIs...".into(),
        });

        let report = detection::detect_all();
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
                    let new_report = detection::detect_all();
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

        let cli_info = detection::detect_single_cli(cli_name);
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

            let result = match agent.send_turn(&next_message, AGENT_TURN_SCHEMA) {
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
