# 1. Summary

This proposal adds a durable invocation UUID, parent edge, provider/account name, and nullable session correlation to SQLite, then adds a `trace` subcommand that walks the invocation tree from SQLite and dereferences transcripts lazily from raw CLI logs. It keeps transcript content out of SQLite and makes degraded states explicit.

The revision is driven by `VALUES.md`, not by reviewer preference alone. Two changes are load-bearing:

- Session capture becomes a declarative `session_capture` field on `ProviderConfig`, so the runner dispatches generically by configured strategy instead of sniffing CLI identity. That is required by V1, V2, and V3.
- Transcript lookup moves into an optional `transcript_locator` adapter declared in `sessions.toml`, so the runner stays ignorant of provider-specific storage layouts. That is required by V1 and V2, even though storing a resolved path in SQLite would be cheaper at trace time under V8.

# 2. Data model

This changes the invocation write pattern from `insert on completion` to `insert on spawn, update on finish`. That lifecycle change is the prerequisite for returning a durable ID before the wrapped CLI exits.

`invocations` still keeps its integer primary key for local joins, but adds a caller-visible UUID and explicit lifecycle fields:

```sql
CREATE TABLE invocations_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    invocation_uuid TEXT NOT NULL UNIQUE,
    model_name TEXT NOT NULL,
    provider_name TEXT,
    provider_index INTEGER NOT NULL,
    parent_invocation_id INTEGER REFERENCES invocations_new(id),
    session_id TEXT,
    session_capture_method TEXT,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
    success INTEGER,
    exit_code INTEGER,
    error_category TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT,
    CHECK (provider_name IS NOT NULL OR status = 'legacy')
);

CREATE INDEX idx_invocations_uuid
    ON invocations_new (invocation_uuid);
CREATE INDEX idx_invocations_parent
    ON invocations_new (parent_invocation_id, created_at);
CREATE INDEX idx_invocations_provider_created
    ON invocations_new (provider_name, created_at);
CREATE INDEX idx_invocations_session
    ON invocations_new (provider_name, session_id);

ALTER TABLE session_turns ADD COLUMN parent_turn_id TEXT;
ALTER TABLE session_turns ADD COLUMN is_sidechain INTEGER NOT NULL DEFAULT 0;

CREATE INDEX idx_session_turns_session_ts
    ON session_turns (provider_name, session_id, timestamp);
CREATE INDEX idx_session_turns_parent
    ON session_turns (provider_name, session_id, parent_turn_id, timestamp);
```

Notes:

- `provider_name` must live on `invocations` because the returned ID is composite `{"source":"<provider>","id":"<uuid>"}` and trace/transcript resolution need a durable account key, not a future reinterpretation of `(model_name, provider_index)`.
- `session_id` stays nullable. A provider without declared capture, or a failed capture, remains traceable as an invocation tree node with `session_capture_method = 'unresolved'`.
- `session_capture_method` is persisted because V10 requires degraded capture to be observable.
- `status = 'legacy'` is used only for migrated historical rows whose provider/account cannot be resolved cleanly during backfill. This removes the old `legacy:<model>:<idx>` sentinel and keeps `provider_name` a real value domain.

Migration path for the current `invocations` table, which is currently 76 rows and grows over time:

1. Run the migration inside a single transaction.
2. Create `invocations_new`.
3. Read current model config once and resolve `(model_name, provider_index) -> provider_name`.
4. Copy old rows:
   - If the provider resolves cleanly, assign a fresh UUIDv4, copy `provider_name`, set `parent_invocation_id = NULL`, `session_id = NULL`, `session_capture_method = NULL`, `status = CASE success WHEN 1 THEN 'succeeded' ELSE 'failed' END`, and `finished_at = created_at`.
   - If the provider does not resolve cleanly, assign a fresh UUIDv4, keep the original `model_name` and `provider_index`, set `provider_name = NULL`, `parent_invocation_id = NULL`, `session_id = NULL`, `session_capture_method = NULL`, `status = 'legacy'`, and `finished_at = created_at`.
5. Drop old `invocations`, rename `invocations_new` to `invocations`, recreate indexes, commit.

