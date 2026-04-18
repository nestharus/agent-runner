# Scope Risk Assessment: proposals/01-trace-inspection.md

## Verdict: LOW

The proposal stays inside the synthesis's in-scope envelope, self-decomposes into a 4-PR sequence (§12) with clean dependency order and independent user value at each merge, and §9's anti-scope is consistent with what §2-§7 actually build; nothing reviewed here would be improved by reshaping the scope further.

## Findings

### F1. The 4-PR decomposition in §12 is concrete, ordered, and each PR is independently shippable
- Severity: low
- What's addressed: §12 splits the work into PR-A (schema + UUID + lifecycle + stderr ID + env-var propagation + migration), PR-B (`trace` subcommand), PR-C (declarative `session_capture` + `transcript_locator` + Codex `--json` rewrite), and PR-D (within-session sidechain capture + `claude-code-turns` update). Dependencies: A → B, A → C, A → D; B/C/D are mutually independent.
- Why it matters: This matches the decomposition shape the risk gate would otherwise demand. Each PR ships user value on merge (per §12: callers capture IDs after A; humans get a tree after B; sessions resolve after C; sidechains appear after D). No PR is a bare refactor whose value depends on a later merge.
- Suggestion: None. This is the decomposition the gate would recommend.

### F2. The synthesis's "open" question on adapter bundling is resolved by splitting, not bundling
- Severity: low
- What's addressed: Synthesis §6 left it open whether the `claude-code-turns` update ships with the schema change or splits. The current proposal commits to splitting (PR-D owns the adapter update and the `session_turns.parent_turn_id` / `is_sidechain` columns together). §13 records this as a value-aligned correction against a prior bundled stance.
- Why it matters: The schema-add without adapter update would be inert on its own, but the proposal now scopes *both* the column add and the adapter change into the same downstream PR (D), which keeps them coherent while still isolating them from PR-A's lifecycle/migration concerns. That's the right bundling granularity.
- Suggestion: None.

### F3. The Codex `--json` executor rewrite is appropriately scoped into PR-C
- Severity: low
- What's addressed: §4 forces Codex into `--json` event mode with `--output-last-message` plain-text reconstruction. §12 locates that work in PR-C alongside the declarative `session_capture` dispatch, which is where it belongs: the rewrite is the Codex instantiation of the `stdout_json_event` capture strategy, not a standalone executor change.
- Why it matters: An earlier assessment would have flagged this as a bundled executor rewrite. In the current shape it is the *mechanism* of the `stdout_json_event` strategy, so separating it from `session_capture` would leave PR-C without a working Codex path. Bundling is correct here.
- Suggestion: None.

### F4. `provider_name` on `invocations` and `session_capture_method` are synthesis-derived, not creep
- Severity: low
- What's addressed: `provider_name` is required by the composite ID contract (synthesis §4: `{"source":"<provider>","id":"<uuid>"}`). `session_capture_method` is required by V10 observability of degraded capture (explicit `unresolved` / `no_locator` / `missing` / `locator_error` states). `status='running'` is required to emit the ID before the wrapped CLI exits.
- Why it matters: Each of these looks like an extension beyond the four synthesis-blocking gaps, but each is a direct consequence of a synthesis-approved behavior. Reviewers should recognize these as synthesis-derived, not speculative extras.
- Suggestion: None; §2 already foregrounds the lifecycle change ("This changes the invocation write pattern from `insert on completion` to `insert on spawn, update on finish`"), which was the main reviewer-visibility concern.

### F5. Anti-scope (§9) holds across §2-§7
- Severity: low
- What's audited: I checked every schema column, JSON field, CLI flag, and adapter contract extension against §9 for leakage toward descoped features ("all calls today" by account, compliance/customer scoping, stderr retention, streaming, TUI).
  - `idx_invocations_provider_created` on `(provider_name, created_at)` is the closest call — it could accelerate a future "all calls today" query. But it is also justified by provider-scoped tree walks and by the common pattern of debugging recent activity for one account. The index is cheap, and it does not imply a subcommand.
  - `idx_invocations_session` on `(provider_name, session_id)` is justified by the session → invocation reverse lookup the trace flow needs, not by any account-wide query.
  - `sidechain` / `parent_turn_id` are V4 optional fields, backward-compat. Not creep.
  - The composite ID, `transcript_state` enum, and `session_capture_method` enum are all in-scope observability fields, not compliance infra.
