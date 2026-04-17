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
  {"session_id": "...", "turn_id": "...", "timestamp": "<RFC 3339>", "role": "user|assistant"}
  ```
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
| `claude-code-turns BASE` | Claude Code | JSONL tree under `BASE` (`~/.claude*/projects`) |
| `codex-turns BASE`       | Codex CLI   | Date-sharded JSONL under `BASE` (`~/.codex*/sessions/YYYY/MM/DD/rollout-*.jsonl`) |

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

## Quota scripts (`providers.toml`)

A **quota script** prints `{"used_percent": <0..100>, "resets_at": "<ISO8601>"}`
on stdout. Reference: see `anthropic-usage` and `chatgpt-usage` (typically
installed separately under `~/.local/bin/`). Same adapter pattern.