That preserves all historical rows without polluting `provider_name`, and it makes incomplete historical reconstruction explicit instead of hiding it behind an in-band sentinel. Foreign-key enforcement stays on throughout; the self-reference is valid because migrated rows start with `parent_invocation_id = NULL`.

# 3. Cross-invocation parent propagation

Use one environment variable:

```text
OULIPOLY_PARENT_INVOCATION={"source":"claude2","id":"9e69e8cc-616d-4640-bf1d-96f5391b1a2e"}
```

This remains raw JSON because the composite ID is the stable over-the-wire contract and V13 prefers self-contained identifiers. The runner already spawns the wrapped process through `std::process::Command`, not through a shell, so the first hop is safe. Wrapper scripts that re-export the variable must quote it correctly; that requirement belongs in the docs.

Runtime behavior:

1. Parse `OULIPOLY_PARENT_INVOCATION` if present.
2. Validate strict JSON shape and UUID.
3. Resolve `(source, id)` to a parent row if it exists.
4. Insert the new invocation row with `status = 'running'`.
5. Emit `OULIPOLY_INVOCATION=...` on stderr.
6. Spawn the wrapped CLI with `OULIPOLY_PARENT_INVOCATION` overwritten to the current invocation’s composite JSON.
7. On completion, update `status`, `success`, `exit_code`, `error_category`, and `finished_at`.

If the parent env var is absent, invalid, or unresolved, the invocation is recorded as a root. That follows V11: explicit propagation is primary, inference is not.

# 4. Session correlation at spawn time

The runner must not decide “this is Codex” by sniffing the command basename. That would violate V1 and V3. The fix is a declarative `session_capture` field on `ProviderConfig`, because execution-time capture is part of the provider contract, not a hardcoded executor branch.

Proposed shape in model TOML:

```toml
[[providers]]
name = "claude2"
command = "claude"
args = ["-p"]
session_capture = { kind = "forced_flag_verified", flag = "--session-id", readback_args = ["--verbose", "--output-format", "stream-json"], readback_event_type = "system", readback_event_subtype = "init", readback_field = "session_id" }

[[providers]]
name = "codex"
command = "codex"
args = ["exec"]
session_capture = { kind = "stdout_json_event", event_args = ["--json"], event_type = "thread.started", event_field = "thread_id", output_last_message_flag = "--output-last-message" }
```

Initial strategy kinds:

- `none`: current executor behavior, raw stdout bytes passed through unchanged.
- `forced_flag_verified`: generate a requested session UUID, pass it via the configured flag, and verify the CLI honored it by reading back a structured startup event from the same invocation.
- `stdout_json_event`: enable a structured event stream, extract the session/thread id from the configured event, and reconstruct final plain-text stdout from the configured last-message file.

This is still a small surface, but it is declarative rather than procedural. A new provider account is added by config. A new CLI that fits one of the generic strategies is added by script + TOML with no runner code change, which is the V2 bar.

Provider-specific initial mappings:

- Claude Code uses `forced_flag_verified`.
  - The runner may reuse `invocation_uuid` as the requested session UUID for convenience, but `invocation_uuid` and `session_id` remain separate columns and separate concepts.
  - The readback step is mandatory. If the returned `system.init.session_id` is absent or differs from the requested id, capture fails closed and the invocation records `session_id = NULL`, `session_capture_method = 'unresolved'`.
- Codex uses `stdout_json_event`.
  - The runner enables `--json`, extracts `thread.started.thread_id`, and writes the final assistant message back to stdout from the configured `--output-last-message` file.
  - If the event stream is absent or malformed, capture fails closed with `session_id = NULL`, `session_capture_method = 'unresolved'`.

There is no runner-side filesystem fallback. That is an intentional revision. Timestamp/cwd probing was always fallback-grade, and keeping it in the runner would reintroduce provider-specific storage knowledge through the back door. Under V10 the correct degraded behavior is `unresolved`, not a hidden heuristic.

For text providers that use structured capture, the executor reconstructs the plain model response before writing to stdout so the caller still sees normal text output. Providers with `session_capture = none` keep the current raw-byte stdout path unchanged, which preserves V9 for binary wrappers.

# 5. Session adapters: `turn_script` and `transcript_locator`

`sessions.toml` remains the place where provider storage knowledge lives. This proposal extends that contract in two ways:

```toml
[claude2]
turn_script = "claude-code-turns ~/.claude2/projects"
transcript_locator = "claude-code-locate-transcript ~/.claude2/projects"

[codex]
turn_script = "codex-turns ~/.codex/sessions"
transcript_locator = "codex-locate-transcript ~/.codex/sessions"
state_dir = "~/.cache/oulipoly/codex"
```

First, `turn_script` grows optional fields exactly per V4:

```json
{
  "session_id": "5169694d-de0f-40d1-890c-6e28e55bab27",
  "turn_id": "5290cdae-50af-4a51-99f6-ea629ebc53f9",
  "timestamp": "2026-04-16T16:18:51.314Z",
  "role": "user",
  "parent_turn_id": null,
  "is_sidechain": false
}
```

Rules:

- `parent_turn_id` is optional; absent means “root or unknown”.
- `is_sidechain` is optional; absent means `false`.
- Existing third-party scripts that emit only the current four fields keep working unchanged.

Second, add optional `transcript_locator`:

- Declared per provider in `sessions.toml`.
- Runs lazily, the same way `quota_script` is invoked lazily.
- Receives `SESSION_ID` and `STATE_DIR` in its environment.
- Prints the absolute transcript path on stdout.
- Is idempotent.

Reference adapters:

- `scripts/claude-code-locate-transcript`
- `scripts/codex-locate-transcript`

Trace behavior by state:

- `session_id = NULL` -> `transcript_state = "unresolved"`
- `session_id != NULL` and no locator configured -> `transcript_state = "no_locator"`
- Locator returns a path that exists -> `transcript_state = "available"`
- Locator returns a path that does not exist -> `transcript_state = "missing"`
- Locator errors -> `transcript_state = "locator_error"`

This is the key value call from the revision. Storing `transcript_path` on the invocation row would save one lookup at trace time and aligns with V8 narrowly, but it would force the runner to know provider-specific layout rules at capture time. V1 and V2 are higher-order here: storage knowledge belongs in user-replaceable adapter scripts, not in the runner core.

# 6. The `trace` subcommand

CLI shape:

```bash
oulipoly-agent-runner trace <invocation_uuid>
```

The user passes the bare invocation UUID. The composite JSON remains the stderr/env contract, not the CLI input format.

Default human output:

- ASCII tree, no color
- One line per invocation
- Columns: `uuid`, `provider`, `model`, `status`, `started_at`, `session`, `transcript`

Example:

```text
9e69e8cc-616d-4640-bf1d-96f5391b1a2e  claude2  claude~high  succeeded  2026-04-17T20:03:10Z  9e69e8cc-...  available:/home/nes/.claude2/projects/.../9e69e8cc-....jsonl
└── 019d9d09-2de9-7902-b148-f9f3bed4fa41  codex  codex~high  failed  2026-04-17T20:04:02Z  019d9d09-...  no_locator
```

Tree walk:

- Depth-first preorder
- Children sorted by `created_at`, then `id`
- Cycle protection by visited row id

Trace uses SQLite only for structure. Transcript paths are resolved lazily per node by invoking the configured `transcript_locator` only when the node has a `session_id` and the output mode needs a path.

Structured `--json` output is pointer-first:

```json
{
  "requested_id": "<uuid>",
  "generated_at": "<RFC3339>",
  "root": {
    "invocation": {
      "id": "<uuid>",
      "source": "claude2",
      "model_name": "claude~high",
      "parent_id": null,
      "status": "succeeded",
      "success": true,
      "exit_code": 0,
      "error_category": null,
      "started_at": "<RFC3339>",
      "finished_at": "<RFC3339>"
    },
    "session": {
      "id": "<session_id-or-null>",
      "capture_method": "forced_flag_verified|stdout_json_event|unresolved|null",
      "transcript_state": "available|missing|unresolved|no_locator|locator_error",
      "transcript_path": "<absolute path or null>"
    },
    "children": []
  }
}
```

`trace <uuid> --transcript` appends a rendered transcript for the selected invocation. `trace <uuid> --json --inline-transcript` adds raw provider-native records from the located transcript file. Nothing is persisted back into SQLite.

Direct user CLI sessions with no invocation row are not traversed by `trace`. They remain discoverable through session ingestion and ad-hoc queries, but they are outside the invocation tree.