- Why it matters: The classic creep smell (shaping today's schema to preserve optionality for a deferred feature) is absent. Discipline is good.
- Suggestion: None. If the team wants to be strict, `idx_invocations_provider_created` could be dropped until a scoped query subcommand actually lands, but it is cheap and defensible today.

### F6. The `transcript_locator` adapter introduction is in-scope by synthesis constraint
- Severity: low
- What's addressed: §5 adds a `transcript_locator` adapter contract to `sessions.toml`, on top of widening `turn_script`. This is an additional adapter surface beyond what synthesis §4 explicitly named (`turn_script` with `parent_turn_id` / `is_sidechain`).
- Why it matters: Synthesis §4 committed the runner to *not* knowing provider storage layouts (V1, V2: "user-extensible adapters"; "runner explicitly does not know whether a CLI stores history as JSONL, SQLite, or remote API"). Given that constraint, transcript dereference *must* live behind an adapter. A path column on `invocations` would have been the "cheap" alternative, and §5 explicitly acknowledges that trade — the proposal correctly chooses the V1/V2-aligned path over the V8-cheaper path. This is not scope creep; it is the design the synthesis tradeoffs force.
- Suggestion: None.

### F7. Estimated ~1500 lines is the four-PR total, not one-PR size
- Severity: low
- What's addressed: §12 breaks line estimates down per PR: A (~450-550 + tests), B (~300-420 + tests), C (~320-430 + scripts/tests), D (~120-180 + scripts/tests). Each PR is individually reviewable at 350-600 Rust lines plus localized test/doc work.
- Why it matters: No PR is above the "one human can review this in one sitting" bar. The seven test categories called out in earlier revisions now distribute across PRs (migration + lifecycle tests to A; trace tree/JSON tests to B; capture fixtures to C; sidechain ingest tests to D).
- Suggestion: None.

## Decomposition assessment

The proposal already ships with a concrete decomposition in §12. Restated with dependencies:

- **PR-A — Durable invocation IDs + lifecycle.** Schema migration (rebuild `invocations`, add `invocation_uuid`, `provider_name`, `parent_invocation_id`, `status`, `finished_at`, migrate 76 legacy rows as `status='legacy'` when provider cannot be resolved), insert-on-spawn + update-on-finish lifecycle, stderr `OULIPOLY_INVOCATION=` emission, `OULIPOLY_PARENT_INVOCATION` env-var parse/validate/propagate. ~500-600 lines. Dependencies: none. Value on merge: callers capture a durable ID immediately, and cross-invocation parent edges populate for any runner-spawned children even before `trace` exists.
- **PR-B — `trace` subcommand.** Tree walk over SQLite, ASCII + `--json` output, explicit `unresolved` / `no_locator` / `missing` / `locator_error` transcript states, cycle guard, optional `--transcript` / `--inline-transcript` plumbing. ~350-500 lines. Dependencies: PR-A. Value on merge: users can inspect the invocation tree; `session` column reports `unresolved` until PR-C lands, which is a documented graceful degradation.
- **PR-C — Session correlation.** Declarative `session_capture` on `ProviderConfig` with three initial kinds (`none`, `forced_flag_verified`, `stdout_json_event`), Claude readback verification, Codex `--json` event capture + `--output-last-message` plain-text reconstruction, `transcript_locator` adapter contract, reference `claude-code-locate-transcript` and `codex-locate-transcript` scripts. ~400-550 lines. Dependencies: PR-A. Value on merge: `trace` gains real `session_id` values and transcript pointers.
- **PR-D — Within-session sidechain capture.** `session_turns.parent_turn_id` + `is_sidechain` columns, widened `turn_script` contract (optional fields; backward-compat preserved), `claude-code-turns` update, optional sidechain counts in `trace --json`. ~150-250 lines + script change. Dependencies: PR-A. Value on merge: within-session Claude Task/subagent branching becomes visible instead of flattened.

Each PR is independently reviewable, independently testable, and independently shippable. The user-visible contract is stable at every merge point. The split the risk gate would have asked for is the split the proposal already commits to.

## Recommended revisions (if any)

None that change the scope shape. The proposal's §12 decomposition, §9 anti-scope, and §13 revision log collectively make the scope story explicit and defensible.

Optional nits the author can take or leave:

1. **§5 index audit.** `idx_invocations_provider_created` on `(provider_name, created_at)` is not strictly needed by the trace walk (which enters via `invocation_uuid` and recurses via `parent_invocation_id`). Consider whether to defer it until a scoped-query feature actually lands, or leave it as cheap infrastructure that does no harm. Either choice is defensible.
2. **§12 PR-D phrasing.** PR-D is described as depending only on PR-A, but the "surface sidechain counts/branches in `trace --json`" bullet technically requires PR-B to exist. If PR-B has not merged when PR-D lands, that bullet becomes a no-op. Worth a one-line note: "sidechain surfacing in `trace --json` is contingent on PR-B having landed; PR-D otherwise ships columns + adapter only."
