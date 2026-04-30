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

Subcommands:
  trace <invocation_uuid> [--json] [--transcript] [--max-depth N]
        Walk a recorded invocation tree (see Inspecting a Run)

  repl <model> [--resume <session-id>] [-p <project>] [--models-dir <path>]
        Launch a balanced interactive session of the wrapped CLI
        (see Interactive REPL)

  resume -m <model> --session-id <session-id> [-f <answer.md>|--prompt <text>] [-p <project>] [--models-dir <path>]
        Resume a provider session non-interactively with an answer payload
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

## Interactive REPL

`oulipoly-agent-runner repl <model>` launches the wrapped CLI as an interactive session through the load balancer instead of as a one-shot. Stdin / stdout / stderr are inherited (TTY pass-through), so terminal-generated `Ctrl+C` reaches the child directly. The runner stays alive only long enough to reap and finalize the invocation row.

```bash
# Launch a balanced Claude REPL
oulipoly-agent-runner repl claude-opus

# Resume a specific session by full UUID — picks the right provider
# automatically, regardless of which account owns the session
oulipoly-agent-runner repl claude-opus --resume 9e69e8cc-616d-4640-bf1d-96f5391b1a2e

# Codex resume composes via subcommand instead of a flag, transparently
oulipoly-agent-runner repl codex-high --resume 5169694d-de0f-40d1-890c-6e28e55bab27
```