# 7. ID return contract

Exact stderr line:

```text
OULIPOLY_INVOCATION={"source":"claude2","id":"9e69e8cc-616d-4640-bf1d-96f5391b1a2e"}
```

Rules:

- Trailing newline is mandatory.
- Exactly one such line is emitted per runner process that launches a provider command.
- It is emitted immediately after the `running` row is inserted, before the wrapped CLI is spawned.
- It is always emitted and not flag-gated.

That follows V9 and V15: stdout stays reserved for model output, while metadata is always available to callers on stderr.

# 8. README and script docs

`README.md` updates:

- document the lifecycle change and stderr ID line
- add `trace` usage and degraded transcript states
- document `session_capture` on providers and `transcript_locator` in `sessions.toml`
- explain that wrapper scripts must quote `OULIPOLY_PARENT_INVOCATION` on re-export

`scripts/README.md` updates:

- widen the `turn_script` contract with optional `parent_turn_id` and `is_sidechain`
- document `transcript_locator`
- clarify that locators and turn scripts may use `STATE_DIR` for their own cursors/cache

# 9. Anti-scope

- No transcript content is copied into SQLite.
- No runner-side hardcoded transcript layout rules.
- No web UI, TUI, or live viewer.
- No per-invocation stderr retention beyond the existing outcome fields.
- No account-wide query subcommand such as “show all calls today”.
- No streaming comparison of parallel runs.
- No compliance/customer-scoping model.
- No transcript support guarantee for providers that declare `session_capture = none`.

# 10. Tradeoff justification

- V1, V2, V3: session capture is declarative on `ProviderConfig`; the executor dispatches by configured strategy, not by `match provider_name` or command sniffing.
- V1, V2: transcript lookup lives behind `transcript_locator` scripts in `sessions.toml`; the runner does not know Claude or Codex directory layouts.
- V4: `turn_script` evolves by optional fields only. Existing adapters keep working.
- V5, V6: SQLite holds IDs, edges, timestamps, and capture metadata. Raw logs remain the source of truth for transcript content.
- V8: transcript lookup is lazy on `trace` use. We accept the extra adapter call because it preserves V1 and V2.
- V9, V15: invocation IDs go to stderr, model output stays on stdout, and inline transcripts are query-time only.
- V10: `unresolved`, `no_locator`, `missing`, and `locator_error` are explicit states, not silent empties.
- V11: parent/child invocation edges use explicit env-var propagation, not inference.
- V16: implementation is split into four reviewable PRs, each with independent user value.

# 11. Open questions for the risk gate

- Is the current Codex `thread.started` JSON event stable enough to treat as the long-term `stdout_json_event` reference shape, or should PR-C keep a fixture-based contract test that is expected to fail loudly on upstream drift?
- Are the two initial `session_capture.kind` variants sufficient for the current supported providers, or does the project want to reserve a third generic strategy now for CLIs that expose session IDs only through structured stderr rather than stdout?

Neither question blocks the proposal. Both are contract-shape checks for PR-C.

# 12. Decomposition: 4-PR sequence

PR-A — Durable invocation IDs + lifecycle

- What: rebuild `invocations`; add `invocation_uuid`, `provider_name`, `parent_invocation_id`, `status`, and `finished_at`; switch to insert-on-spawn/update-on-finish; emit `OULIPOLY_INVOCATION` on stderr; propagate `OULIPOLY_PARENT_INVOCATION`; migrate legacy rows transactionally.
- Estimated lines: 450-550 Rust + 40-80 tests/docs.
- Tests: migration test; running-row/finalize-success/finalize-failure lifecycle tests; parent propagation tests.
- User value at merge: callers can capture a durable ID immediately, and future parent/child edges populate even before `trace` exists.
- Dependencies: none.

PR-B — `trace` subcommand

- What: add `trace <uuid>`; tree walk over SQLite; ASCII and `--json` output; explicit `unresolved` and `no_locator` states; optional `--transcript` / `--inline-transcript` plumbing.
- Estimated lines: 300-420 Rust + 40-70 tests/docs.
- Tests: ASCII tree; JSON shape; unresolved session; no-locator state; missing transcript; cycle guard.
- User value at merge: users can inspect the invocation tree immediately, even before session correlation lands.
- Dependencies: PR-A.

