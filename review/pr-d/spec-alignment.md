# PR-D Spec Alignment Audit

Verdict: `PASS`

No spec-alignment findings on the requested PR-D gate checks. The branch matches `tmp/01-pr-d-contract.md`, the optional-field contract in `proposals/01-trace-inspection.md` §5, and the PR-D slice in §12, with one note under "Open question" about proposal wording around trace-side branch surfacing.

## Findings

No findings.

## Check Results

1. `session_turns.parent_turn_id` and `is_sidechain` are additive, with no table rebuild on existing DBs.
   Evidence: fresh schema includes both columns in [`src-tauri/src/state/db.rs:448`](../../src-tauri/src/state/db.rs#L448) through [`src-tauri/src/state/db.rs:459`](../../src-tauri/src/state/db.rs#L459), and legacy upgrade uses only `ALTER TABLE ... ADD COLUMN` in [`src-tauri/src/state/db.rs:522`](../../src-tauri/src/state/db.rs#L522) through [`src-tauri/src/state/db.rs:539`](../../src-tauri/src/state/db.rs#L539). There is no `session_turns_new`, copy, drop, or rename path in the diff. Coverage exists in [`src-tauri/src/state/db.rs:1984`](../../src-tauri/src/state/db.rs#L1984) through [`src-tauri/src/state/db.rs:2026`](../../src-tauri/src/state/db.rs#L2026).

2. `ScriptTurn` is widened compatibly for legacy 4-field adapters.
   Evidence: optional fields are declared with `#[serde(default)]` in [`src-tauri/src/sessions/mod.rs:34`](../../src-tauri/src/sessions/mod.rs#L34) through [`src-tauri/src/sessions/mod.rs:42`](../../src-tauri/src/sessions/mod.rs#L42), matching the optional-field rule in proposal §5 (`proposals/01-trace-inspection.md:150-167`). Back-compat tests live at [`src-tauri/src/sessions/mod.rs:364`](../../src-tauri/src/sessions/mod.rs#L364) through [`src-tauri/src/sessions/mod.rs:387`](../../src-tauri/src/sessions/mod.rs#L387).

3. `SessionTurnIngest` carries the widened ingest payload.
   Evidence: the new ingest struct includes `parent_turn_id` and `is_sidechain` in [`src-tauri/src/state/db.rs:77`](../../src-tauri/src/state/db.rs#L77) through [`src-tauri/src/state/db.rs:95`](../../src-tauri/src/state/db.rs#L95), and the session scan path populates it in [`src-tauri/src/sessions/mod.rs:88`](../../src-tauri/src/sessions/mod.rs#L88) through [`src-tauri/src/sessions/mod.rs:123`](../../src-tauri/src/sessions/mod.rs#L123).

4. `count_session_turns` returns `(total, assistant, sidechain)`.
   Evidence: `SessionTurnCounts` is defined in [`src-tauri/src/state/db.rs:90`](../../src-tauri/src/state/db.rs#L90) through [`src-tauri/src/state/db.rs:94`](../../src-tauri/src/state/db.rs#L94), and `count_session_turns` executes the contracted three-count query in [`src-tauri/src/state/db.rs:1736`](../../src-tauri/src/state/db.rs#L1736) through [`src-tauri/src/state/db.rs:1759`](../../src-tauri/src/state/db.rs#L1759). Count coverage exists in [`src-tauri/src/state/db.rs:2578`](../../src-tauri/src/state/db.rs#L2578) through [`src-tauri/src/state/db.rs:2638`](../../src-tauri/src/state/db.rs#L2638).

5. Trace integration populates `sidechain_turn_count` from `count_session_turns`.
   Evidence: `TraceSession` already exposes `sidechain_turn_count` in [`src-tauri/src/trace/mod.rs:60`](../../src-tauri/src/trace/mod.rs#L60) through [`src-tauri/src/trace/mod.rs:67`](../../src-tauri/src/trace/mod.rs#L67). `build_trace_session()` now calls `db.count_session_turns()` and maps `total`, `assistant`, and `sidechain` into the JSON session payload in [`src-tauri/src/trace/mod.rs:220`](../../src-tauri/src/trace/mod.rs#L220) through [`src-tauri/src/trace/mod.rs:344`](../../src-tauri/src/trace/mod.rs#L344). The JSON assertion is covered in [`src-tauri/src/trace/mod.rs:1085`](../../src-tauri/src/trace/mod.rs#L1085) through [`src-tauri/src/trace/mod.rs:1144`](../../src-tauri/src/trace/mod.rs#L1144).

6. `claude-code-turns` emits `parentUuid` -> `parent_turn_id` and `isSidechain` -> `is_sidechain`.
   Evidence: passthrough happens in [`scripts/claude-code-turns:76`](../../scripts/claude-code-turns#L76) through [`scripts/claude-code-turns:83`](../../scripts/claude-code-turns#L83). Script-level verification exists in [`src-tauri/tests/pr_d_claude_code_turns.rs:25`](../../src-tauri/tests/pr_d_claude_code_turns.rs#L25) through [`src-tauri/tests/pr_d_claude_code_turns.rs:63`](../../src-tauri/tests/pr_d_claude_code_turns.rs#L63).

7. `codex-turns` is unchanged.
   Evidence: `git diff --name-only main..HEAD` touches only:
   - `scripts/claude-code-turns`
   - `src-tauri/src/sessions/mod.rs`
   - `src-tauri/src/state/db.rs`
   - `src-tauri/src/state/mod.rs`
   - `src-tauri/src/trace/mod.rs`
   - `src-tauri/tests/pr_d_claude_code_turns.rs`

8. Anti-scope is respected.
   Evidence: the same changed-file list excludes `scripts/codex-turns`, excludes any `README*` file, and excludes Cargo manifest / lockfile changes. The implementation stays inside the expected PR-D touch surface from `tmp/01-pr-d-contract.md:120-133`.

9. PR-A/B/C regression tests pass.
   Evidence: `cargo test` in `src-tauri/` passed cleanly:
   - lib tests: `178 passed`
   - `src/main.rs`: `4 passed`
   - `tests/pr_a_invocation_integration.rs`: `3 passed`
   - `tests/pr_b_trace_integration.rs`: `8 passed`
   - `tests/pr_c_locator_scripts.rs`: `4 passed`
   - `tests/pr_d_claude_code_turns.rs`: `1 passed`

## Values Alignment

- `V1` / `V2`: provider-specific sidechain knowledge stays in the Claude adapter script; the runner only consumes the generic turn-script contract.
- `V4`: contract growth is additive and optional; missing adapter fields still deserialize and default correctly.
- `V10`: trace count failures are surfaced as warnings instead of being silently coerced.

## Open Question

`tmp/01-pr-d-contract.md` scopes trace work to populating `session.sidechain_turn_count`, and this branch satisfies that. `proposals/01-trace-inspection.md` §12 uses broader wording, saying PR-D should "surface sidechain counts/branches in `trace --json`." I did not find any new trace JSON field that exposes per-turn branch structure directly; only the persisted metadata and aggregate counts are surfaced. I am treating that as acceptable for this gate because the PR-D contract and the explicit review checks both narrow the requirement to count population, but the wording difference is worth keeping in mind for follow-up review.
