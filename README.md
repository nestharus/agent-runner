# oulipoly-agent-runner

CLI agent runner with load balancing across LLM providers. Routes prompts to CLI tools like `claude`, `codex`, `opencode`, etc. with automatic failover, error diagnostics, and persistent state tracking.

## Install

```bash
cargo install --path .
```

Or grab a binary from [Releases](https://github.com/nestharus/agent-runner/releases).

## Quick Start

```bash
# Run a prompt against a model
oulipoly-agent-runner --model claude-haiku "Explain monads in one sentence"

# Run an agent (model + system instructions)
oulipoly-agent-runner my-agent "Fix the login bug"

# Pipe prompt from stdin
cat spec.md | oulipoly-agent-runner --model codex-high

# Read prompt from file
oulipoly-agent-runner --model glm --file prompt.md
```

## Configuration

All config lives in `~/.config/oulipoly-agent-runner/`:

```
~/.config/oulipoly-agent-runner/
  config.toml          # Global settings
  models/              # Model configs (one .toml per model)
  agents/              # Agent configs (one .md per agent)
```

### Adding a Model

Create a `.toml` file in `~/.config/oulipoly-agent-runner/models/`. The filename becomes the model name.

**Single provider:**

```toml
# ~/.config/oulipoly-agent-runner/models/claude-haiku.toml
command = "claude"
args = ["-p", "--model", "haiku"]
prompt_mode = "stdin"
```

**Multiple providers (load balanced):**

```toml
# ~/.config/oulipoly-agent-runner/models/codex-high.toml
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec", "-m", "gpt-5.3-codex"]

[[providers]]
command = "codex2"
args = ["exec", "-m", "gpt-5.3-codex"]
```

With multiple providers, the runner automatically alternates between them and avoids providers with recent errors.

#### Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `command` | Yes (single) | - | CLI command to execute |
| `args` | No | `[]` | Arguments passed to the command |
| `prompt_mode` | No | `"stdin"` | `"stdin"` pipes prompt to stdin, `"arg"` appends as final argument |
| `[[providers]]` | Yes (multi) | - | List of provider configs (each has `command` and `args`) |

Use `prompt_mode = "stdin"` for CLIs that read from stdin (e.g., `claude -p`).
Use `prompt_mode = "arg"` for CLIs that take the prompt as an argument (e.g., `codex exec`).

### Adding an Agent

Create a `.md` file in `~/.config/oulipoly-agent-runner/agents/`. The filename becomes the agent name.

```markdown
---
description: 'Code review assistant'
model: claude-haiku
output_format: ''
---

You are a senior code reviewer. Review the following code for:
- Security vulnerabilities
- Performance issues
- Readability concerns

Be concise and actionable.
```

#### Frontmatter Fields

| Field | Required | Description |
|-------|----------|-------------|
| `description` | No | Human-readable description of the agent |
| `model` | Yes | Model name (must match a `.toml` in models/) |
| `output_format` | No | Reserved for future use |

Everything after the `---` closing delimiter is the agent's system instructions, prepended to every prompt.

### Global Config

```toml
# ~/.config/oulipoly-agent-runner/config.toml

# Model to use for analyzing errors when a provider fails
diagnostics_model = "claude-haiku"
```

When a provider returns a non-zero exit code, the runner pipes stderr to the diagnostics model to classify the error (rate limit, auth expired, quota exhausted, etc.). This influences future load balancing decisions.

## CLI Reference

```
oulipoly-agent-runner [OPTIONS] [AGENT] [PROMPT...]

Arguments:
  [AGENT]       Agent name (from agents directory)
  [PROMPT...]   Prompt text (remaining arguments joined)

Options:
  -m, --model <MODEL>            Execute a model directly (no agent)
  -a, --agent-file <AGENT_FILE>  Path to an agent .md file (any location)
  -f, --file <FILE>              Read prompt from file
  -p, --project <PROJECT>        Working directory for subprocess
      --models-dir <MODELS_DIR>  Override models directory
      --agents-dir <AGENTS_DIR>  Override agents directory
  -h, --help                     Print help
```

**Prompt resolution priority:** `--file` > positional arguments > stdin

### Examples

```bash
# Direct model execution
oulipoly-agent-runner --model glm "List 3 sorting algorithms"

# Named agent
oulipoly-agent-runner code-reviewer "Review this function"

# Agent file from any path
oulipoly-agent-runner --agent-file ./my-agent.md --model claude-haiku "Do the thing"

# Set working directory for the subprocess
oulipoly-agent-runner --model codex-high -p /path/to/repo "Fix the tests"

# Use custom model directory
oulipoly-agent-runner --models-dir ./my-models --model local "Hello"
```

## Load Balancing

Models with multiple `[[providers]]` are automatically load balanced:

- **Round-robin**: Picks the provider with the fewest total invocations
- **Error avoidance**: Providers with 3+ errors in the last 30 minutes are deprioritized
- **Persistent state**: All invocation history is stored in SQLite at `~/.local/share/oulipoly-agent-runner/state.db`

No daemon or background process — state is shared via filesystem-level SQLite WAL locking, so multiple CLI invocations coordinate safely.

## Diagnostics

When a provider fails, the runner can automatically diagnose the error:

1. Pipes stderr to the configured `diagnostics_model`
2. Classifies into: `rate_limit`, `quota_exhausted`, `auth_expired`, `cli_version_mismatch`, `network_error`, or `unknown`
3. Stores the classification in SQLite
4. Future load balancing uses this to avoid broken providers

Falls back to heuristic keyword matching if the diagnostics model itself fails.

## Building

```bash
cargo build --release
```

The binary is at `target/release/oulipoly-agent-runner`.

## Testing

```bash
cargo test
```
