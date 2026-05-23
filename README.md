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
Cargo.toml                    Rust workspace manifest
crates/
  oulipoly-core/              Shared leaf types
  oulipoly-config/            Model/provider/agent/session config
  oulipoly-state/             SQLite state DB + schema probe
  oulipoly-runtime/           Executor, balancer, discovery, sessions, quota, trace
  oulipoly-setup/             Setup agent, detection, memory, sync
  oulipoly-agent-store/       Versioned artifact storage binary (`agent-store`)
  oulipoly-agent-scratchpad/  Invocation-private artifact workspace (`agent-scratchpad`)
  oulipoly-agent-messenger/   Return-to-caller artifact binary (`agent-messenger`)
src-tauri/                    Tauri/headless client crate
  src/
    main.rs                   CLI entry point
    lib.rs                    App builder + command registration
    setup/flow.rs             Tauri channel setup orchestration
  Cargo.toml
  tauri.conf.json
e2e/                          Playwright QA tests + screenshots
```

## CLI Usage

When launched with no arguments, the app opens the desktop GUI. When given arguments, it runs in headless CLI mode.

The workspace also ships standalone artifact tools: `agent-store`, `agent-scratchpad`, and `agent-messenger`. `agent-messenger` lets a child return durable artifact refs to its caller; one-shot and headless resume invocations record those refs as terminal `returned_artifacts`, and `trace --json` projects them under each invocation without changing raw provider stdout.

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
  -n, --new                      Start a fresh default-provider interactive session
      --resume <SESSION_ID>      Resume an existing session at the top level
      --usage                    Print live usage for model-pool provider accounts
      --models-dir <MODELS_DIR>  Override models directory
      --agents-dir <AGENTS_DIR>  Override agents directory
  -h, --help                     Print help

Subcommands:
  trace <invocation_uuid> [--json] [--transcript] [--max-depth N]
        Walk a recorded invocation tree (see Inspecting a Run)

  session locate <session-id> [--json]
        Locate stable metadata for a provider session (see Locating a Session)

  session schema-probe
        Inspect the default state-DB schema and supported session features
        (see Probing the Session Surface).

  session export <session-id> [--format canonical-jsonl]
        Export a provider session as canonical JSONL on stdout
        (see Exporting a Session).

  session import-replace <session-id> [--from-file <path>] [--preimage-sha256 <hex>]
        Atomically replace a provider transcript and its derived state from
        canonical JSONL on stdin or `--from-file` (see Replacing a Session
        Transcript).

  session pause-handshake <session-id> [--ttl-ms <ms>]
        Acquire an advisory pause lease for a session (see Session Locks).

  session resume-handshake <session-id> --token <token>
        Release a previously acquired advisory pause lease.

  repl [<model>] [--resume <session-id>] [-p <project>] [--models-dir <path>]
        Launch a balanced interactive session of the wrapped CLI
        (see Interactive REPL)

  resume [-m <model>] --session-id <session-id> [-f <answer.md>|--prompt <text>] [-p <project>] [--models-dir <path>]
        Resume a provider session non-interactively with an answer payload

  migrate-db
        Run the existing session-chain and compaction backfill.

  migrate --rebuild
        Back up state.db plus WAL/SHM sidecars, then create a fresh current
        state DB. Historical live rows are not preserved after rebuild.

  migrate-config [--models-dir <path>]
        Move provider runtime blocks from old model TOMLs into providers.toml.
        Idempotent - safe to re-run if a previous run left empty args.
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

`oulipoly-agent-runner --new` starts a fresh interactive session using
`default_provider` from `config.toml`. This is the top-level fresh-session
entrypoint; `--resume <session-id>` is its existing-session counterpart.

`oulipoly-agent-runner repl <model>` launches the wrapped CLI as an interactive session through the load balancer instead of as a one-shot. Stdin / stdout / stderr are inherited (TTY pass-through), so terminal-generated `Ctrl+C` reaches the child directly. The runner stays alive only long enough to reap and finalize the invocation row.

```bash
# Launch a fresh default-provider REPL
oulipoly-agent-runner --new

# Launch a balanced Claude REPL
oulipoly-agent-runner repl claude-opus

# Resume a specific session by full UUID — picks the right provider
# automatically, regardless of which account owns the session
oulipoly-agent-runner repl claude-opus --resume 9e69e8cc-616d-4640-bf1d-96f5391b1a2e

