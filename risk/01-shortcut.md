# Shortcut Risk Assessment: proposals/01-trace-inspection.md

## Verdict: LOW

The revised proposal closes the two structural shortcuts the prior
review flagged (CLI-specific executor branch, runner-side transcript
layout knowledge) by elevating both surfaces to declarative config:
`session_capture` on `ProviderConfig` and `transcript_locator` in
`sessions.toml`. Historical backfill no longer encodes a sentinel in
`provider_name`; fail-closed replaces the silent filesystem fallback;
the migration is explicitly transactional; the `--session-id` path
now requires readback verification. The remaining concerns are
low-severity judgment calls with stated tradeoffs and open-question
follow-ups, not load-bearing debt.

## Findings

### 1. Declarative `session_capture` on `ProviderConfig`

- Severity: low
- What might be a shortcut: §4 adds a declarative
  `session_capture = { kind = ..., ... }` to `ProviderConfig` with
  three initial kinds (`none`, `forced_flag_verified`,
  `stdout_json_event`) and dispatches generically. Codex-specific
  output reconstruction (reading `--output-last-message` tmpfile and
  writing it back to stdout) is parameterized via
  `output_last_message_flag`.
- Why it might be one: Historically this was the highest-risk
  shortcut, because the naïve implementation would be a hardcoded
  `match provider.command.contains("codex")` branch.
- Why it is NOT: The dispatch surface is declarative, parameterized
  on the exact CLI-visible shape (`event_args`, `event_type`,
  `event_field`, `output_last_message_flag`), so a third CLI that
  fits the same pattern is added by TOML + adapter script, not
  runner code. This is the V1/V2/V3 bar the synthesis demanded.
- Verdict on this finding: **clean**

### 2. `transcript_locator` adapter contract in `sessions.toml`

- Severity: low
- What might be a shortcut: §5-§6 move per-CLI transcript layout
  knowledge into `transcript_locator` scripts invoked lazily with
  `SESSION_ID` / `STATE_DIR`, parallel to the existing `turn_script`
  contract at `src-tauri/src/sessions/mod.rs:1-19`. The runner reads
  a path, never derives one.
- Why it might be one: The prior design had the runner walk
  `~/.claude*/projects/<encoded-cwd>/` and
  `~/.codex*/sessions/YYYY/MM/DD/` itself.
- Why it is NOT: Knowledge now lives where the rest of the
  storage-format knowledge lives (adapter scripts). Trace degrades
  through explicit `unresolved | no_locator | missing |
  locator_error` states — no silent fallback path can quietly
  become primary. Matches the V1/V2 bar and removes the reverse-
  direction drift of teaching the runner CLI layouts.
- Verdict on this finding: **clean**

### 3. Legacy-row handling via `status = 'legacy'`

- Severity: low
- What might be a shortcut: §2's backfill uses
  `status = 'legacy'` and sets `provider_name = NULL` when the
  historical `(model_name, provider_index)` cannot be resolved to a
  current `provider_name`. A `CHECK (provider_name IS NOT NULL OR
  status = 'legacy')` enforces the invariant.
- Why it might be one: The earlier proposal stuffed a
  `legacy:<model>:<idx>` sentinel into `provider_name`.
- Why it is NOT: The sentinel is gone. `provider_name` stays a
  real value domain; downstream queries and indexes
  (`idx_invocations_provider_created`,
  `idx_invocations_session`) don't get polluted; the `'legacy'`
  status value in the CHECK is actively used rather than orphaned.
- Verdict on this finding: **clean**

### 4. Raw JSON in `OULIPOLY_PARENT_INVOCATION`

- Severity: low
- What might be a shortcut: §3 keeps a single env var carrying
  `{"source":"...","id":"..."}` JSON rather than two scalar vars.
- Why it might be one: Wrappers that re-export via shell (e.g.
  `echo "inv=$OULIPOLY_PARENT_INVOCATION" >>debug.log`) have to
  quote the `{`, `"`, and `:` carefully.
- Why it might NOT: The runner spawns via `Command` directly
  (`executor/cli.rs:268`), so the first hop is shell-free. The
  composite is the stable over-the-wire contract (V13); splitting
  into two vars would fragment extensibility. The proposal now
  explicitly states the quoting requirement belongs in the wrapper
  docs (§3, §8) rather than leaving it implicit.
- Verdict on this finding: **defensible** (tradeoff stated, doc
  path committed)