PR-C — Session correlation

- What: add declarative `session_capture` to `ProviderConfig`; implement generic executor dispatch for `none`, `forced_flag_verified`, and `stdout_json_event`; Claude readback verification; Codex JSON-event capture with last-message restoration; add `transcript_locator` to `sessions.toml`; ship `claude-code-locate-transcript` and `codex-locate-transcript`.
- Estimated lines: 320-430 Rust + 60-110 scripts/tests/docs.
- Tests: Claude verified-readback fixtures; Codex JSON-event fixtures; plain-text stdout reconstruction; locator success/no-locator/locator-error cases.
- User value at merge: `trace` gains real `session_id` and transcript pointers for providers that declare capture/locator contracts.
- Dependencies: PR-A. Works with PR-B if already landed; otherwise it simply populates data for later trace use.

PR-D — Within-session sidechain capture

- What: add `session_turns.parent_turn_id` and `is_sidechain`; widen `turn_script` contract; update `claude-code-turns`; keep Codex linear until a real parent surface exists; surface sidechain counts/branches in `trace --json`.
- Estimated lines: 120-180 Rust + 20-50 script/tests/docs.
- Tests: backward-compat turn-script parsing; Claude sidechain ingest fixtures; trace sidechain counts.
- User value at merge: within-session Claude Task/subagent branching becomes visible instead of flattened.
- Dependencies: PR-A. Can land independently of PR-B and PR-C after the schema base exists.

# 13. Revision Log

For each medium-or-higher risk finding, this section records the original stance, the revised stance, the value classification, and the scope impact.

| Finding | Category | Original proposal stance | Revised proposal stance | Value cited | Scope impact |
|---|---|---|---|---|---|
| Shortcut F1 (HIGH) | value-aligned correction | Codex capture implicitly required a special executor branch, with no declared dispatch surface. | Added declarative `session_capture` to `ProviderConfig`; executor dispatches by configured strategy kind, not CLI identity. See §4. | V1, V2, V3 | Adds ~40-70 lines in PR-C for config parsing/dispatch, removes the hidden long-term cost of command-name sniffing. |
| Shortcut F2 (HIGH) | value-aligned correction | Transcript dereference implicitly depended on runner knowledge of Claude/Codex storage layouts. | Added `transcript_locator` adapter contract in `sessions.toml`; trace resolves paths lazily through scripts. See §5-§6. | V1, V2, V8, V10 | Adds ~30-60 lines of config/docs plus two small scripts, removes runner-side layout logic from trace/capture. |
| Scope F1 (MED) | value-aligned correction | Codex `--json` rewrite was bundled inside the single proposal scope estimate. | Isolated session correlation work, including Codex JSON-event capture, into PR-C. See §12. | V16 | No net feature increase; review size drops by carving ~320-430 lines into a focused PR. |
| Scope F6 (MED) | value-aligned correction | Section 12 was one bundled line-count estimate across five subsystems. | Replaced with an explicit 4-PR sequence, dependencies, tests, and user value at each merge point. See §12. | V16 | No net scope change; review effort is distributed across four smaller merges. |
| Shortcut F3 (MED) | value-aligned correction | Historical backfill used `provider_name = 'legacy:<model>:<idx>'` and left `'legacy'` orphaned in the status enum. | Replaced sentinel with `status = 'legacy'` only for unmappable historical rows and made `provider_name` nullable in that case. See §2. | V5, V10 | Small net reduction in implementation complexity and cleaner indexes; no feature increase. |
| Shortcut F6 (MED) | value-aligned correction | Claude primary capture trusted `--session-id` if the flag was accepted. | `forced_flag_verified` now requires readback verification before storing `session_id`; mismatch fails closed to `unresolved`. See §4. | V10, V11 | Adds ~15-30 lines in PR-C and removes a silent-wrong correlation path. |

Low-severity editorial findings were also accepted where they did not conflict with values: the migration row count is updated from 61 to “currently 76, grows over time”; the migration is explicitly transactional; the Claude structured-capture path now explains how plain-text stdout is preserved; and the orphan `'legacy'` enum value is now used deliberately rather than left dangling.
