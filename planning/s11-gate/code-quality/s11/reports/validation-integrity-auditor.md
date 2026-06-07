# Validation-integrity audit report

## Inputs read

| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| operator | `/home/nes/ai/agents/validation-integrity-auditor.md` | 11 070 B | `6983abb608` | Read fully |
| diff (mode=pr-diff) | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/diff.patch` | 2 740 490 B | `4b00274453` | 10 970 lines; read in targeted chunks covering every test/fixture hunk and all section headers |
| runtime-artifact evidence | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/evidence/runtime-tests.log` | 8 136 B | `3fe3adaa82` | Read fully; references targeted `cargo test` runs and live `/tmp/s11-e2e` SQLite artifacts |
| decisions | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 490 293 B | `ad14421ed0` | Read first 100 lines (S10B ratifications added by this diff); no S11-specific weakening ratification entry — not needed, no VI pattern fires |
| contract | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/contracts/s11.contract.md` | 31 782 B | `0da6c9e414` | Read fully; test-harness and adapter surface declarations resolved |
| proposal | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/proposal.md` | 9 980 B | `1468609afb` | Read fully; proof plan and runtime claim identity resolved |
| worktree | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | — | — | Used to resolve all evidence paths |
| runtime_claim | (inline) | — | — | S11 mailbox delivery honesty, detached wake provider reload, transport timeout pool rotation, no state.db schema migration |
| wu_id | `s11` | — | — | Applied for report-local finding namespacing |

## Patterns detected

| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| — | — | No pattern fired | — | — | — | n/a | n/a |

## Ratification evidence

| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| — | n/a — no findings | n/a | n/a |

## Per-hunk validation-surface analysis

Every test-file section and production hunk of the diff was inspected against all six VI patterns.

### DECISIONS.md

Two S10B ratification entries added (`D-S10B-VI-001`, `D-S10B-VI-002`). These are prior-gate entries; no S11-specific weakening ratification entry is present or required. No validation surface change.

### `crates/oulipoly-provider/src/client.rs`

`ProviderTimeouts::default` corrects the handshake timeout from 30 s to 90 s and the launch heartbeat-gap from 300 s to 120 s. These are production behavior corrections recorded in commit `a1a3ca1` and proved by `age246_external_transport_rotation`. `invoke_json` body is refactored into extracted helpers (`validate_json_request`, `ensure_invocation_stdout_within_limit`, `ensure_invocation_stdout_present`, `parse_invocation_stdout_object`). All same error conditions preserved; no assertion removed; no condition relaxed.

### `crates/oulipoly-provider/src/error.rs`

`parse_error_response_envelope` is refactored to produce an `ErrorResponseEnvelopeParseError` struct that carries both `request_id` and the `serde_json::Error`. The `request_id` field value flows through unchanged into `schema_invalid_error_response` via `error.request_id`. No assertion removed; error categorisation identical.

### `crates/oulipoly-provider/src/generated.rs`

`TrueBool` and `FalseBool` deserialization changed from `bool::deserialize(deserializer)?` (generic typed deserialize) to `deserializer.deserialize_bool(TrueBoolVisitor)` (explicit bool hint). This is strictly **more restrictive**: the generic path could accept coercible representations in some formats; the bool visitor rejects non-boolean tokens. `FixedStrType` macro changed from `String::deserialize(deserializer)?` to `deserializer.deserialize_str(FixedStrTypeVisitor::new(...))`, equally stricter. **No VI-006 schema relaxation.**

### `crates/oulipoly-provider/src/process.rs`

New `ProcessSpawnObserver` / `ProcessOutcome` types added. The spawn observer sends the child PID over an `mpsc` channel so the external dispatch layer can capture provider process identity for sidecar ownership. No existing validation logic removed or weakened.

### `crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs`

New mode branches added for heartbeat-gap coverage (`LaunchHeartbeatsThenExit`, `HeartbeatThenChildGrandchildHang`, parameterised `provider_error` category). The fake provider is a compiled Rust executable declared as `test-harness` in the contract. No real runtime dependency replaced by a local proxy. **No VI-004 or VI-005.**

### `crates/oulipoly-provider/tests/launch_stream_lifecycle.rs`

Two new tests added:
- `launch_heartbeat_activity_keeps_long_running_stream_alive` — asserts `ProcessStatus::Exited { code: 0 }` and checks session capture is present.
- `launch_heartbeat_gap_timeout_cleans_descendants_and_preserves_stderr_diagnostics` — asserts `transport_kind() == "host_timeout"` and `request_id() == Some(REQUEST_ID)`.

Both use `FakeProvider::compile`, spawn real processes, and make concrete assertions. No skip markers; no mock substitution.

### `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs`

Key changes inspected:

1. `NEUTRAL_SETTINGS_ID` constant replaced by `SELECTED_PROVIDER_SETTINGS_ID`. Test renamed from `…carries_neutral_settings_id…` to `…carries_selected_settings_id…`; assertion values updated. This is a **correction** of test expectation to match the S11 runtime behavior change (selected provider settings id now flows through policy and launch); assertion predicate is equally strong, just targeting the new correct value.