- Suggested fix: None structurally required. The README bullet in
  §8 ("wrapper scripts must quote `OULIPOLY_PARENT_INVOCATION` on
  re-export") is the minimal mitigation and is already in scope.

### 5. Reusing `invocation_uuid` as Claude's `--session-id` value

- Severity: low
- What might be a shortcut: §4 says the runner "may reuse
  `invocation_uuid` as the requested session UUID for convenience."
- Why it might be one: Two semantically distinct identifiers
  carrying the same value can conflate in callers' minds.
- Why it is NOT: The proposal now spells out that
  `invocation_uuid` and `session_id` remain separate columns and
  separate concepts; if Claude ever changes session-id format
  (longer UUID, namespace prefix, typed-ID shape), the runner
  generates a distinct value at capture time with no schema
  change. The collapse is only in the *assigned value*, not in the
  data model. `forced_flag_verified` also re-reads the actual
  returned id and fails closed on mismatch, so drift would be
  detected rather than silently accepted.
- Verdict on this finding: **clean**

### 6. Readback verification on `forced_flag_verified`

- Severity: low
- What might be a shortcut: §4 requires readback of
  `system.init.session_id` from the CLI's own stream-JSON output
  before trusting the forced `--session-id` value; mismatch fails
  closed to `unresolved`.
- Why it might be one: Readback adds coupling to a second CLI
  output shape.
- Why it is NOT: Without it, the "flag accepted but ignored"
  failure mode silently writes a wrong transcript pointer —
  exactly the kind of symptom-masking the gate is looking for.
  The readback is cheap (one JSON event parse), observable
  (`session_capture_method`), and the fallthrough is explicit,
  not heuristic.
- Verdict on this finding: **clean**

### 7. Removal of runner-side filesystem fallback

- Severity: low
- What might be a shortcut: Earlier drafts layered a
  cwd+timestamp filesystem probe as the final fallback for Claude
  and Codex capture.
- Why the current design is NOT: §4 deletes the fallback outright
  ("There is no runner-side filesystem fallback. That is an
  intentional revision."). The degraded state is `unresolved`,
  which is persisted, queryable in `trace`, and attributable via
  `session_capture_method`. This is the right read of V10 —
  degradation must be observable, not papered over.
- Verdict on this finding: **clean**

### 8. `invocations` table rebuild migration

- Severity: low
- What might be a shortcut: §2's create-copy-drop-rename dance for
  column changes that can't be expressed as `ALTER TABLE`.
- Why it is NOT: This is the canonical SQLite pattern. The
  proposal now explicitly commits to running the whole migration
  inside a single transaction (§2 step 1 / step 5 commit), calls
  out FK enforcement semantics during rename ("foreign-key
  enforcement stays on throughout; the self-reference is valid
  because migrated rows start with `parent_invocation_id =
  NULL`"), and acknowledges the current row count (76, growing).
- Verdict on this finding: **clean**

### 9. `is_sidechain INTEGER` boolean-as-int

- Severity: low
- What might be a shortcut: `is_sidechain INTEGER NOT NULL
  DEFAULT 0`.
- Why it is NOT: Matches the existing project norm.
  `invocations.success INTEGER` (`state/db.rs:237`) and the new
  `success INTEGER` in the rebuilt table both use int-as-bool.
  Deviating would be the surprise.
- Verdict on this finding: **clean**

### 10. Dependency on Codex `thread.started` JSON shape

- Severity: low
- What might be a shortcut: `stdout_json_event` parses
  `thread.started.thread_id` from Codex's `--json` stream.
- Why it is NOT: The dependency is declared (not inferred),
  configurable per-provider (`event_type`, `event_field`), and
  the proposal calls the stability question out as an explicit
  open question (§11) with a suggested remediation (fixture-based
  contract test that fails loudly on upstream drift). When the
  only alternative is timestamp/cwd guessing, depending on
  documented machine-readable output is the correct choice.
- Verdict on this finding: **defensible**
- Suggested fix: Adopt the §11 suggestion — ship a PR-C fixture
  test pinned to the current `thread.started` shape so a silent
  upstream schema change becomes a loud CI failure instead of a
  `session_capture_method = 'unresolved'` uptick.

### 11. `--inline-transcript` vs. "no transcripts in SQLite" anti-scope

- Severity: low
- What might be a shortcut: §6's `--json --inline-transcript`
  embeds raw transcript records in JSON output while §9 forbids
  transcript content in SQLite.
- Why it is NOT: Different layers. Anti-scope governs
  **persistence**. `--inline-transcript` is **query-time
  assembly** — `trace` reads raw files at request time via the
  configured locator and prints them. Nothing is written back to
  SQLite (§6 end: "Nothing is persisted back into SQLite.").
- Verdict on this finding: **clean**

## Patterns followed correctly

- **Env-var propagation over timestamp/cwd inference** —
  reliable mechanism picked, inference not used even as a
  fallback.
- **Declarative capture via `session_capture` on
  `ProviderConfig`** — the key correction from the prior round;
  removes CLI-identity sniffing as a shortcut path.
- **Declarative layout via `transcript_locator` in
  `sessions.toml`** — keeps the runner ignorant of per-CLI
  storage, matching the existing `turn_script` pattern.
- **Explicit degraded states** (`unresolved`, `no_locator`,
  `missing`, `locator_error`) instead of silent empties.
- **Transactional migration** with FK semantics spelled out.
- **`status = 'legacy'` + nullable `provider_name`** with a
  CHECK constraint — no sentinels leaking into a value-domain
  column.
- **Composite `{source, id}` ID with raw UUID as the CLI input**
  — extensible over the wire, minimal at the CLI.
- **Stderr-only metadata line** — stdout stays reserved for
  model output, preserving V9 for binary wrappers.
- **Optional adapter-contract fields** (`parent_turn_id`,
  `is_sidechain`) — existing third-party scripts keep working.
- **Readback verification on forced session-id** — closes the
  "flag accepted but ignored" silent-correctness hole.
- **Raw logs as transcript source of truth, SQLite as tree
  index** — no duplication to keep in sync.

## Recommended revisions (if any)

None blocking. Two low-cost follow-ups worth noting but not
required for this gate:

1. **PR-C fixture test** for Codex `thread.started` (the §11
   open question). Pins the shape so upstream drift surfaces as
   a test failure, not a runtime capture gap.
2. **One-liner in §3** (or the README bullet it already points
   at) stating that wrapper scripts that re-export
   `OULIPOLY_PARENT_INVOCATION` through a shell must quote it
   (`"$OULIPOLY_PARENT_INVOCATION"`). The proposal already
   commits to documenting this; lock the wording so the
   docs-PR author doesn't have to rediscover the hazard.
