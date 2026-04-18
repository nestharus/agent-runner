# PR-B Spec Alignment Audit

Verdict: PASS

The branch `feat/pr-b-trace-subcommand` aligns with `tmp/01-pr-b-contract.md`, `proposals/01-trace-inspection.md` §6 and §12 PR-B, and `VALUES.md` for the requested audit scope. The diff against `feat/pr-a-invocation-lifecycle` stays within the expected hookpoints, the trace behavior matches the contract, and the PR-A no-subcommand regression path still passes.

| Requirement | Status | Evidence | Notes |
| --- | --- | --- | --- |
| 1. CLI subcommand structure: `agents trace <uuid>` works and no-subcommand preserves run flow | PASS | `src-tauri/src/main.rs:17-25` adds `command: Option<Subcommands>` while preserving the existing run fields; `src-tauri/src/main.rs:183-202` dispatches `Trace` and falls back to the prior run flow on `None`. Unit coverage exists in `src-tauri/src/main.rs:469-568`. Integration coverage exists in `src-tauri/tests/pr_b_trace_integration.rs:121-209`. | `cargo test --test pr_b_trace_integration` passed, including `default_cli_flow_still_runs_without_subcommand`. |
| 2. `--json`, `--inline-transcript`, `--transcript`, `--max-depth` semantics | PASS | `src-tauri/src/main.rs:63-88` defines the contracted flags. `--inline-transcript` has `requires = "json"`, `--transcript` is human-mode only via `conflicts_with = "json"`, and `--max-depth` defaults to `64`. `TraceOptions` is passed through at `src-tauri/src/main.rs:186-199`. | Unit tests cover `--inline-transcript` rejection without `--json`, `--json --transcript` rejection, and max-depth parsing. Integration tests cover `--json`, `--json --inline-transcript`, and `--max-depth 10`. |
| 3. `StateDb::list_invocation_children` signature and ordering by `(created_at, id)` | PASS | `src-tauri/src/state/db.rs:808-830` adds `pub fn list_invocation_children(&self, parent_id: i64) -> Result<Vec<InvocationRecord>, String>` and orders with `ORDER BY created_at, id`. | `cargo test list_invocation_children` passed all three DB tests. |
| 4. Tree walk: DFS preorder, cycle protection via `HashSet`, depth limit | PASS | `src-tauri/src/trace/mod.rs:94-115` loads the root and initializes `HashSet<i64>` tracking. `src-tauri/src/trace/mod.rs:131-203` walks children recursively in query order, emits depth-limit warnings/leaves at `168-177`, and emits cycle warnings/leaves at `180-187`. | The recursion is preorder: node emitted before descendants in `render_ascii_node`, with child expansion driven by the recursive builder. Unit tests cover three-level nesting, cycle detection, and depth limits. |
| 5. ASCII output matches contract format string | PASS | `src-tauri/src/trace/mod.rs:206-245` renders one line per node as `<uuid>  <provider>  <model>  <status>  <started_at>  session=<session_or_dash>  <transcript_state>`, with indented child prefixes and explicit cycle/depth leaves. The transcript footer placeholder is rendered at `117-128`. | Unit tests in `src-tauri/src/trace/mod.rs:534-618` verify the single-node format, indentation, legacy provider dash rendering, and transcript footer. |
| 6. JSON output shape matches contract, with `session.*` unresolved/null in PR-B | PASS | `src-tauri/src/trace/mod.rs:21-66` defines `TraceReport`, `TraceNode`, `TraceInvocation`, and `TraceSession` with the contracted root/invocation/session/children shape. `src-tauri/src/trace/mod.rs:139-166` populates `session.id`, `capture_method`, `transcript_path`, `turn_count`, `assistant_turn_count`, and `sidechain_turn_count` as `None`, and `transcript_state` as `Unresolved`. `parent_id` is populated from the parent invocation UUID at `193`. | Unit tests in `src-tauri/src/trace/mod.rs:621-718` verify top-level shape, nested children, running-row null terminal fields, unresolved/null session fields, and `transcript: null` when `--inline-transcript` is enabled. |
| 7. Anti-scope: no `session_capture`, no `transcript_locator`, no PR-D sidechain columns, no `executor/cli.rs` changes | PASS | `git diff --name-only feat/pr-a-invocation-lifecycle..HEAD` shows only `.gitignore`, `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`, `src-tauri/src/state/db.rs`, `src-tauri/src/trace/mod.rs`, and `src-tauri/tests/pr_b_trace_integration.rs`. `git diff --name-only feat/pr-a-invocation-lifecycle..HEAD -- src-tauri/src/executor/cli.rs` is empty. | The only sidechain-related addition is the JSON placeholder field `session.sidechain_turn_count`, which the contract explicitly expects to remain `null` in PR-B. No DB columns or provider/session adapter contracts were added. |
| 8. PR-A regression: `pr_a_invocation_integration` still passes after CLI refactor | PASS | `cargo test --test pr_a_invocation_integration` passed all three tests. | This directly validates that the no-subcommand run flow and parent invocation behavior still hold after the subcommand refactor. |
| 9. Files touched match hookpoint research + contract | PASS | The diff touches exactly `.gitignore`, `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`, `src-tauri/src/state/db.rs`, `src-tauri/src/trace/mod.rs`, and `src-tauri/tests/pr_b_trace_integration.rs`. | This matches the expected hookpoints from the contract, and the `.gitignore` addition for `.tmp/` is acceptable incidental scope. |

## Findings

No spec-alignment findings.

## Validation Run

- `cargo test --test pr_b_trace_integration`
- `cargo test --test pr_a_invocation_integration`
- `cargo test trace::`
- `cargo test list_invocation_children`
- `cargo test trace_subcommand_rejects_inline_transcript_without_json`
- `cargo test no_subcommand_still_parses_existing_model_flow`
- `cargo test trace_subcommand_rejects_transcript_with_json`
