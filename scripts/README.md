# Adapter Scripts

These are reference adapter scripts for `oulipoly-agent-runner`. They aren't
linked into the binary — they're standalone executables wired in via TOML
config, the same way `anthropic-usage` is wired into `providers.toml`.

## Turn scripts (`sessions.toml`)

A **turn script** lets the runner count how many assistant turns each
provider account has issued — across *all* CLI usage, including direct user
prompts that bypass agent-runner. Used by the load balancer's quota
projection.

### Contract

A turn script:

- Receives `STATE_DIR` as an env var — a writable dir it may use for its own
  incremental cursor bookkeeping (e.g. an mtime watermark).
- May take any positional args (typically a base path / DB / endpoint).
- Emits **one JSON object per line** on stdout, one per turn:
  ```json
  {
    "session_id":     "...",
    "turn_id":        "...",
    "timestamp":      "<RFC 3339>",
    "role":           "user|assistant",
    "parent_turn_id": "<turn_id|null>",
    "is_sidechain":   true
  }
  ```
- `parent_turn_id` and `is_sidechain` are **optional**. Adapters that don't
  track within-session parentage (or that adapt CLIs without that surface,
  like Codex) emit only the first four fields and the runner treats those
  turns as linear with `is_sidechain = false`.
- Returns 0 on success. Non-zero with diagnostic on stderr on failure.
- Is **idempotent**: re-running with no source changes outputs nothing.
  (The runner's `session_turns` table has `UNIQUE(provider, session_id, turn_id)`,
  so duplicate emission is also tolerated — it just wastes work.)

The runner doesn't care whether the underlying CLI stores sessions as JSONL
files, SQLite, a remote API, or anything else. The script is the bridge.

### Wiring

In `~/.config/oulipoly-agent-runner/sessions.toml`:

```toml
[claude]
turn_script = "claude-code-turns ~/.claude/projects"

[claude2]
turn_script = "claude-code-turns ~/.claude2/projects"

[codex]
turn_script = "codex-turns ~/.codex/sessions"

# Optional: override where the script keeps its cursor state.
# Defaults to <data_dir>/oulipoly-agent-runner/sessions/<provider>.
[codex2]
turn_script = "codex-turns ~/.codex2/sessions"
state_dir   = "~/.cache/oulipoly/codex2-cursor"
```

### Bundled reference scripts

| Script | What it adapts | Storage assumption |
|---|---|---|
| `claude-code-turns BASE` | Claude Code | JSONL tree under `BASE` (`~/.claude*/projects`); preserves `parentUuid` + `isSidechain` so `trace --json` surfaces Task-tool subagent branch counts |
| `codex-turns BASE`       | Codex CLI   | Date-sharded JSONL under `BASE` (`~/.codex*/sessions/YYYY/MM/DD/rollout-*.jsonl`); linear (Codex has no `parentUuid` analogue) |

Install them somewhere on `$PATH` (e.g. `~/.local/bin/`):

```bash
install -m 755 claude-code-turns codex-turns ~/.local/bin/
```

### Writing your own

For a CLI that stores history in SQLite, your script might be:

```bash
#!/usr/bin/env bash
DB="$1"
LAST="$(cat "$STATE_DIR/last_ts" 2>/dev/null || echo '1970-01-01T00:00:00Z')"
sqlite3 "$DB" "
  SELECT json_object(
    'session_id', session_id,
    'turn_id',    message_id,
    'timestamp',  created_at,
    'role',       role
  )
  FROM messages
  WHERE created_at > '$LAST' AND role IN ('user', 'assistant');
"
sqlite3 "$DB" "SELECT max(created_at) FROM messages;" > "$STATE_DIR/last_ts"
```

For a CLI with a remote API, the script `curl`s the API and emits the
response. For a CLI that doesn't expose history at all (OpenCode, currently)
no turn script is possible — that provider falls back to invocation-count
balancing.

## Transcript locators (`sessions.toml`)

A **transcript locator** resolves a known `session_id` back to the absolute
path of its raw transcript file. Used by `oulipoly-agent-runner trace
<uuid> --json` to populate `transcript_path` / `transcript_state` per node.
The lookup runs **lazily at trace time** — never at invocation time — so
unused providers cost nothing.

### Contract

- Receives `STATE_DIR` and `SESSION_ID` env vars.
- May take any positional args (typically a base path).
- Prints **one absolute path** on stdout. Empty stdout = error (the locator
  contract requires a single line).
- Returns 0 on success; non-zero on failure (e.g. session not found).
- Idempotent and side-effect-free.

### Wiring

Extend the same `sessions.toml` entry with `transcript_locator`:

```toml
[claude]
turn_script        = "claude-code-turns ~/.claude/projects"
transcript_locator = "claude-code-locate-transcript ~/.claude/projects"
```

When unset, `trace` shows `transcript_state = "no_locator"` for that
provider — graceful degradation.

### Bundled reference locators

| Script | Resolution strategy |
|---|---|
| `claude-code-locate-transcript BASE` | Direct filename match `<session_id>.jsonl` (Claude Code's naming convention), with content-based fallback for older versions |
| `codex-locate-transcript BASE`       | Filename suffix match `rollout-*-<session_id>.jsonl`, with `session_meta.payload.id` content fallback |

### Writing your own

Same shape as turn scripts — `sh -c <script>` with env vars. Example for
a CLI that names files by session id:

```bash
#!/usr/bin/env bash
BASE="$1"
find "$BASE" -name "${SESSION_ID}.jsonl" -print -quit | head -1
```

## Session capture (model TOML, `[providers.session_capture]`)

Trace's `session.id` is populated by **session capture**, declared on
each provider in the model TOML. Two declarative strategies exist —
the runner does NOT know about specific CLIs; everything is configured.

```toml
# Force a runner-generated UUID via a CLI flag, then verify the CLI
# actually used it by parsing the readback from its initial output.
[providers.session_capture]
kind          = "forced_flag_verified"
flag          = "--session-id"
readback_args = ["--verbose", "--output-format", "stream-json"]
```

```toml
# Run the CLI in a structured-event mode, parse the session id from a
# named JSON event on stdout, then restore plain-text stdout from a
# tmpfile the CLI writes to.
[providers.session_capture]
kind              = "stdout_json_event"
json_flag         = "--json"
last_message_flag = "-o"
event_type        = "thread.started"
event_id_path     = "thread_id"
```

Without `[providers.session_capture]`, the column records `"none"` and
trace shows `transcript_state = "unresolved"` — graceful degradation
again, no breakage.

`session_capture` is the **fresh-session** capture mechanism for one-shot
runs. It is intentionally bypassed by `oulipoly-agent-runner repl --resume
<UUID>`: the user has already supplied the session id explicitly, so the
runner records `session_capture_method = "resumed"` directly and never
runs the capture parser. See the README's "Resuming a session" section
for the parallel `[providers.resume]` declaration that controls how
the user-supplied UUID is composed onto the wrapped CLI's argv.

## Quota scripts (`providers.toml`)

A **quota script** emits multi-window quota data on stdout:

```json
{
  "windows": [
    {"used_percent": 23, "resets_at": "2026-04-23T19:00:00Z"},
    {"used_percent": 45, "resets_at": "2026-04-17T15:00:00Z"}
  ]
}
```

Backwards-compat: the legacy single-window shape `{"used_percent": X,
"resets_at": "..."}` is still parsed and treated as one window.

Reference: see `anthropic-usage` (5h + 7d windows from
`/api/oauth/usage`) and `zai-usage` (GLM via z.ai). Same adapter pattern
as turn scripts.
