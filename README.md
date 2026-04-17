# Agent Runner

Desktop app for managing LLM provider pools with an AI-driven setup agent. Routes prompts to CLI tools like `claude`, `codex`, `opencode`, etc. with automatic load balancing, error diagnostics, and persistent state tracking.

Built with [Tauri v2](https://v2.tauri.app/) + [SolidJS](https://www.solidjs.com/) + TypeScript.

## Install

Grab a binary from [Releases](https://github.com/nestharus/agent-runner/releases), or build from source (see below).

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Bun](https://bun.sh/) (v1.2+)
- Platform system libraries (Linux only):
  ```bash
  sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev
  ```

## Development

```bash
# Install frontend dependencies
bun install

# Start dev mode (Vite HMR + Rust hot-reload)
bunx tauri dev
```

This opens the app window with the Vite dev server at `localhost:5173` and hot-reloads both frontend and Rust changes.

## Building

```bash
# Build the release binary + platform installers
bunx tauri build
```

Output locations:
- **Linux**: `src-tauri/target/release/bundle/deb/` and `appimage/`
- **macOS**: `src-tauri/target/release/bundle/dmg/` and `macos/`
- **Windows**: `src-tauri/target/release/bundle/msi/` and `nsis/`

The raw binary is at `src-tauri/target/release/oulipoly-agent-runner` (or `.exe` on Windows).

### Manual install (Linux/macOS)

```bash
bunx tauri build
cp src-tauri/target/release/oulipoly-agent-runner ~/.local/bin/
```

## Testing

```bash
# Frontend unit tests (Vitest)
bun run test

# Lint + format check (Biome)
bun run check

# TypeScript type check
bunx tsc --noEmit

# Rust tests
cd src-tauri && cargo test

# Rust lint
cd src-tauri && cargo clippy -- -D warnings
cd src-tauri && cargo fmt --check
```

## Project Structure

```
index.html                    Vite entry point
src/                          Frontend (SolidJS + TypeScript)
  index.tsx                   Mount point
  App.tsx                     Root component with TanStack Router
  lib/
    tauri.ts                  Typed invoke/Channel wrappers
    types.ts                  TypeScript types (mirrors Rust)
    styles.ts                 Tailwind Variants recipes
  views/
    PoolsView.tsx             Model pool management
    SetupView.tsx             AI-driven setup flow
  components/
    FormRenderer.tsx           Dynamic forms from agent actions
    WizardStepper.tsx          Multi-step wizard (Ark UI Steps)
    OAuthFlow.tsx              OAuth login instructions
    ApiKeyEntry.tsx            API key input
    CliSelector.tsx            CLI checkbox selection
    ConfirmDialog.tsx          Confirmation prompts
    ResultDisplay.tsx          Detection/test result summaries
    NavBar.tsx                 Navigation tabs
src-tauri/                    Rust backend (Tauri v2)
  src/
    main.rs                   Tauri entry point
    lib.rs                    App builder + command registration
    ...                       Detection, memory, sync modules
  Cargo.toml
  tauri.conf.json
e2e/                          Playwright QA tests + screenshots
```

## CLI Usage

When launched with no arguments, the app opens the desktop GUI. When given arguments, it runs in headless CLI mode.

```bash
oulipoly-agent-runner [OPTIONS] [AGENT] [PROMPT...]

Arguments:
  [AGENT]       Agent name (from agents directory)
  [PROMPT...]   Prompt text (remaining arguments joined)

Options:
  -m, --model <MODEL>            Execute a model directly (no agent)
  -a, --agent-file <AGENT_FILE>  Path to an agent .md file (any location)
  -f, --file <FILE>              Read prompt from file
  -p, --project <PROJECT>        Working directory for subprocess
  -i, --input <KEY=VALUE>        Pass model inputs as key=value (repeatable)
      --models-dir <MODELS_DIR>  Override models directory
      --agents-dir <AGENTS_DIR>  Override agents directory
  -h, --help                     Print help
```

**Prompt resolution priority:** `--file` > positional arguments > stdin

### Examples

```bash
# Launch desktop GUI
oulipoly-agent-runner

# Direct model execution
oulipoly-agent-runner --model claude-haiku "Explain monads in one sentence"

# Named agent
oulipoly-agent-runner code-reviewer "Review this function"

# Agent file from any path
oulipoly-agent-runner --agent-file ./my-agent.md --model claude-haiku "Do the thing"

# Pipe prompt from stdin
cat spec.md | oulipoly-agent-runner --model codex-high

# Read prompt from file
oulipoly-agent-runner --model glm --file prompt.md

# Set working directory for the subprocess
oulipoly-agent-runner --model codex-high -p /path/to/repo "Fix the tests"

# Generate an image (raw bytes on stdout, pipe to file)
oulipoly-agent-runner -m seedream-t2i "A sunset over mountains" > sunset.jpeg

# Generate a video
oulipoly-agent-runner -m seedance-t2v-low -i duration=5 -i resolution=480p "A whale swimming" > whale.mp4

# Image-to-video with source image
oulipoly-agent-runner -m seedance-i2v-fast -i image=./photo.jpg "Slow camera orbit" > orbit.mp4

# Image editing with reference images
oulipoly-agent-runner -m seedream-i2i -i image=input.png "Make it warmer" > edited.jpeg

# Chain: generate an image then animate it
oulipoly-agent-runner -m seedream-t2i "A cat painting" > cat.jpeg
oulipoly-agent-runner -m seedance-i2v-low -i image=cat.jpeg "The cat blinks slowly" > cat.mp4
```

## Load Balancing

Models with multiple `[[providers]]` are automatically load balanced. The runner picks a provider per invocation using, in order of preference:

1. **Quota-aware scoring** (when every provider has a cached quota reading): picks the provider with the lowest *projected* usage, where projection = `used_percent + calls_since_refresh × avg_percent_per_call`. The `avg_percent_per_call` is learned globally across refreshes, so fresh accounts with no learned delta fall back to raw `used_percent` ordering.
2. **Invocation-count round-robin** (fallback when any provider lacks a quota reading): picks the provider with the fewest lifetime invocations.
3. **Error avoidance** (always applied): providers with 3+ errors in the last 30 minutes are deprioritized regardless of score.

Quota readings are refreshed lazily — each CLI invocation checks if any participating provider's cached reading is older than 12 hours and, if so, runs that provider's `quota_script` (see [`providers.toml`](#providerstoml) below) synchronously before picking. Refreshes are deduplicated across concurrent callers by an in-process lock.

Provider state is keyed by the provider's `name` field (the CLI account — e.g. `claude`, `claude2`) and is shared across every model routed through that account. This means two models pointing at the same provider share quota and error history.

**Persistent state**: all invocation history and quota snapshots live in SQLite at `~/.local/share/oulipoly-agent-runner/state.db`. No daemon or background process — state is shared via filesystem-level SQLite WAL locking, so multiple CLI invocations coordinate safely.

### `providers.toml`

To enable quota-aware balancing, create `~/.config/oulipoly-agent-runner/providers.toml` with one entry per provider account. Each entry declares a `quota_script` — a shell command that prints JSON `{"used_percent": <0..100>, "resets_at": "..."}` on stdout:

```toml
[claude]
quota_script = "anthropic-usage ~/.claude/.credentials.json"

[claude2]
quota_script = "anthropic-usage ~/.claude2/.credentials.json"
```

The `used_percent` field is on a 0..100 scale (matching what provider APIs typically return). `resets_at` is optional (RFC 3339 timestamp). Scripts have a 30-second timeout and run via `sh -c`, so `~` expansion and pipelines work.

Providers without a `quota_script` entry cause that model to fall back to invocation-count scoring.

### Diagnostic tool

```bash
cd src-tauri && cargo run --release --example quota_check
```

Loads the installed models + `providers.toml`, refreshes any stale quotas, then prints — for every multi-provider model — what `select_provider` would pick and the score breakdown.

## Diagnostics

When a provider fails, the runner can automatically diagnose the error:

1. Pipes stderr to the configured `diagnostics_model`
2. Classifies into: `rate_limit`, `quota_exhausted`, `auth_expired`, `cli_version_mismatch`, `network_error`, or `unknown`
3. Stores the classification in SQLite for future load balancing decisions

Falls back to heuristic keyword matching if the diagnostics model itself fails.

## Configuration

All user config lives in `~/.config/oulipoly-agent-runner/`:

```
~/.config/oulipoly-agent-runner/
  config.toml          Global settings
  providers.toml       Per-provider quota scripts (optional, for quota-aware balancing)
  models/              Model configs (one .toml per model)
  agents/              Agent configs (one .md per agent)
```

### Adding a Model

Create a `.toml` file in the models directory. The filename becomes the model name.

**Text model (single provider):**
```toml
command = "claude"
args = ["-p", "--model", "haiku"]
prompt_mode = "stdin"

[[inputs]]
name = "prompt"
type = "string"
required = true
default_input = true
description = "The text prompt"
```

**Text model (multiple providers, load balanced):**
```toml
prompt_mode = "arg"

[[providers]]
command = "codex"
args = ["exec", "-m", "gpt-5.3-codex"]

[[providers]]
command = "codex2"
args = ["exec", "-m", "gpt-5.3-codex"]

# Optional: set `name = "..."` on a provider to pin the identity used for
# quota/error tracking. If omitted, the name is auto-derived from the command
# (e.g. `env -u CLAUDECODE claude2` → `claude2`).

[[inputs]]
name = "prompt"
type = "string"
required = true
default_input = true
description = "The text prompt"
```

**Image/video model with typed inputs:**
```toml
command = "atlas-i2v-fast"
prompt_mode = "arg"

[[inputs]]
name = "prompt"
type = "string"
default_input = true
description = "Motion/style description"

[[inputs]]
name = "image"
type = "string"
flag = "--image"
required = true
description = "Source image path (jpg/png/svg)"

[[inputs]]
name = "duration"
type = "integer"
flag = "--duration"
min = 4.0
max = 12.0
default = 8
description = "Video length in seconds"

[[inputs]]
name = "resolution"
type = "enum"
flag = "--resolution"
options = ["480p", "720p", "1080p"]
default = "720p"
description = "Output video resolution"

[[inputs]]
name = "aspect_ratio"
type = "enum"
flag = "--aspect-ratio"
options = ["16:9", "9:16", "1:1", "4:3", "3:4", "21:9"]
default = "16:9"
description = "Output aspect ratio"

[[inputs]]
name = "images"
type = "array"
flag = "--image"
item_type = "string"
min_items = 1
max_items = 14
description = "Reference images (for edit models)"
```

### Input Schema

Each `[[inputs]]` entry declares a parameter the model accepts. The runner validates inputs
and passes them as CLI flags to the underlying command.

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Input identifier |
| `type` | yes | `string`, `integer`, `number`, `boolean`, `enum`, `array` |
| `flag` | no | CLI flag to pass to the command (e.g. `"--size"`) |
| `required` | no | Fail if not provided and no default |
| `default_input` | no | This is the unnamed positional input (the "prompt") |
| `default` | no | Default value when not provided by user |
| `description` | no | Human/AI-readable description |
| `options` | enum only | List of valid values |
| `min` / `max` | integer/number | Value range bounds |
| `item_type` | array only | Type of array elements |
| `min_items` / `max_items` | array only | Array length bounds |

**How inputs flow:**
- The `default_input` receives the positional prompt (args, `--file`, or stdin)
- Named inputs (`-i key=value`) are validated against the schema, then passed as `--flag value` to the command
- Repeated `-i` with the same key collects into an array (e.g. `-i images=a.png -i images=b.png`)
- Inputs with defaults are passed automatically when not overridden
- Unknown inputs pass through as `--key value`

**Stdout is raw bytes** — commands can output binary data (images, videos) and it passes through unmodified. Pipe to a file to save: `agents -m seedream-t2i "A cat" > cat.jpeg`

### Adding an Agent

Create a `.md` file in the agents directory:

```markdown
---
description: 'Code review assistant'
model: claude-haiku
output_format: ''
---

You are a senior code reviewer. Be concise and actionable.
```

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Framework | Tauri v2 |
| Frontend | SolidJS 1.9 + TypeScript |
| Build | Vite 7 |
| Styling | Tailwind CSS 4 + Tailwind Variants |
| Components | Ark UI (headless) |
| Routing | TanStack Solid Router |
| Async state | TanStack Solid Query |
| Linting | Biome |
| Testing | Vitest + Playwright |
| Package manager | Bun |
| Backend | Rust (Tokio + SQLite) |

## License

MIT