# Codex resume composes via subcommand instead of a flag, transparently
oulipoly-agent-runner repl codex-high --resume 5169694d-de0f-40d1-890c-6e28e55bab27
```

Each `repl` invocation requires the resolved provider to declare `interactive_args` in `providers.toml` (the argv shape used for interactive launch — distinct from `args`, which encodes one-shot mode like Claude `-p` or Codex `exec`). With `--resume`, the provider must additionally declare a `[<provider>.resume]` block; see [Resuming a session](#resuming-a-session) below.

For Claude provider entries, see [`docs/architecture/claude-proxy-mcp-launch-shape.md`](docs/architecture/claude-proxy-mcp-launch-shape.md) before adding MCP-related tool filters to `interactive_args` — interactive PTY MCP replacement launches require `--allowedTools mcp__<server>__<tool>,...` (or no filter) and not `--tools mcp__<server>__<tool>,...`.

On Unix, signal handling forwards `SIGTERM` once and lets `SIGINT` / `SIGHUP` reach the child through the foreground process group. Windows console-control handling is not implemented yet.

## Load Balancing

Models with multiple `[[providers]]` are automatically load balanced. The runner picks a provider per invocation using, in order of preference:

1. **Per-window binding-rate scoring** (when every provider has at least one quota window): for each window `w` of each provider, project forward with a **per-window burn rate** — `projected_used_w = used_percent_w + turns_since_refresh × burn_rate_w`, where `burn_rate_w` is learned refresh-to-refresh from observed `Δused_percent / Δturns` and stored per window. Score = `(1 − projected_used_w) × hours_until_reset_w`; binding score per provider = `min_w`. A near-exhausted short window (e.g. 5h hitting 95%) drops binding score toward 0 and forces traffic away even if the weekly is fine, and a heavily-used weekly tier is correctly weighted against a fresh 5h tier because each projects at its own rate.
2. **Invocation-count round-robin** (fallback when no provider has learned any burn rate yet — a true first-run pool): picks the provider with the fewest lifetime invocations.
3. **Error avoidance** (always applied): providers with 3+ errors in the last 30 minutes are deprioritized regardless of score.

When learned binding scores are close, selection fans out deterministically instead of pinning every call to the argmax. Providers within the 2× score band of the best provider are eligible for fanout; the runner picks the band member with the lowest lifetime invocation count, then breaks ties by higher binding score and provider order. Wide score gaps still hard-pin to the best binding score, so a much lower-capacity account is not pulled into rotation just because it has fewer historical invocations.

**Bootstrap cascade.** A window with no directly-learned rate falls through: own-provider → pool sibling on the same `window_id` → duration-ratio from a longer sibling window (scaled by `long_hours / target_hours`, so a 5h slot derived from a 7d learned rate gets a ~33.6× multiplier — shorter tiers burn proportionally faster per turn). If every window of every provider returns `None`, the pool goes to invocation-count round-robin.

Accounts often have different reset days/times AND different tier structures. Comparing a 50%-used 1h tier to a 10%-used 7d tier on raw `used_percent` is misleading because the same turn consumes a much larger fraction of the shorter tier. Per-window burn rates make the projection tier-aware.

When a provider actually fails with a quota-exhausted diagnostic (`quota`, `billing`, or `usage limit` in stderr/stdout), the account is marked exhausted in SQLite. The balancer skips that provider account for future selections until the next successful non-empty quota refresh clears the flag. Cached live quota windows at or above 100% are also hard-excluded before scoring, so an exhausted 5h or 7d window cannot win through round-robin or fallback logic. If every provider in a pool is exhausted, routing fails before spawning a CLI with `all providers in pool <model> are quota-exhausted`.

Quota readings are refreshed lazily — each CLI invocation runs the participating providers' `quota_script` (see [`providers.toml`](#providerstoml) below) when their cached reading is older than the **dynamic TTL**: `min(hours_until_reset across windows) / 5`, clamped to `[5min, 24h]`. So a provider with a 5-hour window gets re-queried hourly; a provider with only a weekly window gets re-queried every ~33 hours. Refreshes are deduplicated across concurrent callers by an in-process lock. Empty-window responses are rejected (prior windows preserved, `provider_quotas.last_empty_refresh_at` recorded for audit); a provider whose quota row ends up with zero windows is **force-stale** on the next `is_stale` check so it self-heals on the next `select_provider` call.

After the normal stale-refresh pass, balanced CLI routing also compares live quota-window topology across providers in the same pool. If one provider has a non-empty but shorter cached topology than its siblings or its previously observed peak, the runner records a topology-probe timestamp and runs that provider's `quota_script` once before scoring, subject to a one-hour cooldown. This repairs stale one-window caches without changing `refresh_quotas` IPC output or the quota unit contract: scripts still emit `used_percent` on a 0..100 scale, and stored routing windows still use 0..1 ratios.

Provider state is keyed by the provider's `name` field (the CLI account — e.g. `claude`, `claude2`) and is shared across every model routed through that account. This means two models pointing at the same provider share quota and error history.

**Persistent state**: invocation history, quota snapshots, and ingested session turns live in SQLite at `~/.local/share/oulipoly-agent-runner/state.db`. No daemon or background process — state is shared via filesystem-level SQLite WAL locking, so multiple CLI invocations coordinate safely.

### `providers.toml`

Create `~/.config/oulipoly-agent-runner/providers.toml` with one entry per provider account. This is the runtime config for that account: how to invoke the CLI, how prompts are passed, how resume is composed, where local session files live, and optional quota/auth hooks.

```toml
[claude]
quota_script         = "anthropic-usage ~/.claude/.credentials.json"
auth_refresh_command = "claude auth status"
command              = "claude"
args                 = ["-p"]
interactive_args     = []
prompt_mode          = "stdin"

