# Audit Risk Assessment: proposals/01-trace-inspection.md

## Verdict: LOW

All load-bearing empirical claims re-verified cleanly on 2026-04-17, the
data model supports every query described in section 6, and each
commitment from the synthesis is honored (or explicitly flagged and
justified). Remaining issues are editorial.

## Findings

### Claude `--session-id` is accepted and echoed back verbatim
- Severity: low
- Claim from proposal: §4 — `forced_flag_verified` generates a UUID,
  passes `--session-id <uuid>` to `claude -p`, and reads back
  `system.init.session_id` from `--verbose --output-format stream-json`
  to confirm the CLI honored it.
- What I verified:
  - `claude --help` lists `--session-id <uuid>  Use a specific session
    ID for the conversation (must be a valid UUID)`.
  - `claude -p --session-id 9e69e8cc-616d-4640-bf1d-96f5391b1a2e
    --verbose --output-format stream-json "Reply OK"` emitted a first
    line `{"type":"system","subtype":"init", ...,
    "session_id":"9e69e8cc-616d-4640-bf1d-96f5391b1a2e", ...}`. The
    requested UUID was returned unchanged, confirming the readback
    contract is real.
- Conclusion: holds exactly as the proposal states.

### Codex `exec --json` emits `thread.started` first with `thread_id`
- Severity: low
- Claim from proposal: §4 — `codex exec --json "Reply OK"` emits
  `thread.started` as the first line with a `thread_id`, and
  `--output-last-message` writes the final plain-text assistant message
  to a file so the runner can restore plain-text stdout.
- What I verified:
  - `codex exec --json "Reply OK"` first line:
    `{"type":"thread.started","thread_id":"019d9d27-73ee-79b3-ac59-8128a2eb5e47"}`.
  - `codex exec --help` lists `-o, --output-last-message <FILE>` and
    `--json  Print events to stdout as JSONL`.
- Conclusion: holds. `codex exec` has no caller-supplied session-id
  flag (only a `resume` subcommand), which matches the proposal's
  decision to use `stdout_json_event` for Codex rather than
  `forced_flag_verified`.

### Executor does not clear env; inherit-overlay propagation is sound
- Severity: low
- Claim from proposal: §3 — `OULIPOLY_PARENT_INVOCATION` can be set on
  the spawned Command and the wrapped CLI will see it because the
  executor never calls `env_clear`.
- What I verified: `src-tauri/src/executor/cli.rs:214-291` constructs
  `Command::new(&parts[0])`, appends args/input flags, optionally sets
  `current_dir`, configures stdio, and spawns. There is no
  `env_clear`, `env_remove`, or `envs(...)` call anywhere in this file.
  The child inherits the runner's environment by default.
- Conclusion: holds. The env-var propagation mechanism is mechanically
  valid.

### Live invocation-row count (minor staleness, not a correctness issue)
- Severity: low
- Claim from proposal: §2 says "currently 76 rows and grows over
  time" (a deliberate revision from the original "61 rows" flagged in
  §13 as a low-severity editorial correction).
- What I verified: live DB count is 78 rows as of 2026-04-17. The
  schema is the pre-migration shape listed in the proposal:
  `(id, model_name, provider_index, success, exit_code, error_category,
  created_at)`, with only three distinct `model_name` values
  (`claude-opus`, `gemini-high`, `gpt-high`) that will need
  `(model_name, provider_index) -> provider_name` resolution during
  migration.
- Conclusion: holds. The migration path iterates rows rather than
  depending on the exact count, so 76 vs 78 is cosmetic.

### Schema additions support every section-6 query
- Severity: low
- Claim from proposal: §2 and §6 — the new columns and indexes make
  the `trace` walk executable without extra joins.
- What I verified: walked the queries:
  - Root lookup from user-supplied UUID: `WHERE invocation_uuid = ?`
    served by `idx_invocations_uuid`.
  - Child walk: `WHERE parent_invocation_id = ? ORDER BY created_at,
    id` served by `idx_invocations_parent`.
  - Per-node session metadata: `session_id`, `session_capture_method`,
    `status`, `success`, `exit_code`, `error_category`, `created_at`,
    `finished_at` — all columns are present on `invocations_new`.
  - Within-session sidechain walk (§2 ALTER TABLE on `session_turns`):
    `WHERE provider_name = ? AND session_id = ? AND parent_turn_id =
    ? ORDER BY timestamp` served by `idx_session_turns_parent`.
  - Linear session walk: `WHERE provider_name = ? AND session_id = ?
    ORDER BY timestamp` served by `idx_session_turns_session_ts`.
