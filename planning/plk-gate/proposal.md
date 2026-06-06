# plk Step-6a Proposal

The live bug was broken parent linkage when a nested invocation inherited `OULIPOLY_PARENT_INVOCATION` but the composite source name no longer matched the provider name stored on the parent row. The fix narrows parent resolution to the durable invariant that matters for this workspace: the inherited UUID must resolve to an invocation row in the same StateDb. Malformed env values and unknown UUIDs still resolve to no parent, so bad input records a root invocation instead of panicking or linking across databases.

The related trace cleanup is conservative. Trace now reconciles old `running` rows only when a PID identity sidecar exists and the sidecar evidence conclusively proves that every recorded process identity for the invocation is dead. Missing sidecar rows, live matching identities, or unknown process reads leave the row untouched and keep the existing JSON-only stale-running lift behavior.

No `state.db` schema change is introduced. Parent linkage uses existing invocation UUID and parent row fields, while liveness evidence remains in `pid-identity.db` sidecar storage.

## Proof plan

Evidence log: `planning/plk-gate/evidence/runtime-tests.log`.

Runtime claim: Nested `agent-bash` children inherit the parent invocation environment and durably record the child row's `parent_invocation_id` as the parent's StateDb row id.

Proof method: `src-tauri/tests/pr_a_invocation_integration.rs::nested_agent_bash_chain_records_parent_id_from_inherited_env`.

Evidence-class match: particular-integration; the test uses a real `agent-bash` binary supplied through `AGENT_BASH_BIN`, dispatches a nested runner command, waits for `DONE rc=0`, parses the captured child `OULIPOLY_INVOCATION` marker, and asserts the child row's `parent_invocation_id` equals the parent row id. The evidence log records this test as `ok` under an XDG-isolated command with `env -u OULIPOLY_DATA_DIR`.

Runtime claim: Same-DB UUID parent resolution tolerates provider/source name drift.

Proof method: `src-tauri/src/dispatch.rs::tests::resolve_parent_invocation_id_uses_same_db_uuid_despite_source_name_drift`.

Evidence-class match: unit; the test seeds a parent row with provider name `fixture-provider`, serializes a parent env value with source `renamed-provider` and the same UUID, and asserts `resolve_parent_invocation_id` returns the seeded row id. The evidence log records this targeted unit test as `ok` under an XDG-isolated command with `env -u OULIPOLY_DATA_DIR`.

Runtime claim: Malformed parent env values and unknown parent UUIDs remain safe root-invocation cases, not panics or invalid links.

Proof method: `src-tauri/tests/pr_a_invocation_integration.rs::ignores_malformed_and_unresolved_parent_env_values` plus `src-tauri/src/dispatch.rs::tests::resolve_parent_invocation_id_returns_none_for_malformed_json`, `resolve_parent_invocation_id_returns_none_for_unknown_uuid`, and `resolve_parent_invocation_id_returns_none_for_invalid_uuid_format`.

Evidence-class match: integration plus unit; the integration test runs the binary with malformed JSON, a valid but absent UUID, and an invalid UUID string, then asserts each resulting row has `parent_invocation_id == None`. The dispatch unit tests cover the resolver directly. The evidence log records the integration suite as passing; the direct unit tests are also covered by the prior full workspace gate.

Runtime claim: Trace reconciles stale-running rows only when sidecar PID evidence proves the recorded process is dead, then persists durable failed terminal fields.

Proof method: `src-tauri/tests/pr_b_trace_integration.rs::trace_reconciles_liveness_stale_running_row_with_dead_pid`.

Evidence-class match: particular-integration; the test seeds a running invocation older than the stale threshold, writes a PID identity sidecar row for an impossible PID, runs `trace --json`, and asserts JSON plus stored StateDb row show `status=failed`, `success=false`, `exit_code=-1`, `error_category=stale_running`, `terminal_reason=stale_running_liveness`, and a non-null `finished_at`. The evidence log records this test as `ok` under an XDG-isolated command with `env -u OULIPOLY_DATA_DIR`.

Runtime claim: Trace keeps the existing JSON-only stale-running lift without mutating DB state when no conclusive PID sidecar evidence exists.

Proof method: `src-tauri/tests/pr_b_trace_integration.rs::trace_json_stale_running_row_is_lifted_without_mutating_db`.

Evidence-class match: particular-integration; the test seeds an old running row without sidecar proof, runs `trace --json`, asserts the rendered invocation is lifted with `terminal_reason=tracing_timeout` and `stale_running` warning metadata, then reopens StateDb and asserts the row remains `status=running` with null terminal fields. The evidence log records this test as `ok`.

Runtime claim: Fresh running rows remain running with null terminal fields and no stale-running warning.

Proof method: `src-tauri/tests/pr_b_trace_integration.rs::trace_json_running_row_uses_null_terminal_fields_and_no_stale_warning`.

Evidence-class match: particular-integration; the test seeds a fresh running row, runs `trace --json`, and asserts `success`, `exit_code`, `terminal_reason`, and `finished_at` are null with no `stale_running` object or warning. The evidence log records this test as `ok`.
