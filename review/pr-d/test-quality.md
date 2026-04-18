# PR-D Test Quality Audit

## Verdict

Pass, with two low-severity coverage gaps. The PR exercises all six required contract points, and every targeted test I ran passed. The suite is solid enough for the PR-D scope, but it is not a clean A because two important seams are only indirectly covered.

## Findings

1. Low: the old-schema migration test does not prove existing `session_turns` rows survive migration.
   `src-tauri/src/state/db.rs:1999` opens a legacy table and checks the new columns after `StateDb::open()`, but it seeds no pre-existing data. That means the "additive ALTER on existing tables" part of the contract is only partially pinned: schema shape is verified, row preservation is not.

2. Low: there is no end-to-end runner ingest test for the widened six-field turn shape.
   The new fields are tested in three separate places:
   `src-tauri/src/sessions/mod.rs:363`,
   `src-tauri/src/state/db.rs:2544`,
   `src-tauri/tests/pr_d_claude_code_turns.rs:25`.
   What is still missing is one test that drives `scan_provider()` with six-field JSON and then asserts `parent_turn_id` / `is_sidechain` were persisted. Without that, the `ScriptTurn -> SessionTurnIngest -> DB` seam is covered only by composition of smaller tests, not directly.

## Per-Dimension Grades

- Contract coverage: `B`
- Assertion quality: `A`
- Isolation and determinism: `A`
- Integration realism: `B`
- Regression resistance: `B`

## Required Coverage Walk

1. Schema additions test: covered.
   `src-tauri/src/state/db.rs:1983` checks fresh-schema creation includes `parent_turn_id TEXT` and `is_sidechain INTEGER NOT NULL DEFAULT 0`.
   `src-tauri/src/state/db.rs:1999` checks old-schema migration adds both columns with the expected nullability/defaults.

2. ScriptTurn deserialization (legacy 4-field + full 6-field): covered.
   `src-tauri/src/sessions/mod.rs:363` verifies legacy four-field JSON deserializes with `None` defaults.
   `src-tauri/src/sessions/mod.rs:377` verifies the full six-field shape populates both new fields.

3. Batch insert with widened struct: covered.
   `src-tauri/src/state/db.rs:2544` inserts a `SessionTurnIngest` carrying `parent_turn_id` and `is_sidechain`, then reads the row back from SQLite and asserts both persisted.

4. `count_session_turns` correctness: covered.
   `src-tauri/src/state/db.rs:2577` mixes roles, sidechain values, another session, and another provider, then asserts the method returns `total = 3`, `assistant = 2`, `sidechain = 1` for the requested provider/session pair.

5. Trace integration `sidechain_turn_count` populated: covered.
   `src-tauri/src/trace/mod.rs:1084` builds a trace fixture with ingested turns and asserts JSON output populates `turn_count`, `assistant_turn_count`, and `sidechain_turn_count`.

6. `claude-code-turns` emits new fields: covered.
   `src-tauri/tests/pr_d_claude_code_turns.rs:25` runs the real adapter script against a temporary Claude-style JSONL transcript and asserts the emitted child turn preserves both `parentUuid` and `isSidechain`.

## Verification

Targeted tests executed and passing:

- `cargo test script_turn_ -- --nocapture`
- `cargo test session_turns_schema_ -- --nocapture`
- `cargo test ingest_session_turns_batch_persists_parent_and_sidechain_columns -- --nocapture`
- `cargo test count_session_turns_reports_total_assistant_and_sidechain_counts -- --nocapture`
- `cargo test json_output_populates_sidechain_turn_count_from_session_turns -- --nocapture`
- `cargo test --test pr_d_claude_code_turns -- --nocapture`
