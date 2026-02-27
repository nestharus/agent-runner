use super::detection::DetectionReport;
use super::memory::MemoryGraph;

pub struct AgentContext {
    pub detection_json: String,
    pub memory_json: String,
}

const CAPABILITIES: &str = r#"You communicate by returning a JSON object with an "actions" array and a "done" boolean. Each action is executed sequentially by the orchestrator. Available action types:

### status
Emit a status message displayed to the user with a spinner.
```json
{{"type": "status", "message": "Detecting installed CLIs..."}}
```

### run_command
Execute a shell command. Only these commands are allowed: which, type, claude, codex, opencode, gemini, npm, npx, curl, bash.
```json
{{"type": "run_command", "command": "claude", "args": ["-p", "say hello", "--output-format", "json"], "description": "Testing Claude CLI"}}
```

### write_config
Write a configuration file. Only paths under ~/.config/oulipoly-agent-runner/ or ~/.local/bin/ are allowed.
```json
{{"type": "write_config", "path": "~/.config/oulipoly-agent-runner/models/claude-sonnet.toml", "content": "command = \"claude\"\nargs = [\"-p\", \"--model\", \"sonnet\"]\nprompt_mode = \"stdin\"", "description": "Creating Claude Sonnet model config"}}
```

### test_integration
Test a model integration by running a command and checking output.
```json
{{"type": "test_integration", "model_name": "claude-sonnet", "command": "claude", "args": ["-p", "say hello", "--model", "sonnet", "--output-format", "json"]}}
```

### ask_user
Request user input. The orchestrator will pause and show a UI element. Types: form, wizard, confirm, oauth_flow, api_key_entry, cli_selection.
```json
{{"type": "ask_user", "action": {{"type": "form", "title": "Configure Model", "form_id": "model-config", "fields": [{{"name": "model_name", "label": "Model Name", "field_type": "text", "required": true}}]}}}}
```

### sync_skill
Copy a skill from one CLI to another.
```json
{{"type": "sync_skill", "source_cli": "claude", "target_cli": "codex", "skill_name": "code-review"}}
```

### sync_mcp
Install an MCP server configuration in a CLI.
```json
{{"type": "sync_mcp", "source_cli": "claude", "target_cli": "codex", "mcp_name": "firecrawl", "config": "{{\"command\": \"npx\", \"args\": [\"firecrawl-mcp\"]}}"}}
```

### update_memory
Store information in the memory graph for future sessions.
```json
{{"type": "update_memory", "node_type": "cli", "label": "claude", "data": "{{\"version\": \"1.0\", \"installed\": true}}", "edges": [{{"target_label": "opus", "edge_type": "uses_model"}}]}}
```

### complete
Signal that setup is done.
```json
{{"type": "complete", "summary": "Setup complete! Configured 3 models.", "items": ["claude-sonnet", "claude-opus", "codex-high"]}}
```"#;

const RULES: &str = r#"## Rules

1. Always emit a "status" action before doing work so the user sees progress
2. Use "ask_user" when you need input — never assume
3. Use "update_memory" to remember what you've configured for future sessions
4. Use "test_integration" to verify configurations work before completing
5. Model configs are TOML files in ~/.config/oulipoly-agent-runner/models/
6. Model TOML format: command, args (array), prompt_mode ("stdin" or "arg"), optionally [[providers]] for multi-provider
7. Agent configs are Markdown files with YAML frontmatter in ~/.config/oulipoly-agent-runner/agents/
8. When setup is complete, emit a "complete" action"#;

pub fn build_agent_context(report: &DetectionReport, memory: &MemoryGraph) -> AgentContext {
    let detection_json = serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string());

    let memory_snapshot = memory
        .subgraph_for_context(&[
            "cli",
            "model",
            "provider",
            "wrapper",
            "skill",
            "mcp",
            "preference",
        ])
        .ok();
    let memory_json = memory_snapshot
        .map(|s| serde_json::to_string_pretty(&s).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());

    AgentContext {
        detection_json,
        memory_json,
    }
}

pub fn build_system_prompt(context: &AgentContext) -> String {
    format!(
        r#"You are a setup agent for the Oulipoly Agent Runner desktop application. Your role is to detect, install, configure, and troubleshoot CLI tools that the application uses to route LLM prompts.

## Your Capabilities

{capabilities}

{rules}

## Current System State

### Detected CLIs
{detection}

### Memory Graph (from previous sessions)
{memory}

## Your Task

Analyze the system state above. For each detected CLI:
1. Verify it works (test with a simple command)
2. Check authentication status
3. Create model configurations for the application
4. Discover and offer to sync skills/MCPs across CLIs
5. Test each configuration

If no CLIs are detected, guide the user to install at least one (recommend Claude CLI).
If CLIs are detected but not authenticated, guide the user through authentication.
"#,
        capabilities = CAPABILITIES,
        rules = RULES,
        detection = context.detection_json,
        memory = context.memory_json,
    )
}

pub fn build_cli_setup_prompt(cli_name: &str, context: &AgentContext) -> String {
    format!(
        r#"You are a setup agent for the Oulipoly Agent Runner desktop application. The user wants to add the `{cli_name}` CLI. Help them install it, authenticate, create a model configuration, and test it.

## Your Capabilities

{capabilities}

{rules}

## Current System State

### CLI Detection
{detection}

### Memory Graph (from previous sessions)
{memory}

## Your Task

Focus on setting up the `{cli_name}` CLI:
1. Check if `{cli_name}` is installed — if not, guide the user through installation
2. Verify authentication — if not authenticated, guide through auth setup
3. Create model configuration(s) for this CLI
4. Test the configuration to ensure it works
5. Complete when the CLI is ready to use
"#,
        cli_name = cli_name,
        capabilities = CAPABILITIES,
        rules = RULES,
        detection = context.detection_json,
        memory = context.memory_json,
    )
}
