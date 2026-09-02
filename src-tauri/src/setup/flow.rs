use oulipoly_config::app::SetupBrainConfig;
use oulipoly_setup::actions::{
    AgentAction, AgentTurnResult, ResultContent, SetupEvent, UserResponse,
};
use oulipoly_setup::context;
use oulipoly_setup::detection;
use oulipoly_setup::memory::MemoryGraph;
use oulipoly_setup::schemas::AGENT_TURN_SCHEMA;
use oulipoly_setup::sync;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

use super::setup_brain_host::{self, SetupBrainHost};
use super::setup_provider_ops;

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
    setup_brain: Result<Option<SetupBrainConfig>, String>,
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
            setup_brain: crate::try_load_app_config().map(|config| config.setup.brain),
        }
    }

    #[allow(dead_code)]
    pub fn new_with_setup_brain(
        channel: Channel<SetupEvent>,
        input_rx: mpsc::Receiver<UserResponse>,
        memory: MemoryGraph,
        session_id: String,
        setup_brain: Option<SetupBrainConfig>,
    ) -> Self {
        SetupFlow {
            channel,
            input_rx,
            memory,
            session_id,
            setup_brain: Ok(setup_brain),
        }
    }

    pub async fn run(mut self) {
        let _ = self.memory.create_session(&self.session_id);
        let _ = self.channel.send(SetupEvent::Status {
            message: "Loading setup provider...".into(),
        });
        let agent_context = endpoint_agent_context(&self.memory);
        let system_prompt = context::build_system_prompt(&agent_context);
        self.try_run_configured_setup_brain(
            &system_prompt,
            "Analyze the system state and begin setup.",
        )
        .await;
    }

    pub async fn run_for_cli(mut self, cli_name: &str) {
        let _ = self.memory.create_session(&self.session_id);

        let _ = self.channel.send(SetupEvent::Status {
            message: format!("Loading setup provider for {cli_name}..."),
        });
        let agent_context = endpoint_agent_context(&self.memory);
        let system_prompt = context::build_cli_setup_prompt(cli_name, &agent_context);
        let initial_message = format!("Help set up the {} CLI.", cli_name);
        self.try_run_configured_setup_brain(&system_prompt, &initial_message)
            .await;
    }

    async fn try_run_configured_setup_brain(
        &mut self,
        system_prompt: &str,
        initial_message: &str,
    ) -> bool {
        let setup_brain = match self.setup_brain.clone() {
            Ok(setup_brain) => setup_brain,
            Err(error) => {
                let _ = self.channel.send(SetupEvent::Error {
                    message: format!("Setup brain config error: {error}"),
                    recoverable: false,
                });
                let _ = self.memory.end_session(&self.session_id, "agent_error");
                return true;
            }
        };

        match require_setup_brain(setup_brain) {
            Ok(setup_brain) => {
                let endpoint = match setup_brain_host::setup_bootstrap_endpoint(&setup_brain) {
                    Ok(endpoint) => endpoint,
                    Err(error) => {
                        let _ = self.channel.send(SetupEvent::Error {
                            message: format!("Setup brain error: {error}"),
                            recoverable: error.recoverable,
                        });
                        let _ = self.memory.end_session(&self.session_id, "agent_error");
                        return true;
                    }
                };
                let setup_operations =
                    setup_provider_ops::build_setup_provider_context_with_endpoint(
                        &setup_brain,
                        endpoint.clone(),
                    );
                for diagnostic in &setup_operations.diagnostics {
                    let _ = self.channel.send(SetupEvent::Status {
                        message: format!(
                            "Setup provider operation '{}' reported {}.",
                            diagnostic.operation, diagnostic.kind
                        ),
                    });
                }
                match SetupBrainHost::from_endpoint(
                    setup_brain,
                    endpoint,
                    system_prompt.to_string(),
                    setup_operations.context,
                ) {
                    Ok(mut host) => {
                        self.run_configured_agent_loop(&mut host, initial_message)
                            .await;
                    }
                    Err(error) => {
                        let _ = self.channel.send(SetupEvent::Error {
                            message: format!("Setup brain error: {error}"),
                            recoverable: error.recoverable,
                        });
                        let _ = self.memory.end_session(&self.session_id, "agent_error");
                    }
                }
                true
            }
            Err(error) => {
                let _ = self.channel.send(SetupEvent::Error {
                    message: format!("Setup brain error: {error}"),
                    recoverable: false,
                });
                let _ = self.memory.end_session(&self.session_id, "agent_error");
                true
            }
        }
    }

    async fn run_configured_agent_loop(
        &mut self,
        host: &mut SetupBrainHost,
        initial_message: &str,
    ) {
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

            let result = match host.send_turn(&next_message, AGENT_TURN_SCHEMA) {
                Ok(result) => result,
                Err(error) => {
                    let _ = self.channel.send(SetupEvent::Error {
                        message: format!("Agent error: {error}"),
                        recoverable: error.recoverable,
                    });
                    let _ = self.memory.end_session(&self.session_id, "agent_error");
                    break;
                }
            };

            let Some(feedback) = self
                .process_agent_turn_result(&result, turn_number, &next_message)
                .await
            else {
                break;
            };
            if result.done {
                let _ = self.memory.end_session(&self.session_id, "done");
                break;
            }
            next_message = feedback;
        }
    }

    async fn process_agent_turn_result(
        &mut self,
        result: &AgentTurnResult,
        turn_number: i32,
        next_message: &str,
    ) -> Option<String> {
        let actions_summary = format!("{} actions processed", result.actions.len());
        let _ = self.memory.record_turn(
            &self.session_id,
            turn_number,
            next_message,
            &actions_summary,
            "[]",
        );

        let mut feedback_parts: Vec<String> = Vec::new();
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
                            return None;
                        }
                        Some(response) => {
                            let response_json = serde_json::to_string(&response)
                                .unwrap_or_else(|_| "{}".to_string());
                            feedback_parts.push(format!("User responded: {response_json}"));
                        }
                        None => {
                            // Channel closed — flow cancelled
                            let _ = self.memory.end_session(&self.session_id, "cancelled");
                            return None;
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
                    return None;
                }
            }
        }

        Some(if feedback_parts.is_empty() {
            "Continue with the next step.".to_string()
        } else {
            format!(
                "Results from previous actions:\n\n{}",
                feedback_parts.join("\n\n")
            )
        })
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub async fn process_agent_turn_result_for_test(
        &mut self,
        result: &AgentTurnResult,
        turn_number: i32,
        next_message: &str,
    ) -> Option<String> {
        self.process_agent_turn_result(result, turn_number, next_message)
            .await
    }
}

fn endpoint_agent_context(memory: &MemoryGraph) -> context::AgentContext {
    let report = detection::DetectionReport {
        clis: Vec::new(),
        os: detection::OsInfo {
            os_type: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        },
        wrappers: Vec::new(),
    };
    let mut context = context::build_agent_context(&report, memory);
    context.detection_json =
        "Provider-owned setup detection is supplied in the setup turn context.".to_string();
    context
}

fn require_setup_brain(setup: Option<SetupBrainConfig>) -> Result<SetupBrainConfig, &'static str> {
    setup.ok_or_else(setup_brain_host::setup_brain_not_configured)
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

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