Each `repl` invocation requires the resolved provider to declare `interactive_args` (the argv shape used for interactive launch — distinct from `args`, which encodes one-shot mode like Claude `-p` or Codex `exec`). With `--resume`, the resolved provider must additionally declare a `[providers.resume]` block; see [Resuming a session](#resuming-a-session) below.

On Unix, signal handling forwards `SIGTERM` once and lets `SIGINT` / `SIGHUP` reach the child through the foreground process group. Windows console-control handling is not implemented yet.

## Load Balancing

Models with multiple `[[providers]]` are automatically load balanced. The runner picks a provider per invocation using, in order of preference:

1. **Per-window binding-rate scoring** (when every provider has at least one quota window): for each window `w` of each provider, project forward with a **per-window burn rate** — `projected_used_w = used_percent_w + turns_since_refresh × burn_rate_w`, where `burn_rate_w` is learned refresh-to-refresh from observed `Δused_percent / Δturns` and stored per window. Score = `(1 − projected_used_w) × hours_until_reset_w`; binding score per provider = `min_w`; pick = `argmax`. A near-exhausted short window (e.g. 5h hitting 95%) drops binding score toward 0 and forces traffic away even if the weekly is fine, and a heavily-used weekly tier is correctly weighted against a fresh 5h tier because each projects at its own rate.
2. **Invocation-count round-robin** (fallback when no provider has learned any burn rate yet — a true first-run pool): picks the provider with the fewest lifetime invocations.
3. **Error avoidance** (always applied): providers with 3+ errors in the last 30 minutes are deprioritized regardless of score.

**Bootstrap cascade.** A window with no directly-learned rate falls through: own-provider → pool sibling on the same `window_id` → duration-ratio from a longer sibling window (scaled by `long_hours / target_hours`, so a 5h slot derived from a 7d learned rate gets a ~33.6× multiplier — shorter tiers burn proportionally faster per turn). If every window of every provider returns `None`, the pool goes to invocation-count round-robin.

Accounts often have different reset days/times AND different tier structures. Comparing a 50%-used 1h tier to a 10%-used 7d tier on raw `used_percent` is misleading because the same turn consumes a much larger fraction of the shorter tier. Per-window burn rates make the projection tier-aware.

When a provider actually fails with a quota-exhausted diagnostic (`quota`, `billing`, or `usage limit` in stderr), the account is marked exhausted in SQLite. The balancer skips that provider account for future selections until the next successful non-empty quota refresh clears the flag. There are no threshold gates or pre-emptive blocking; projection ranks providers, and reactive failures temporarily remove accounts from the candidate set.

Quota readings are refreshed lazily — each CLI invocation runs the participating providers' `quota_script` (see [`providers.toml`](#providerstoml) below) when their cached reading is older than the **dynamic TTL**: `min(hours_until_reset across windows) / 5`, clamped to `[5min, 24h]`. So a provider with a 5-hour window gets re-queried hourly; a provider with only a weekly window gets re-queried every ~33 hours. Refreshes are deduplicated across concurrent callers by an in-process lock. Empty-window responses are rejected (prior windows preserved, `provider_quotas.last_empty_refresh_at` recorded for audit); a provider whose quota row ends up with zero windows is **force-stale** on the next `is_stale` check so it self-heals on the next `select_provider` call.

Provider state is keyed by the provider's `name` field (the CLI account — e.g. `claude`, `claude2`) and is shared across every model routed through that account. This means two models pointing at the same provider share quota and error history.

**Persistent state**: invocation history, quota snapshots, and ingested session turns live in SQLite at `~/.local/share/oulipoly-agent-runner/state.db`. No daemon or background process — state is shared via filesystem-level SQLite WAL locking, so multiple CLI invocations coordinate safely.

### `providers.toml`

To enable quota-aware balancing, create `~/.config/oulipoly-agent-runner/providers.toml` with one entry per provider account. Each entry declares a `quota_script` — a shell command that prints JSON on stdout describing one or more rolling-quota windows:

```toml
[claude]
quota_script         = "anthropic-usage ~/.claude/.credentials.json"
auth_refresh_command = "claude auth status"

[claude2]
quota_script         = "anthropic-usage ~/.claude2/.credentials.json"
auth_refresh_command = "claude auth status"

[codex]
quota_script         = "chatgpt-usage ~/.codex/auth.json"
auth_refresh_command = "codex login status"

[opencode]
quota_script = "zai-usage ~/.config/opencode/auth.json"
```

**Script output (multi-window)**:

```json
{
  "windows": [
    {"used_percent": 23, "resets_at": "2026-04-23T19:00:00Z"},
    {"used_percent": 45, "resets_at": "2026-04-17T15:00:00Z"}
  ]
}
```

`used_percent` is on a **0..100 scale** — values outside that range are rejected with an error naming the offending script and value. Scripts wrapping APIs that report a 0..1 fraction must multiply by 100 before emitting. The runner does not auto-detect the scale; conforming to the contract is the script's job. `resets_at` is required RFC 3339. Window count is arbitrary — emit one for each rolling-quota tier the provider exposes (Anthropic has 5h + 7d, z.ai has 5h + weekly, etc.).

**Backwards compatibility**: the legacy single-window shape `{"used_percent": X, "resets_at": "..."}` is still parsed and treated as one window.

Scripts have a 30-second timeout and run via `sh -c`, so `~` expansion and pipelines work. Providers without a `quota_script` entry fall back to invocation-count scoring.

**Auth refresh.** Provider OAuth tokens (Claude, Codex) expire and the upstream APIs return errors, which the bundled scripts surface as a non-zero exit. When that happens — or when a script returns an empty `windows: []` on a provider that previously had non-empty windows — the runner shells out to the optional `auth_refresh_command`, lets the CLI's own auth code refresh the token, then retries `quota_script` once. The runner does not implement OAuth itself; it delegates to whichever command the CLI exposes (`claude auth status`, `codex login status`, etc.). The refresh command runs with closed stdin, a 15-second timeout, and stdout discarded; only its exit code matters. If both the refresh and the retry fail, the failure is recorded in the resulting `RefreshOutcome` so it surfaces in diagnostics.

**Reference quota adapters** (in [`scripts/`](scripts/)):

| Script | API | Windows |
|---|---|---|
| `anthropic-usage CREDS` | `/api/oauth/usage` | 5-hour + 7-day |
| `chatgpt-usage ~/.codex/auth.json` | `/backend-api/wham/usage` | weekly + 5-hour |
| `zai-usage AUTH_JSON` | `/api/monitor/usage/quota/limit` | 5-hour + weekly (when usage > 0) |

Install them on your `$PATH`:

```bash
install -m 755 \
  scripts/anthropic-usage \
  scripts/chatgpt-usage \
  scripts/zai-usage \
  scripts/claude-code-turns \
  scripts/codex-turns \
  ~/.local/bin/
```

## Session Ingestion

Direct CLI usage burns the same weekly/5h quota as agent-runner invocations, but the balancer can't see those calls without help. Session ingestion solves this by reading each CLI's session logs and counting **assistant turns** — each one is one API call.

Configure adapters in `~/.config/oulipoly-agent-runner/sessions.toml`:

```toml
[claude]
turn_script = "claude-code-turns ~/.claude/projects"

[claude2]
turn_script = "claude-code-turns ~/.claude2/projects"

[codex]
turn_script = "codex-turns ~/.codex/sessions"

# Optional: override where the script keeps its incremental cursor.
[claude3]
turn_script = "claude-code-turns ~/.claude3/projects"
state_dir   = "~/.cache/oulipoly/claude3-cursor"
```

**Turn script contract** — same adapter pattern as quota scripts. The runner spawns the script with `STATE_DIR` env (a writable dir for the script's own incremental cursor) and parses one JSON object per line on stdout:

```json
{
  "session_id": "...",
  "turn_id": "...",
  "timestamp": "<RFC 3339>",
  "role": "user|assistant",
  "parent_turn_id": "<turn_id|null>",
  "is_sidechain": true
}
```

`parent_turn_id` and `is_sidechain` are **optional**. Adapters that don't track within-session parentage emit only the first four fields; the runner treats those turns as linear with `is_sidechain = false`. The Claude Code reference adapter passes through the raw `parentUuid` and `isSidechain` fields it sees in Claude's per-session JSONL — those surface as branch counts in `trace --json`'s `session.sidechain_turn_count`.

Idempotent — re-running with no source changes outputs nothing. The runner's `session_turns` table has `UNIQUE(provider, session_id, turn_id)` so duplicate emission is also tolerated.

**Reference turn adapters** (in [`scripts/`](scripts/)):

| Script | Adapts | Storage |
|---|---|---|
| `claude-code-turns BASE_DIR` | Claude Code | JSONL tree under `BASE_DIR`; preserves `parentUuid` + `isSidechain` |
| `codex-turns BASE_DIR` | Codex CLI | Date-sharded JSONL under `BASE_DIR` |

For other CLIs (SQLite history, remote API, etc.), write your own script — see [`scripts/README.md`](scripts/README.md). The application stays format-agnostic; everything CLI-specific lives in adapter scripts.

### Optional: `transcript_locator`

`sessions.toml` entries may also declare a `transcript_locator` script that resolves a `session_id` to the absolute path of its raw transcript file. Used by `trace --json` to fill `transcript_path` and `transcript_state` per node. The lookup is **lazy at trace time** — never at invocation time — so unused providers cost nothing.

```toml
[claude]
turn_script        = "claude-code-turns ~/.claude/projects"
transcript_locator = "claude-code-locate-transcript ~/.claude/projects"
```

The script receives `SESSION_ID` and `STATE_DIR` env vars and prints a single absolute path on stdout. Reference scripts: `claude-code-locate-transcript` (matches `<session_id>.jsonl` directly) and `codex-locate-transcript` (matches `rollout-*-<session_id>.jsonl`).

When unset, `trace` shows `transcript_state = "no_locator"` for that provider — graceful degradation.

## Inspecting a Run

Every invocation emits a stable identifier on **stderr** before spawning the wrapped CLI:

```
OULIPOLY_INVOCATION={"source":"claude2","id":"9e69e8cc-616d-4640-bf1d-96f5391b1a2e"}
```

`stdout` stays the model's response (binary-safe for image/video models). The line is always emitted, exactly once per process.

Capture it from a wrapper:

```bash
oulipoly-agent-runner -m claude-haiku "Refactor X" 2> >(tee /tmp/run.err >&2)
INV=$(grep '^OULIPOLY_INVOCATION=' /tmp/run.err | cut -d= -f2- | jq -r .id)
```

### `trace` subcommand

```bash
oulipoly-agent-runner trace <invocation_uuid>          # ASCII tree
oulipoly-agent-runner trace <invocation_uuid> --json   # structured JSON
```

DFS walk over `parent_invocation_id` edges in the `invocations` table — shows the captured invocation and every child invocation it spawned, with model, provider/account, status, timing, and per-node session/transcript state. Cycle protection (HashSet of visited row IDs); depth limit via `--max-depth` (default 64).

Each node's `session.transcript_state` is one of:

- `available` — locator returned a path that exists; `transcript_path` populated
- `missing` — locator returned a path but the file is gone
- `no_locator` — `session_id` known, but no `transcript_locator` configured for that provider
- `unresolved` — no `session_id` was captured (e.g. the provider has no `session_capture` config, or capture was attempted and failed)

Flags:

- `--json` — structured output for piping into other tools
- `--inline-transcript` (requires `--json`) — embed raw provider records inline; null in this version (placeholder for future)
- `--transcript` (human mode only; conflicts with `--json`) — append a transcript footer
- `--max-depth N` — truncate descendants past depth N

### Cross-invocation tracking

When `oulipoly-agent-runner` invokes a wrapped CLI that itself spawns another `oulipoly-agent-runner` (e.g. via the `Task` tool in Claude Code), the runner propagates `OULIPOLY_PARENT_INVOCATION` as an env var to the subprocess. The child's invocation row records `parent_invocation_id` pointing at the parent. `trace` walks that tree.

If the env var is malformed, points at an unknown invocation, or has an invalid UUID, the child silently treats itself as a root invocation (no panic; observable via `parent_id = null` in trace output).

### Configuring session capture

To populate `trace`'s `session.id` and `transcript_path`, add a `session_capture` block to the provider in your model TOML:

```toml
# Claude Code: force a runner-generated UUID via --session-id and verify readback
[[providers]]
command = "claude"
args    = ["-p"]

[providers.session_capture]
kind          = "forced_flag_verified"
flag          = "--session-id"
readback_args = ["--verbose", "--output-format", "stream-json"]
```

```toml
# Codex: parse the `thread.started` event from --json mode; restore plain text from -o tmpfile
[[providers]]
command = "codex"
args    = ["exec"]

[providers.session_capture]
kind              = "stdout_json_event"
json_flag         = "--json"
last_message_flag = "-o"
event_type        = "thread.started"
event_id_path     = "thread_id"
```

Without `session_capture`, invocations record `session_capture_method = "none"` and `trace` shows `transcript_state = "unresolved"` — clean degradation, no breakage.

### Resuming a session

When a provider declares a `[providers.resume]` block, `repl --resume <UUID>` looks the session up across all providers (via the `session_turns` ingest table), validates that the owning provider belongs to the requested model's provider pool, and composes the right interactive resume argv.

For non-interactive answer handoff, use `resume`:

```bash
oulipoly-agent-runner resume -m claude-opus --session-id 9e69e8cc-616d-4640-bf1d-96f5391b1a2e -f answer.md
oulipoly-agent-runner resume -m codex-high --session-id 5169694d-de0f-40d1-890c-6e28e55bab27 --prompt "answer text"
```

`resume` uses the same owner lookup and provider-pool validation as `repl --resume`, but launches the provider's one-shot `args` with the resume strategy and answer payload. It records `resume_acceptance` in `trace`.

```toml
# Claude: --resume <UUID> as a flag on the existing interactive launch
[[providers]]
name = "claude2"
command = "env"
args             = ["-u", "CLAUDECODE", "claude2", "-p", "--model", "opus", "--dangerously-skip-permissions"]
interactive_args = ["-u", "CLAUDECODE", "claude2", "--model", "opus", "--dangerously-skip-permissions"]

[providers.resume]
kind = "flag"
flag = "--resume"
```

```toml
# Codex: resume <UUID> as a subcommand appended after interactive_args
[[providers]]
name = "codex"
command = "codex"
args             = ["exec", "--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.4"]
interactive_args = ["--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.4"]

[providers.resume]
kind = "subcommand"
subcommand = ["resume"]
```

The runner always emits a short selection line on stderr regardless of TTY:

```
[resume] -> claude2
```

When a session id matched multiple providers (rare; requires cross-provider session id collisions in the ingest table), a longer detail line lists all matches but only when stderr is **not** a TTY:

```
[resume] session 9e69e8cc-... matched claude2, claude3; selected claude2 by latest turn timestamp
```

Resume failures all exit `1` with a specific stderr message:

- **No session found** — the UUID is not in `session_turns` (typically: session ingestion isn't configured, or the provider's local store has dropped the session)
- **Invalid session UUID** — the input wasn't a valid full UUID (no prefix matching)
- **Provider/model mismatch** — the resolved provider is not in the requested model's provider pool. The error suggests other models that include the resolved provider, e.g. `Try a model that includes claude2: claude-opus, claude-sonnet`.
- **Provider has no `[providers.resume]` block** — the resolved provider exists in the model's pool but doesn't declare a resume strategy.

The invocation row records `session_capture_method = "resumed"` and the user-supplied `session_id` *before* spawn. This means `trace` can show what session the runner attempted to resume even if the wrapped CLI rejects the id (e.g. "No conversation found"). Trace renders the session as `Resume target: <UUID>` instead of `Session: <UUID>` to make this distinction explicit, and adds a warning: `session marked as attempted resume target; child acceptance is not confirmed by this row — check exit_code and recent_errors for outcome`.

Plain `repl <model>` (no `--resume`) records `session_capture_method = "none"`; the proposal explicitly does not invent a fresh-session capture mechanism for interactive launches.

### Inspecting via SQL

For ad-hoc questions that don't fit the `trace` shape, query SQLite directly:

```bash
# All invocations for one account today
sqlite3 ~/.local/share/oulipoly-agent-runner/state.db "
  SELECT invocation_uuid, model_name, status, created_at
  FROM invocations
  WHERE provider_name = 'claude2'
    AND created_at > date('now')
  ORDER BY created_at DESC
"
```

### Diagnostic tools

```bash
cd src-tauri
cargo run --release --example quota_check     # Refresh quotas, show density picks
cargo run --release --example session_scan    # Run all turn scripts, show counts
```

`quota_check` loads `providers.toml`, refreshes stale windows, and prints — for every multi-provider model — the binding density and which provider would be picked. `session_scan` runs every `turn_script`, ingests new turns into `session_turns`, and prints per-provider totals.

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
  sessions.toml        Per-provider turn ingestion + transcript locator adapters
                       (optional, for accurate cross-source projection + trace inspection)
  models/              Model configs (one .toml per model; provider entries can declare
                       `[providers.session_capture]` for trace's session correlation)
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
