# s10 Step-6a Proposal

This incremental delta preserves the parent-linkage and PID-sidecar behavior already shipped in PLK while adding S10 carriers around external provider launch session capture and moved-provider configuration references. The runtime change maps a provider launch `exit.session.provider_session_id` into `SessionCaptureResult` with method `external_provider_launch`, so a later resume request can pass the captured provider session id back to the external provider. The setup and config-migration changes backfill the moved provider's external-provider binary reference without changing `state.db` schema or PLK parent-linkage semantics.

No genuine MULTI-CLASSIFIER-RISK was identified in the pre-gate review. The new production helpers are single-role mappers, predicates, accessors, or orchestration wrappers over named helpers. If an auditor finds a remaining multi-classifier function, it should be split before the gate is considered closed.

## Proof plan

Evidence log: `planning/s10-gate/evidence/runtime-tests.log`.

Runtime claim: Nested `agent-bash` children inherit `OULIPOLY_PARENT_INVOCATION` and durably record the child row's `parent_invocation_id` as the parent's StateDb row id.

Proof method: `src-tauri/tests/pr_a_invocation_integration.rs::nested_agent_bash_chain_records_parent_id_from_inherited_env`.

Evidence-class match: particular-integration; the test uses a real `agent-bash` binary supplied through `AGENT_BASH_BIN`, dispatches a nested runner command, waits for `DONE rc=0`, reads the captured child invocation marker, and asserts the child StateDb row links to the parent row id. The S10 evidence log records this test as `ok` under an XDG-isolated command with `env -u OULIPOLY_DATA_DIR`.

Runtime claim: Trace reconciles stale `running` rows only when PID sidecar evidence conclusively proves the recorded process identity is dead, and persists durable failed terminal fields for that row.

Proof method: `src-tauri/tests/pr_b_trace_integration.rs::trace_reconciles_liveness_stale_running_row_with_dead_pid` plus `src-tauri/tests/pr_b_trace_integration.rs::trace_json_stale_running_row_is_lifted_without_mutating_db` as the non-conclusive/no-sidecar control.

Evidence-class match: particular-integration; the positive test seeds a stale `running` invocation row and a PID identity sidecar row for a dead/impossible PID, runs `trace --json`, and asserts the rendered JSON plus stored StateDb row show failed stale-running terminal state with `terminal_reason=stale_running_liveness`. The control test seeds a stale `running` row without conclusive dead PID sidecar evidence, runs `trace --json`, and asserts the JSON lift does not mutate the stored StateDb row's running state or terminal fields. The S10 evidence log records both tests as `ok` under XDG-isolated commands with `env -u OULIPOLY_DATA_DIR`.

Runtime claim: Same-DB UUID parent resolution tolerates provider/source-name drift while preserving malformed or unknown parent values as root-invocation cases.

Proof method: `src-tauri/src/dispatch.rs::tests::resolve_parent_invocation_id_uses_same_db_uuid_despite_source_name_drift` plus the existing malformed/unknown resolver tests shipped in the same test module.

Evidence-class match: unit; the source-drift test seeds a parent row with one provider name, serializes an inherited parent env value with a different source name but the same invocation UUID, and asserts `resolve_parent_invocation_id` returns the seeded same-DB row id. The S10 evidence log records the source-drift test as `ok` under an XDG-isolated command with `env -u OULIPOLY_DATA_DIR`; the malformed/unknown cases remain shipped in the same resolver unit suite and are unchanged by this delta.

Runtime claim: External provider launch exit session metadata populates runtime session capture and is carried into the next external provider resume request.

Proof method: `crates/oulipoly-runtime/tests/s10_external_launch_session.rs::external_launch_exit_session_populates_capture_and_resume_request`.

Evidence-class match: particular-integration; the test runs the production `RuntimeExecutorService` against an executable external-provider fixture that emits launch stdout and an exit event containing `session.provider_session_id`, then asserts the first result records `external_provider_launch` capture and the second launch request contains `known_provider_session_id`. The S10 evidence log records this test as `ok`.

Runtime claim: The S10 moved-provider setup/config carriers backfill the external-provider binary ref without regressing migrated runtime config separation or source-guard thresholds.

Proof method: `crates/oulipoly-setup/src/context.rs::tests::*`, `src-tauri/src/commands/config_migration/tests::migrate_config_backfills_moved_model_external_provider_binary`, `migrate_config_backfills_session_storage_from_turn_scripts`, `migrate_config_keeps_model_only_interactive_args_out_of_provider_conflict`, `crates/oulipoly-runtime/tests/age244_s7b_export_replace_dispatch.rs`, `src-tauri/tests/age245_s7c_rotation_source_guard.rs`, and `src-tauri/tests/age246_s8_setup_dispatch_source_guard.rs`.

Evidence-class match: unit plus source-guard integration; the setup tests assert generated prompts include the moved external-provider ref, config-migration tests assert idempotent binary backfill and runtime/model argument separation, and the source-guard suites assert no new concrete provider vocabulary outside approved scopes. The S10 evidence log records these touched suites as passing.
