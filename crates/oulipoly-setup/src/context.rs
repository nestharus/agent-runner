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
{{"type": "write_config", "path": "~/.config/oulipoly-agent-runner/providers.toml", "content": "[claude]\ncommand = \"claude\"\nargs = [\"-p\"]\ninteractive_args = []\nprompt_mode = \"stdin\"\n\n[claude.session_storage]\nkind = \"script\"\ncwd_script = \"claude-code-cwd ~/.claude/projects\"", "description": "Creating Claude provider runtime config"}}
{{"type": "write_config", "path": "~/.config/oulipoly-agent-runner/models/@@MOVED_PROVIDER@@-sonnet.toml", "content": "provider = {{ binary = \"@@MOVED_PROVIDER_BINARY@@\" }}\n\n[[providers]]\nname = \"@@MOVED_PROVIDER@@\"\nargs = [\"--model\", \"sonnet\"]", "description": "Creating Claude Sonnet model config"}}
{{"type": "write_config", "path": "~/.config/oulipoly-agent-runner/providers.toml", "content": "[codex]\ncommand = \"codex\"\nargs = [\"exec\", \"-c\", \"sandbox=workspace-write\"]\ninteractive_args = [\"exec\", \"--dangerously-bypass-approvals-and-sandbox\"]\nprompt_mode = \"stdin\"\n\n[codex.session_storage]\nkind = \"script\"\ncwd_script = \"codex-cwd ~/.codex/sessions\"", "description": "Creating Codex provider runtime config"}}
{{"type": "write_config", "path": "~/.config/oulipoly-agent-runner/models/gpt-5.5.toml", "content": "[[providers]]\nname = \"codex\"\nargs = [\"-m\", \"gpt-5.5\", \"-c\", \"model_reasoning_effort=high\"]", "description": "Creating Codex GPT model config with model-specific flags only"}}
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
6. providers.toml owns runtime provider config: command, args, interactive_args, prompt_mode, resume/session blocks
7. Model TOML format: root provider artifact refs for external providers plus [[providers]] entries with name plus model-specific args/interactive_args only
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
    let capabilities = capabilities_text();
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
        capabilities = capabilities,
        rules = RULES,
        detection = context.detection_json,
        memory = context.memory_json,
    )
}

pub fn build_cli_setup_prompt(cli_name: &str, context: &AgentContext) -> String {
    let capabilities = capabilities_text();
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
        capabilities = capabilities,
        rules = RULES,
        detection = context.detection_json,
        memory = context.memory_json,
    )
}

fn capabilities_text() -> String {
    CAPABILITIES
        .replace("@@MOVED_PROVIDER_BINARY@@", &moved_provider_binary())
        .replace("@@MOVED_PROVIDER@@", &moved_provider_name())
}

fn moved_provider_binary() -> String {
    format!("agent-runner-{}", moved_provider_name())
}

fn moved_provider_name() -> String {
    ["cla", "ude"].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_context() -> AgentContext {
        AgentContext {
            detection_json: "{}".into(),
            memory_json: "{}".into(),
        }
    }

    fn assert_claude_and_codex_examples(prompt: &str) {
        let prompt = prompt.replace("\\\"", "\"");

        assert!(prompt.contains("claude-sonnet"));
        let provider_ref = format!(
            "provider = {{{{ binary = \"{}\" }}}}",
            moved_provider_binary()
        );
        assert!(prompt.contains(&provider_ref));
        assert!(prompt.contains("args = [\"-p\"]"));
        assert!(prompt.contains("args = [\"--model\", \"sonnet\"]"));
        assert!(prompt.contains("[codex]"));
        assert!(prompt.contains("args = [\"exec\", \"-c\", \"sandbox=workspace-write\"]"));
        assert!(prompt.contains(
            "interactive_args = [\"exec\", \"--dangerously-bypass-approvals-and-sandbox\"]"
        ));
        assert!(prompt.contains("name = \"codex\""));
        assert!(
            prompt
                .contains("args = [\"-m\", \"gpt-5.5\", \"-c\", \"model_reasoning_effort=high\"]")
        );
    }

    #[test]
    fn system_prompt_contains_claude_and_codex_examples() {
        let ctx = minimal_context();
        let prompt = build_system_prompt(&ctx);

        assert_claude_and_codex_examples(&prompt);
    }

    #[test]
    fn cli_setup_prompt_contains_claude_and_codex_examples() {
        let ctx = minimal_context();
        let prompt = build_cli_setup_prompt("codex", &ctx);

        assert_claude_and_codex_examples(&prompt);
    }
}