[claude.resume]
kind = "flag"
flag = "--resume"

[claude2]
quota_script         = "anthropic-usage ~/.claude2/.credentials.json"
auth_refresh_command = "claude auth status"
command              = "env"
args                 = ["-u", "CLAUDECODE", "claude2", "--dangerously-skip-permissions"]
interactive_args     = ["-u", "CLAUDECODE", "claude2", "--dangerously-skip-permissions"]
prompt_mode          = "stdin"

[claude2.resume]
kind = "flag"
flag = "--resume"

[claude2.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[claude2.session_storage]
kind = "script"
cwd_script = "claude-code-cwd ~/.claude2/projects"
transcript_script = "claude-code-locate-transcript ~/.claude2/projects"
storage_type = "claude_code"

[claude2.resume_acceptance]
accepted_output_patterns = ["\"session_id\":\"{session_id}\""]
rejected_output_patterns = ["No conversation found", "Invalid resume"]

[codex]
quota_script         = "chatgpt-usage ~/.codex/auth.json"
auth_refresh_command = "codex login status"
command              = "codex"
args                 = ["exec"]
interactive_args     = []
prompt_mode          = "arg"

[codex.resume]
kind = "subcommand"
subcommand = ["resume"]

[codex.session_storage]
kind = "script"
cwd_script = "codex-cwd ~/.codex/sessions"
transcript_script = "codex-locate-transcript ~/.codex/sessions"
storage_type = "codex_session"

[opencode]
quota_script = "zai-usage ~/.config/opencode/auth.json"
```

`args` and `interactive_args` are provider/account defaults only. Do not put model flags there; model flags live in model TOMLs and are appended at spawn time.

Required reading before adding `--tools` or `--allowedTools` flags to a Claude provider entry: [`docs/architecture/claude-proxy-mcp-launch-shape.md`](docs/architecture/claude-proxy-mcp-launch-shape.md). Interactive PTY MCP replacement launches must use `--allowedTools mcp__<server>__<tool>,...` or no filter, never `--tools mcp__<server>__<tool>,...`.

`cwd_script` resolves the workspace for provider-native resume. `transcript_script` resolves the raw transcript path for locate/export/import-replace, and `storage_type` declares the canonical provider transcript format used by those features.

If you already ran `migrate-config` from `98e692c` or the script-storage migration and it left provider `args`, `interactive_args`, or `session_storage` incomplete, re-run the fixed `migrate-config`. It will repair the provider runtime command shape and backfill script storage from `sessions.toml` turn adapters without duplicating existing resume/session blocks.

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

Scripts have a 30-second timeout and run via `sh -c`, so `~` expansion and pipelines work. During CLI routing, an explicit `quota_script` is refreshed with a 30-second routing TTL; if it is absent but Claude/Codex `session_storage` or `sessions.toml` roots are present, the runner derives the standard bundled `anthropic-usage` / `chatgpt-usage` adapter from those roots. Providers without an explicit or derived quota adapter fall back to invocation-count scoring.

`oulipoly-agent-runner --usage` enumerates provider accounts referenced by the active model pool, runs each configured `quota_script`, refreshes the local quota cache, and prints account, vendor, window, used/limit, and remaining columns. Providers without a usage adapter appear as `(no usage api)`; provider/script failures appear as row errors and do not fail the command unless local config or state cannot be opened.

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
  scripts/claude-code-cwd \
  scripts/codex-cwd \
  ~/.local/bin/
```

The bundled reference adapters are also available as release assets. For binary installs, use `gh release download` from the same release tag as the `oulipoly-agent-runner` binary; script and binary versions must match for body ingestion to work, because stale scripts may silently omit `body` and leave new ingests with empty `session_turns.body`.

```bash
gh release download v0.1.X --repo nestharus/agent-runner \
  --pattern "claude-code-turns" --pattern "codex-turns" \
  --pattern "anthropic-usage" --pattern "chatgpt-usage" \
  --pattern "zai-usage" \
  --pattern "claude-code-locate-transcript" \
  --pattern "codex-locate-transcript" \
  --pattern "claude-code-cwd" \
  --pattern "codex-cwd" \
  --dir ~/.local/bin/
chmod +x ~/.local/bin/{claude-code-turns,codex-turns,anthropic-usage,chatgpt-usage,zai-usage,claude-code-locate-transcript,codex-locate-transcript,claude-code-cwd,codex-cwd}
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
  "is_sidechain": true,
  "body": [{"type": "text", "text": "..."}]
}
```

`parent_turn_id` and `is_sidechain` are **optional**. Adapters that don't track within-session parentage emit only the first four fields; the runner treats those turns as linear with `is_sidechain = false`. The Claude Code reference adapter passes through the raw `parentUuid` and `isSidechain` fields it sees in Claude's per-session JSONL — those surface as branch counts in `trace --json`'s `session.sidechain_turn_count`.

Turn-script adapters also emit body content as JSON in the canonical content shape when available, and `state.db` stores this content directly in `session_turns.body`. Adapters that cannot extract content omit the `body` field, resulting in rows where `session_turns.body` is `NULL` (legacy-style rows without stored bodies).

Idempotent — re-running with no source changes outputs nothing. The runner's `session_turns` table has `UNIQUE(provider, session_id, turn_id)` so duplicate emission is also tolerated.

**Reference turn adapters** (in [`scripts/`](scripts/)):

| Script | Adapts | Storage |
|---|---|---|
| `claude-code-turns BASE_DIR` | Claude Code | JSONL tree under `BASE_DIR`; preserves `parentUuid` + `isSidechain` |
| `codex-turns BASE_DIR` | Codex CLI | Date-sharded JSONL under `BASE_DIR` |

For other CLIs (SQLite history, remote API, etc.), write your own script — see [`scripts/README.md`](scripts/README.md). The application stays format-agnostic; everything CLI-specific lives in adapter scripts.

### Optional: `transcript_locator`

`sessions.toml` entries may also declare a `transcript_locator` script that resolves a `session_id` to the absolute path of its raw transcript file. Used by `trace --json` to fill `transcript_path` and `transcript_state` per node. The lookup is **lazy at trace time** — never at invocation time — so unused providers add no work.

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
- `--inline-transcript` (requires `--json`) — embed DB-stored transcript turns inline as objects with `turn_id`, `role`, `timestamp`, `body_state`, and `content`; `body_state` is `"available"` when body is stored, and `"missing"` for legacy rows where `content` is `null`
- `--transcript` (human mode only; conflicts with `--json`) — append a transcript footer
- `--max-depth N` — truncate descendants past depth N

### Locating a Session

```bash
oulipoly-agent-runner session locate <session-id>
oulipoly-agent-runner session locate <session-id> --json
```

`session locate` resolves one provider session to stable metadata. Output is always JSON. On success, stdout is one compact single-line JSON object with a trailing newline. `--json` is accepted for symmetry with `trace --json`; it does not change the output format.

Success requires a canonical file-backed transcript. If the location cannot be completed, `session locate` emits no partial JSON on stdout. Failures return a JSON error object on stderr instead.

Success JSON fields are required:

- `session_id` — active provider session UUID
- `chain_id` — logical chain UUID
- `provider_name` — active provider/account name
- `storage_type` — one of `claude_code`, `codex_session`, `other`
- `jsonl_path` — canonical absolute UTF-8 transcript path
- `workspace_root` — canonical absolute UTF-8 workspace path
- `transcript_state` — `available` on success
- `mutable` — boolean read-time eligibility hint

`mutable: true` is a **read-time eligibility hint** derived from current chain, storage, resume, transcript, and workspace state. It is not a safety lock or write permission, and consumers should not treat it as permission to mutate. Cross-process write safety requires the future `pause-handshake` sibling feature.

Exit codes:

| Exit | Error code | Trigger |
|------|------------|---------|
| `0` | none | Success: complete `SessionMetadata` JSON on stdout. |
| `1` | `operational-error` | DB open/read failure, model-load failure, JSON serialization failure, unexpected I/O outside transcript/storage classification. |
| `2` | `invalid-session-id` | Non-UUID `<session-id>` (parse before DB open). Clap structural usage errors may use clap's default formatting. |
| `10` | `session-not-found` | `MetadataError::SessionNotFound`; partial-DB segmentless sessions also map here. |
| `11` | `ambiguous-session` | `MetadataError::AmbiguousSession`; only when resolver returns `ResumeError::Ambiguous`. |
| `12` | `unsupported-storage` | `MetadataError::UnsupportedStorage`; absent storage block, transcript not canonical/available, workspace_root not derivable, etc. No partial success JSON ever emitted. |

`trace --json` remains invocation-tree scoped and degrades to `no_locator` or `missing` transcript states for diagnostics. `session locate` is action-oriented and refuses partial locations with `unsupported-storage`.

### Probing the Session Surface

```bash
oulipoly-agent-runner session schema-probe
```

`session schema-probe` is a **read-only** inspector for the default state DB. It reports binary metadata, schema versions, structural compatibility, supported feature flags, and a `safe_for_import_replace` predicate. It does not open the DB read-write, does not run migrations, and does not create state if it is missing.

Use it before scripting any other `session` subcommand to confirm the local DB is current and the feature you want is supported on this build.

| Exit | Error code | Trigger |
|------|------------|---------|
| `0` | none | Probe completed (DB present and compatible, OR DB absent — both are non-error states). |
| `1` | `operational-error` | DB unreadable or corrupt at the OS level. |
| `14` | `schema-incompatible` | DB present but `user_version` is below the binary's minimum supported schema version. |

### State DB Schema Migrations

The persistent runner state DB uses embedded SQLite migrations from
`crates/oulipoly-state/migrations/`. The schema version constants live in
`crates/oulipoly-state/src/schema.rs` as `CURRENT_SCHEMA_VERSION` and
`MINIMUM_SUPPORTED_SCHEMA_VERSION`; `StateDb::open` applies missing forward
migrations before normal state reads or writes.

To add a migration, bump `CURRENT_SCHEMA_VERSION`, add a
`NNNN_description.sql` file whose prefix is the target `PRAGMA user_version`,
register it in `crates/oulipoly-state/src/migrations.rs`, and run the Rust
workspace migration tests. Use migration files for schema changes; legacy
`ensure_*_schema` helpers are only a guarded repair allow-list.

Use `session schema-probe` for read-only diagnostics. Use `agents migrate-db`
for the existing session-chain/compaction backfill. Use
`agents migrate --rebuild` only for destructive recovery: it backs up
`state.db`, `state.db-wal`, and `state.db-shm`, then creates a fresh current DB,
so historical live rows are not preserved except in the backup.

### Exporting a Session

```bash
oulipoly-agent-runner session export <session-id>
```

`session export` resolves the session, opens the provider transcript read-only, and emits **canonical JSONL** on stdout — one record per turn with stable, provider-agnostic fields plus a `source` block carrying provenance for the emitted content. When the provider JSONL is present, `source` carries the byte range and SHA-256 preimage of the source line. When the provider JSONL is missing or unreadable, export falls back to bodies stored directly in `state.db`; those fallback records use `source.storage_type = "state_db"`, `source.jsonl_path = "db://session_turns/<row_id>"`, `source.line = <row_id>`, zero byte offsets, and `source.sha256` computed from the stored body JSON. The default `--format canonical-jsonl` is the only supported format today.

Canonical record fields:

```json
{
  "session_id": "…", "provider_name": "…", "turn_id": "…",
  "role": "user|assistant", "timestamp": "<RFC 3339>",
  "content": [{"type": "text|tool_use|…", "text": "…"}],
  "source": {
    "storage_type": "claude_code|codex_session|state_db",
    "jsonl_path": "<absolute path or db://session_turns/<row_id>>",
    "line": 1, "byte_start": 0, "byte_end": 192,
    "sha256": "<hex>"
  },
  "unsupported_record": false
}
```

The reader is the round-trip oracle for `session import-replace`: `sha256(session export <id>)` byte-equals `import-replace`'s `preimage_sha256` / `postimage_sha256` for the same transcript file.

| Exit | Error code | Trigger |
|------|------------|---------|
| `0` | none | Compact JSONL on stdout. |
| `1` | `operational-error` | DB / I/O failure. |
| `2` | `invalid-session-id` | Non-UUID input or unsupported `--format`. |
| `10` | `session-not-found` | No chain owns this session. |
| `11` | `ambiguous-session` | Multiple chains match; refuses partial answer. |
| `12` | `unsupported-storage` | Provider has no `session_storage` config or transcript is not canonical. |
| `15` | `malformed-provider-transcript` | Provider transcript is invalid (out-of-order timestamps, missing required fields, non-UTF-8). |

### Replacing a Session Transcript

```bash
# stdin
oulipoly-agent-runner session export <id> | edit-canonical | \
  oulipoly-agent-runner session import-replace <id>

# --from-file
oulipoly-agent-runner session import-replace <id> --from-file ./edited.jsonl

# preimage-gated (recommended): refuses if the transcript drifted between read and write
PREIMAGE=$(oulipoly-agent-runner session export <id> | sha256sum | awk '{print $1}')
…edit canonical…
oulipoly-agent-runner session import-replace <id> \
  --preimage-sha256 "$PREIMAGE" --from-file ./edited.jsonl
```

`session import-replace` accepts **canonical JSONL only** (the same shape `session export` emits). It renders that input back into provider-native bytes, swaps the on-disk transcript atomically, replaces the session's `session_turns` rows, updates stored body bytes alongside metadata, and emits a receipt JSON to stdout:

```json
{
  "session_id": "…", "provider_name": "…",
  "storage_type": "claude_code|codex_session",
  "operation": "import-replace",
  "preimage_sha256": "<canonical export hash before>",
  "postimage_sha256": "<canonical export hash after>",
  "jsonl_path": "<absolute path>",
  "state_updated": true,
  "committed_at": "<RFC 3339>"
}
```

Atomicity contract:
- The transcript-on-disk write and the `session_turns` row replacement happen in a two-phase commit (rename + SQLite transaction). Either both land or neither lands.
- Acquires an advisory `SessionLock` for the duration; concurrent import-replace on the same session returns exit `13` `session-busy`.
- If the runner crashes between the rename and the SQLite commit, the next agent-runner startup scans `<state-data-dir>/replace_journal/` and either re-applies the DB change or quarantines the journal entry.
- `--preimage-sha256` is the optimistic-concurrency guard: if it does not match the canonical hash of the transcript at acquire time, the operation aborts before any mutation.
- Lossless rendering is required; canonical records that cannot be losslessly rendered (non-text content chunks without `text`, unsupported roles, etc.) cause exit `15` with no transcript or DB mutation.

| Exit | Error code | Trigger |
|------|------------|---------|
| `0` | none | Receipt JSON on stdout; transcript and DB updated atomically. |
| `1` | `operational-error` | I/O, lock, or SQLite failure outside the structured contract. |
| `2` | `invalid-session-id` / `invalid-argument` | Non-UUID input or malformed `--preimage-sha256`. |
| `10` | `session-not-found` | No chain owns this session. |
| `11` | `ambiguous-session` | Multiple chains match. |
| `12` | `unsupported-storage` | Provider has no compatible `session_storage` block. |
| `13` | `session-busy` | Another holder owns the `SessionLock`. |
| `14` | `schema-incompatible` | State DB schema is unsupported; run `agents migrate --rebuild` only after deciding to replace the live DB from backup. |
| `15` | `invalid-input-transcript` / `preimage-mismatch` | Canonical input is malformed, lossy under the renderer, or `--preimage-sha256` did not match. |

Anti-scope: `session import-replace --recover` and `migrate-db --recover` do not exist. Import-replace recovery runs implicitly at the top of every CLI invocation after the state DB migration gate.

### Session Locks

```bash
TOKEN=$(oulipoly-agent-runner session pause-handshake <session-id> --ttl-ms 30000 | jq -r '.token')
# … hold the lease while you read or modify the transcript out-of-band …
oulipoly-agent-runner session resume-handshake <session-id> --token "$TOKEN"
```

`pause-handshake` acquires an advisory lease on a session and emits a JSON object with `token`, `expires_at`, and the resolved storage block. While the lease is held, `session import-replace` on the same session returns exit `13` `session-busy`. The lease auto-expires after the TTL (default ≈5 minutes); `resume-handshake` releases it explicitly.

The lock is advisory: it coordinates well-behaved `oulipoly-agent-runner` consumers but does not block external writers from mutating the on-disk transcript. Use it together with `session import-replace`'s `--preimage-sha256` for safe concurrent edits.

### Cross-invocation tracking

When `oulipoly-agent-runner` invokes a wrapped CLI that itself spawns another `oulipoly-agent-runner` (e.g. via the `Task` tool in Claude Code), the runner propagates `OULIPOLY_PARENT_INVOCATION` as an env var to the subprocess. The child's invocation row records `parent_invocation_id` pointing at the parent. `trace` walks that tree.

If the env var is malformed, points at an unknown invocation, or has an invalid UUID, the child silently treats itself as a root invocation (no panic; observable via `parent_id = null` in trace output).

### Configuring session capture

To populate `trace`'s `session.id` and `transcript_path`, add a `session_capture` block to the provider in `providers.toml`:

```toml
# Claude Code: force a runner-generated UUID via --session-id and verify readback
[claude.session_capture]
kind          = "forced_flag_verified"
flag          = "--session-id"
readback_args = ["--verbose", "--output-format", "stream-json"]
```

```toml
# Codex: parse the `thread.started` event from --json mode; restore plain text from -o tmpfile
[codex.session_capture]
kind              = "stdout_json_event"
json_flag         = "--json"
last_message_flag = "-o"
event_type        = "thread.started"
event_id_path     = "thread_id"
```

Without `session_capture`, invocations record `session_capture_method = "none"` and `trace` shows `transcript_state = "unresolved"` — clean degradation, no breakage.

### Resuming a session

For top-level interactive flows, use `--new` to start a fresh
default-provider session and `--resume <UUID>` to continue an existing one.
The model-specific `repl --resume <UUID>` form remains available when the
caller wants to name the model explicitly.

When a provider declares a `[<provider>.resume]` block in `providers.toml`, `repl --resume <UUID>` looks the session up across all providers (via the `session_turns` ingest table), validates that the owning provider belongs to the requested model's provider pool when a model is known, and composes the right interactive resume argv.

For non-interactive answer handoff, use `resume`:

```bash
oulipoly-agent-runner resume -m claude-opus --session-id 9e69e8cc-616d-4640-bf1d-96f5391b1a2e -f answer.md
oulipoly-agent-runner resume -m codex-high --session-id 5169694d-de0f-40d1-890c-6e28e55bab27 --prompt "answer text"
```

`resume` uses the same owner lookup and provider-pool validation as `repl --resume`, but launches the provider's one-shot `args` with the resume strategy and answer payload. It records `resume_acceptance` in `trace`.

With a model, one-shot spawn is `providers[name].command + providers[name].args + model.providers[name].args + resume_args`. Without a model, the model TOML is not loaded for spawn; the runner uses only `providers[name].command + providers[name].args + resume_args`, so the upstream CLI uses its own default model.

When load balancing migrates a Claude Code session during `repl --resume` or `resume`, the copied target JSONL is re-anchored under the Claude project directory derived from the child process working directory (`-p/--project`, or the current directory when omitted). This matches Claude Code's own cwd-scoped `--resume <UUID>` lookup; the resume argv remains just the configured resume flag or subcommand plus the session id.

The runner always emits a short selection line on stderr regardless of TTY:

```
[resume] -> claude2
```

When a session id matched multiple providers (rare; requires cross-provider session id collisions in the ingest table), a longer detail line lists all matches but only when stderr is **not** a TTY:

```
[resume] session 9e69e8cc-... matched claude2, claude3; selected claude2 by latest turn timestamp
```

If no model can be inferred for a UI-started session, the runner uses the active provider's `providers.toml` command shape only. No model TOML is consulted and no model flag is injected.

Resume failures all exit `1` with a specific stderr message:

- **No session found** — the UUID is not in `session_turns` (typically: session ingestion isn't configured, or the provider's local store has dropped the session)
- **Invalid session UUID** — the input wasn't a valid full UUID (no prefix matching)
- **Ambiguous session** — the UUID maps to multiple recent chains; rerun with a printed `chain_id`
- **Provider/model mismatch** — the resolved provider is not in the requested model's provider pool. The error suggests other models that include the resolved provider, e.g. `Try a model that includes claude2: claude-opus, claude-sonnet`.
- **Provider not configured** — the active provider is missing from `providers.toml`, so the runner has no command shape to spawn.
- **Provider has no `[<provider>.resume]` block** — the resolved provider exists but doesn't declare a resume strategy.

The invocation row records `session_capture_method = "resumed"` and the user-supplied `session_id` *before* spawn. This means `trace` can show what session the runner attempted to resume even if the wrapped CLI rejects the id (e.g. "No conversation found"). Trace renders the session as `Resume target: <UUID>` instead of `Session: <UUID>` to make this distinction explicit, and adds a warning: `session marked as attempted resume target; child acceptance is not confirmed by this row — check exit_code and recent_errors for outcome`.

Plain `repl <model>` (no `--resume`) records `session_capture_method = "none"`; the proposal explicitly does not invent a fresh-session capture mechanism for interactive launches.

### Inspecting via SQL

SQL is for ad-hoc debugging when `trace` or `oulipoly-agent-runner session locate <session-id>` do not answer the question. The supported command path for stable session metadata is `oulipoly-agent-runner session locate`; the SQLite schema is not the contract.

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
  providers.toml       Per-provider runtime config, resume/session storage, quota scripts
  sessions.toml        Per-provider turn ingestion + transcript locator adapters
                       (optional, for accurate cross-source projection + trace inspection)
  models/              Model configs (one .toml per model; provider entries contain
                       provider names plus model-specific args only)
  agents/              Agent configs (one .md per agent)
```

### Adding a Model

Create or update the account entry in `providers.toml`, then create a `.toml` file in the models directory. The filename becomes the model name.

When adding a new Claude provider entry, see [`docs/architecture/claude-proxy-mcp-launch-shape.md`](docs/architecture/claude-proxy-mcp-launch-shape.md) before configuring any MCP-related tool filters in `args` or `interactive_args` for the Claude provider.

**Runtime provider entry:**
```toml
[claude]
command = "claude"
args = ["-p"]
interactive_args = []
prompt_mode = "stdin"

[claude.resume]
kind = "flag"
flag = "--resume"
```

**Text model (single provider):**
```toml
[[providers]]
name = "claude"
args = ["--model", "haiku"]

[[inputs]]
name = "prompt"
type = "string"
required = true
default_input = true
description = "The text prompt"
```

**Text model (multiple providers, load balanced):**
```toml
[[providers]]
name = "codex"
args = ["-m", "gpt-5.3-codex"]

[[providers]]
name = "codex2"
args = ["-m", "gpt-5.3-codex"]

[[inputs]]
name = "prompt"
type = "string"
required = true
default_input = true
description = "The text prompt"
```

**Image/video model with typed inputs:**
```toml
[[providers]]
name = "atlas-i2v-fast"

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

## Implementing a Provider

To implement a provider, begin with the [`Provider Contract Crate (oulipoly-provider)`](AGENTS.md#provider-contract-crate-oulipoly-provider) reference in `AGENTS.md`, which documents all per-concern traits, the `ProviderCapabilities` aggregation type, and the `CapabilityError` surface.

`providers.toml` accounts configure runtime dispatch to CLI binaries; `ProviderCapabilities` trait implementations are a separate Rust interface layer that the runner may call for per-concern operations. The two are currently independent; a `providers.toml` entry does not need a corresponding `ProviderCapabilities` implementation to function.

**Steps:**

1. Implement the relevant per-concern traits from `crates/oulipoly-provider/`. Each trait governs one operational concern; implement only the traits your provider supports.

2. Populate a `ProviderCapabilities` bundle with only the capabilities your implementation supports. Each field is `Option`-typed and defaults to `None`.

3. Optionally declare the implementation in a model TOML via `provider = { ... }`. The four flavors are `path`, `crate` (plus optional `version`), `binary`, and `script`.

> **Parse-only in this release:** Dynamic loading and runtime dispatch are not implemented in this release; the `provider = { ... }` field is recorded by the parser but has no effect on routing or execution.

**Provider vocabulary précis** (full definitions in [AGENTS.md §Provider Term Glossary](AGENTS.md#provider-term-glossary)):

- **account** — a runtime `providers.toml` entry keying per-account quota and session state.
- **pool-member** — a `[[providers]]` entry in a model TOML that references an account and supplies model-specific arguments.
- **implementation-reference** — the `provider = { ... }` TOML field pointing to a `ProviderCapabilities` implementation (parse-only in this release).