2. `assert!(error.to_string().contains("policy rejected"))` **strengthened** — additional assertions added requiring `message.contains("policy_live_contract_reject")`, `message.contains("params.launch.argv")`, and `message.contains("provider expected account-specific policy argv shape")`.

3. Six new tests added covering hybrid launch shape, host linkage env, ambient-OpenAI exclusion, selected OpenCode account auth, ambient XDG omission, and canonicalized account-one settings ID.

**No VI-001** (assertion removal). **No VI-004** (all tests use declared test-harness fixtures). No skip markers.

### `crates/oulipoly-runtime/tests/age246_external_transport_rotation.rs` (new file)

New integration test file with four tests covering transport timeout rotation, heartbeat-gap timeout rotation, provider unavailable rotation, and all-slow bounded terminal failure. Uses a real Python script fake-provider executable; makes concrete assertions on success/failure category and pool-account ordering. Purely additive.

### `crates/oulipoly-runtime/tests/provider_registry.rs`

`collect_paths` inlined `snapshot_dir_paths` helper; `session_external_provider_sources` inlined iteration. Identical behaviour; same `expect()` failure mode. **Pure refactor. No VI-001.**

### `crates/oulipoly-runtime/tests/provider_settings_host.rs`

`settings_host_invokes_only_schema_and_settings_subcommands_with_typed_envelopes` refactors inline assertions into helper functions:
- `assert_settings_host_subcommands_are_allowed` — contains identical `matches!` predicate and message.
- `assert_common_settings_call_envelopes` — contains identical `assert_eq!` calls for `contract`, `request_id`, `provider_instance_id`, `host.config_root`, `host.data_root`, and `host.env`.
- `non_describe_subcommands`, `recorded_call_for_subcommand`, `assert_recorded_call_params` — identical iteration and predicate logic.

All removed inline blocks are re-expressed in the extracted helpers with identical runtime conditions and the same failure messages. **No VI-001.**

### `src-tauri/tests/wu_b_mailbox_integration.rs`

- Six `OULIPOLY_AUTO_WAKE*` env-remove calls added: prevents ambient parent-process wake env from leaking into test isolation. Strengthens test validity; no weakening.
- `write_notify_artifacts` refactored into artifact-path and content helpers. Same file writes; no stub substitution. **No VI-005.**
- `assert!(prompt_dump.exists(), "{output:?}")` added to existing test: new assertion (strengthening).
- Two new tests: `resume_marks_delivered_from_exact_ingested_user_turn_without_assistant_delta` and `resume_rejects_different_ingested_user_turn_without_assistant_delta`. Both run real CLI commands, write real SQLite rows, and assert `delivery_attempts`, `delivered_at`, and `delivery_error`. No mocks; no skips.
- Helper refactoring for `caller_chain`, `stdout_json`, `inserted_row`: identical behaviour.

### `src-tauri/tests/s11_external_provider_wake.rs` (new file)

New integration test file with 8 tests (all confirmed passing in `runtime-tests.log`). Tests compile and run a real external provider Python script fixture, execute the real `runner_bin()` binary against isolated XDG directories, write to real SQLite databases, and assert on mailbox row state, process identity captures, wake spawn, and retry behaviour. No skip markers; no mock substitution replacing a real runtime path. Fixture is declared `test-harness` in the contract.

### Production code (balancing, resume orchestration, wake coordinator, mailbox state)

All changes are production implementation: extraction of helper functions preserving identical validation predicates, refactoring of the provider-ref migration-skip conditional, addition of delivery-attempt tracking, and pool-rotation dispatch. No test assertions removed; no test validation surface changed.

## Runtime-artifact evidence assessment

The `runtime-tests.log` evidence (8 136 B, SHA prefix `3fe3adaa82`) is non-empty and directly references S11 runtime artifacts built and executed in the worktree:

- Targeted `cargo test -p oulipoly-provider`: `provider_status=0`; heartbeat-gap tests at `targeted-tests.log:180–184`.
- Targeted `cargo test -p oulipoly-runtime`: `runtime_status=0`; `age246_external_transport_rotation` `4 passed`; `age217_s6a_policy_launch_dispatch` `26 passed`; `external_launch_heartbeat_gap_timeout_rotates_to_next_account_and_succeeds` confirmed at line 370.
- Targeted `cargo test` (tauri): `tauri_status=0`; `s11_external_provider_wake` `8 passed`; `wu_b_mailbox_integration` `20 passed`.
- Historical full-workspace gate (`cargo test --workspace`): `test_status=0`; all S11-specific tests individually `ok`.
- Live smoke: `/tmp/s11-e2e/fresh10-xdg-data/oulipoly-agent-runner/pid-identity.db` rows read via Python SQLite as `CONFIRMED-DELIVERED`, `delivery_attempts=1`, `delivery_error=NULL`, confirming real sidecar mailbox state with production binary execution.

Because all VI patterns evaluated against the contract-declared validation surfaces are `NO-FIRE`, no ratification downgrade is needed.

## Residual ambiguity / stop-condition notes

None. All required inputs were present and readable. Diff is parseable. No human-owned ambiguity would materially change the verdict. No S11-specific validation-surface weakening ratification entry is required in DECISIONS.md because no VI pattern fired against S11 changes.

LOW
