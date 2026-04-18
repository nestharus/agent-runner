# PR-D Coverage Delta Audit

Verdict: `PARTIAL`

PR-D’s new session-turn surfaces are mostly covered. The added tests directly exercise the schema expansion, `ScriptTurn` backward-compatible deserialization, the adapter’s new `parentUuid` / `isSidechain` fields, and both the DB-level and trace-level happy paths for `turn_count`, `assistant_turn_count`, and `sidechain_turn_count`. The remaining gap is the graceful-degradation branch in [`build_trace_session()`](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:220): the code now swallows `count_session_turns()` failures, emits a warning, and leaves counts as `None`, but no test forces that path. That matters under `V10 — Failures are observable, never silent`.

## Findings

1. Medium: the `count_session_turns()` graceful-degradation path in trace is untested. [`build_trace_session()`](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:264) explicitly catches DB count failures, pushes a warning, and falls back to `None` for all three counts, but the suite only covers the unresolved-no-session case in [`json_session_fields_are_null_or_unresolved_in_pr_b()`](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:842) and the positive count-population path in [`json_output_populates_sidechain_turn_count_from_session_turns()`](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:1085). I do not see any test that makes [`StateDb::count_session_turns()`](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:1736) return `Err(...)` and then asserts both the warning text and `null` JSON counts.

## Checkpoints

1. `count_session_turns` covered for all three count types?
Status: Covered.
Evidence: [`count_session_turns_reports_total_assistant_and_sidechain_counts()`](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2578) asserts `total = 3`, `assistant = 2`, and `sidechain = 1`, while isolating against another session and another provider. The trace-level projection of those same three fields is then asserted in [`json_output_populates_sidechain_turn_count_from_session_turns()`](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:1085).

2. Trace `count_session_turns` graceful-degradation path?
Status: Missing.
Evidence: the fallback branch is implemented in [`build_trace_session()`](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:264), but there is no test that forces the warning `"failed to count session turns..."` from [`trace/mod.rs`](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:271) and confirms `turn_count`, `assistant_turn_count`, and `sidechain_turn_count` become `null`.

3. Schema migration on legacy `session_turns` (no `parent` column yet) exercised?
Status: Covered.
Evidence: [`session_turns_schema_migration_adds_parent_and_sidechain_columns()`](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2000) constructs a legacy `session_turns` table without the new columns, opens the DB through migration, and asserts that `parent_turn_id` and `is_sidechain` were added with the expected nullability/defaults.

4. `ScriptTurn` deserialization, legacy + full?
Status: Covered.
Evidence: [`script_turn_legacy_json_deserializes_with_none_defaults()`](/home/nes/projects/agent-runner/src-tauri/src/sessions/mod.rs:364) verifies missing fields deserialize to `None`, and [`script_turn_full_json_deserializes_parent_and_sidechain_fields()`](/home/nes/projects/agent-runner/src-tauri/src/sessions/mod.rs:378) verifies the richer shape parses both new fields.

5. `claude-code-turns` adapter sidechain rows emit `parentUuid` + `isSidechain`?
Status: Covered.
Evidence: the adapter now forwards those fields in [`scripts/claude-code-turns`](/home/nes/projects/agent-runner/scripts/claude-code-turns:76), and the integration test [`claude_code_turns_emits_parent_uuid_and_is_sidechain_fields()`](/home/nes/projects/agent-runner/src-tauri/tests/pr_d_claude_code_turns.rs:26) writes a Claude-style JSONL transcript, runs the real script, and asserts the emitted child row preserves both values.

## Verification

Executed targeted checks:

- `cargo test --manifest-path src-tauri/Cargo.toml count_session_turns_reports_total_assistant_and_sidechain_counts -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml session_turns_schema_migration_adds_parent_and_sidechain_columns -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml script_turn_ -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml json_output_populates_sidechain_turn_count_from_session_turns -- --nocapture`
- `cargo test --manifest-path src-tauri/Cargo.toml --test pr_d_claude_code_turns -- --nocapture`
