# Spec Alignment: feat/pr-a-invocation-lifecycle

## Verdict: PASS

The diff implements the PR-A contract faithfully: the lifecycle, schema, env-var propagation, stderr contract, and anti-scope boundaries all match, and the only non-contract file outside the main hookpoints is `src-tauri/src/balancer/mod.rs`, where the changes are test-only migration off the removed `record_invocation` API.

## Per-requirement verification

| # | Requirement | Status | Notes |
|---|---|---|---|
| 1 | Schema matches contract DDL | PASS | Fresh-schema SQL in `src-tauri/src/state/db.rs:484-505` matches the contract shape: `invocation_uuid`, nullable `provider_name`, `parent_invocation_id`, `status` enum check, nullable terminal fields, and the three required indexes. Migration rebuild SQL in `src-tauri/src/state/db.rs:559-573` uses the same column set and constraints. |
| 2 | Migration transactional | PASS | Legacy rebuild is wrapped in `unchecked_transaction()` at `src-tauri/src/state/db.rs:528-531`, with create/copy/drop/rename/index recreation all committed together at `631-632`. The rollback test at `1769-1817` confirms failure does not partially rewrite `invocations`. |
| 3 | `InvocationStart` struct fields match contract | PASS | `InvocationStart` is `pub`, `Debug`, `Clone`, and exposes exactly the five contracted public fields at `src-tauri/src/state/db.rs:92-99`. |
| 4 | `InvocationRecord` struct fields match contract | PASS | `InvocationRecord` is widened exactly as required at `src-tauri/src/state/db.rs:75-90`, including `id`, `invocation_uuid`, nullable `provider_name`, nullable terminal fields, and `finished_at: Option<DateTime<Utc>>`. |
| 5 | `InvocationStatus` has all four variants | PASS | `Running`, `Succeeded`, `Failed`, and `Legacy` are defined at `src-tauri/src/state/db.rs:101-107`; `as_str()` and the contracted inherent `from_str()` are present at `109-129`. |
| 6 | `CompositeInvocationId` contracts | PASS | The type is declared with the exact two fields at `146-150`. `stderr_line()` emits the required `OULIPOLY_INVOCATION=...` line without trailing newline at `153-160`. `parse_env_value()` rejects malformed JSON and invalid UUIDs at `162-167`, and the proposal’s strict-shape requirement is enforced via `#[serde(deny_unknown_fields)]` at `146-147`. |
| 7 | `StateDb::start_invocation` signature and behavior | PASS | Signature matches at `src-tauri/src/state/db.rs:665`. Insert behavior at `667-693` writes `status='running'`, leaves `success`, `exit_code`, `error_category`, and `finished_at` NULL, and returns `last_insert_rowid()`. |
| 8 | `StateDb::finalize_invocation` signature and aggregate behavior | PASS | Signature matches at `src-tauri/src/state/db.rs:696-703`. The method updates the row to terminal state at `730-751`, rejects missing/already-finalized rows at `710-728`, and preserves old aggregate provider-stat behavior at `753-785` by incrementing `invocation_count`, `error_count`, `last_invoked_at`, and `last_error` on failure. |
| 9 | `StateDb::get_invocation_by_uuid` signature matches | PASS | Exact signature and lookup behavior appear at `src-tauri/src/state/db.rs:788-805`. Returned records are mapped through the widened `InvocationRecord` shape at `808-849`. |
| 10 | `record_invocation` removed cleanly | PASS | The production method is deleted. `git show main:src-tauri/src/state/db.rs` shows the old API existed; `rg -n "record_invocation" -S src-tauri` now finds only the new `record_invocation_for_test` helper in balancer tests. No compatibility shim or deprecated alias remains. |
| 11 | `run_with_balancing` lifecycle reorder | PASS | Ordering matches the contract in `src-tauri/src/main.rs:256-305`: parent env is resolved before provider selection (`259-260`), `start_invocation` runs before stderr emission (`266-275`), the wrapped command is spawned only after the stderr line (`277-284`), and finalization happens only after execution returns (`286-312`). |
| 12 | Stderr line format exactly matches | PASS | `CompositeInvocationId::stderr_line()` formats `OULIPOLY_INVOCATION={"source":"<provider>","id":"<uuid>"}` exactly at `src-tauri/src/state/db.rs:153-160`; `eprintln!` in `src-tauri/src/main.rs:275` provides the mandatory trailing newline. Integration test `src-tauri/tests/pr_a_invocation_integration.rs:83-97` asserts exactly one such line per invocation. |
| 13 | Parent env var read at startup with silent root fallback | PASS | `resolve_parent_invocation_id()` at `src-tauri/src/main.rs:332-340` reads `OULIPOLY_PARENT_INVOCATION`, parses it, resolves the UUID, and silently falls back to `None` for malformed JSON, invalid UUID, DB lookup failure, or provider-source mismatch. The integration test at `src-tauri/tests/pr_a_invocation_integration.rs:147-182` covers malformed, invalid-UUID, and unresolved inputs. |
| 14 | Parent env var written to spawned `Command` | PASS | The spawn path adds `OULIPOLY_PARENT_INVOCATION` to the child `Command` in `src-tauri/src/executor/cli.rs:242-247`. `run_with_balancing()` serializes the current composite ID and passes it down at `src-tauri/src/main.rs:273-284`. |
| 15 | Anti-scope respected | PASS | No PR-B/C/D surfaces appear in the diff. Searches across touched files show no `trace` subcommand, `session_capture`, `transcript_locator`, `session_turns.parent_turn_id`, `is_sidechain`, or CLI subcommand refactor. The CLI remains flat in `src-tauri/src/main.rs`. |
| 16 | No new Cargo dependencies | PASS | `git diff --name-only main..HEAD -- src-tauri/Cargo.toml` returned no changes. The implementation reuses existing `uuid`, `serde`, and `serde_json` dependencies. |
| 17 | Files touched match hookpoint research | PASS | Changed files are `state/db.rs`, `state/mod.rs`, `main.rs`, `executor/cli.rs`, `executor/mod.rs`, the new integration test, and `balancer/mod.rs`. The balancer diff is entirely inside `#[cfg(test)] mod tests` (`src-tauri/src/balancer/mod.rs:223-431`), so the only non-hookpoint runtime surfaces are unchanged. |

## Findings (severity ≥ medium)

None.

## Out-of-scope additions (if any)

- None.

## Required-but-missing items (if any)

- None.