- Conclusion: holds. The JSON shape in §6 is expressible as a single
  recursive CTE over `invocations` plus lazy locator calls per node.

### Migration FK handling and rebuild strategy are safe
- Severity: low
- Claim from proposal: §2 — rebuild `invocations` inside a single
  transaction, migrate with `parent_invocation_id = NULL` for historical
  rows, use `status = 'legacy'` for unmappable provider resolutions,
  use `CHECK (provider_name IS NOT NULL OR status = 'legacy')`.
- What I verified:
  - The new table's only FK is the self-reference
    `parent_invocation_id INTEGER REFERENCES invocations_new(id)`.
    Historical rows migrate with NULL, so the FK is trivially
    satisfied.
  - After renaming `invocations_new` → `invocations`, SQLite (3.26+)
    rewrites FK definitions to track the new table name. The self-FK
    continues to resolve correctly.
  - No other table holds an FK into `invocations`: checked
    `session_turns`, `providers`, `provider_quotas`,
    `provider_quota_windows`. Migration does not need cross-table FK
    rewiring.
  - The CHECK constraint and the `status = 'legacy'` path are
    internally consistent — step 4 in §2 uses exactly the
    `provider_name = NULL, status = 'legacy'` shape the CHECK allows.
- Conclusion: holds. The revision cleanly resolves the previous
  `provider_name = 'legacy:<model>:<idx>'` sentinel concern from
  shortcut F3.

### Transcript dereference from `(provider_name, session_id)`
- Severity: low
- Claim from proposal: §5-§6 — `transcript_locator` receives
  `SESSION_ID` plus `STATE_DIR` and returns an absolute path; for
  Claude, the primary session file is `<base>/<cwd-encoded>/<session_id>.jsonl`.
- What I verified:
  - `find ~/.claude2/projects/ -name "9e69e8cc*"` returned exactly one
    file:
    `/home/nes/.claude2/projects/-home-nes-projects-agent-runner/9e69e8cc-616d-4640-bf1d-96f5391b1a2e.jsonl`.
  - The filename IS the session_id, so a recursive glob
    `rglob("<session_id>.jsonl")` under the provider's base dir yields
    exactly one path. `scripts/claude-code-turns:42` already does a
    `base.rglob("*.jsonl")` walk, validating the traversal pattern.
  - Codex session files (`~/.codex*/sessions/YYYY/MM/DD/rollout-<ts>-<thread_id>.jsonl`)
    embed `thread_id` in the filename, which again makes the inverse
    mapping unambiguous once a locator script walks the date tree.
- Conclusion: holds. The locator contract is satisfiable for both
  in-scope providers. Claude sidechain files
  (`subagents/agent-*.jsonl`) are not named by session_id, but §5
  models sidechains as rows inside the primary session's `session_id`
  in `session_turns`, so no secondary file resolution is needed at
  trace time.

### Adapter-contract widening is backward-compatible
- Severity: low
- Claim from proposal: §5 — `turn_script` gets two optional fields
  (`parent_turn_id`, `is_sidechain`); existing 4-field scripts
  continue to work.
- What I verified: `src-tauri/src/sessions/mod.rs:33-39` defines
  `ScriptTurn` with `#[derive(Deserialize, ...)]` and no
  `deny_unknown_fields`. Adding `parent_turn_id: Option<String>` and
  `is_sidechain: Option<bool>` deserializes cleanly from 4-field input
  with both values `None`. Batch-insert at
  `src-tauri/src/sessions/mod.rs:109` passes a 4-tuple and will need
  to widen — mechanical, not a correctness risk.
- Conclusion: holds.

### "One stderr line per process" vs "Codex `--json` stdout" — no contradiction
- Severity: low
- Claim from proposal: §7 — exactly one `OULIPOLY_INVOCATION=...` line
  per runner process; §4 — Codex runs in `--json` mode.
- What I verified: §7 is about the runner's own stderr metadata line,
  emitted *before* the subprocess is spawned. §4 is about the wrapped
  CLI's stdout format. These are two different processes and two
  different streams. The runner additionally reconstructs plain-text
  stdout from the `--output-last-message` tmpfile before writing to
  its own stdout, so the caller-visible stdout shape is preserved.
- Conclusion: holds. Internally consistent.

### Claude fallback path was removed (revision closes a prior gap)
- Severity: low
- Claim from proposal: §4 — "There is no runner-side filesystem
  fallback. Under V10 the correct degraded behavior is `unresolved`,
  not a hidden heuristic."
- What I verified: §4 is now the explicit failure-closed policy, and
  `session_capture_method = 'unresolved'` plus `session_id = NULL` is
  the single degraded outcome. No earlier stream-json fallback exists
  that would disturb plain-text stdout.
- Conclusion: holds. A concern I would have flagged against a prior
  version no longer applies.

### Citation accuracy (spot checks)
- Severity: low
- Claim from proposal: file:line citations in the body and §13.
- What I verified:
  - `src-tauri/src/executor/cli.rs:214-290` — `execute_provider`
    actually spans 214-291; matches within one line.
  - `src-tauri/src/executor/cli.rs:239-241` — `current_dir` block;
    exact.
  - `src-tauri/src/sessions/mod.rs:32-39` — `ScriptTurn` struct;
    exact.
  - `scripts/claude-code-turns:68-82` — the 4-field emission that
    drops `parentUuid`/`isSidechain`; exact.
- Conclusion: citations are accurate within a line or two; no
  fabrications.

## Synthesis adherence check

- **Source of truth (SQLite structure, raw logs for content)**:
  honored (§2, §5, §9, §10).
- **Cross-invocation correlation via env var, not inference**: honored
  (§3). §4 explicitly removes the runner-side filesystem fallback.
- **`trace` subcommand with human + JSON output**: honored (§6).
- **Composite `{"source": ..., "id": ...}` ID on stderr**: honored
  (§3, §7, §10).
- **UUIDv4 IDs, UUID column added alongside integer PK**: honored (§2,
  §3).
- **Adapter contract evolution: optional `parent_turn_id` and
  `is_sidechain`**: honored (§5).
- **Synthesis §5 (critical unknown: session-id capture)**: honored —
  per-CLI strategy declared in `ProviderConfig`
  (`forced_flag_verified` for Claude, `stdout_json_event` for Codex).
- **§6 Q1 (top-level parent_invocation_id)**: honored — NULL, §3.
- **§6 Q2 (tree when correlation fails)**: honored — `unresolved`,
  `no_locator`, `missing`, `locator_error` states in §6.
- **§6 Q3 (user-direct CLI sessions)**: honored — §6 "Direct user CLI
  sessions with no invocation row are not traversed by `trace`."
- **§6 Q4 (transcript leaf format)**: honored — pointer by default,
  `--transcript` for rendered, `--inline-transcript` for raw JSON.
- **§6 Q5 (ship `session_turns` parentage now or later)**: honored with
  caveat — §12 splits the column additions into PR-A (schema) and the
  adapter change into PR-D. That is a flagged split, not silent, and
  it preserves the synthesis intent (schema migration is free, adapter
  change lands when ready).
- **§6 Q6 (multimodal binary wrappers)**: honored — §4 `none` strategy
  preserves raw-byte stdout; §9 excludes binary wrappers from
  transcript guarantees.
- **§6 Q7 (backfill of existing rows)**: honored with flagged
  revision — the original sentinel `provider_name =
  'legacy:<model>:<idx>'` was replaced by nullable `provider_name` +
  `status = 'legacy'`, explicitly called out as Shortcut F3 in §13.

Nothing silently dropped. The two revisions against the original
synthesis intent (F3 sentinel → nullable+status, and the addition of
`transcript_locator` to satisfy V1/V2) are both documented in §13
with value citations.

## Recommended revisions (if any)

1. Section 2 and §12 — update "currently 76 rows" to match the
   moving target (78 on 2026-04-17), or reword as "~76 rows at
   proposal time, grows over time." Editorial only.
2. Section 2 migration step 5 — "recreate indexes" is slightly
   ambiguous. Indexes created on `invocations_new` in step 2 carry
   through the rename; either clarify that step 5 is a no-op for
   those indexes or explicitly enumerate which indexes are dropped
   and recreated. Editorial.
3. Section 4 `stdout_json_event` description — mention the handling
   when the `--output-last-message` file is missing or empty (e.g.
   because Codex crashed before writing it). Presumably this folds
   into the existing `session_id = NULL, session_capture_method =
   'unresolved'` failure path, but one sentence would remove doubt.
4. Section 11 open question 2 (structured stderr capture) — the
   proposal is explicit that this doesn't block the revision; if the
   author wants to reduce future risk, reserving a named kind now
   (`stderr_json_event`) costs nothing and signals the surface is
   intentionally extensible.

None of these are correctness concerns. The proposal is ready for
the next gate on audit grounds.
