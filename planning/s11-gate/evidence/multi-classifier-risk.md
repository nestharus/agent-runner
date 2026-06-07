# S11 Multi-Classifier Risk Notes

This artifact is a pre-audit routing aid, not a waiver and not a verdict. The function-classification auditor remains authoritative for A1 single-classification scoring.

The stale pre-remediation function-classification report is preserved separately at `planning/s11-gate/evidence/pre-split-multi-classifier-risk.md`. The current working tree contains a targeted S11 function-classification remediation: helper responsibilities were split across the provider client/process layer, external-provider request/context/policy helpers, runtime executor/provider-settings helpers, state/mailbox helpers, OpenCode turn adapter helpers, balancing/resume/terminal outcome helpers, and touched tests. Current function bodies remain blocking if they still mix A1 categories; this file is not a waiver.

## Genuine Risk Surfaces

| Surface | Why It Is Risky | Audit Expectation |
|---|---|---|
| `src-tauri/src/run/resume/orchestration.rs` | Resume loops, external-provider resume selection, terminal-signal handling, mailbox confirmation, and retry outcomes live in one large orchestration file. | Inspect changed helpers for inline domain logic; orchestration-only helper dispatch can be LOW, but inline validation plus formatting/mapping should be reported. |
| `crates/oulipoly-provider/src/client.rs` | Provider invoke/launch orchestration, request/response validation, stdout parsing, and protocol error mapping are adjacent. | Confirm parsing, validation, and error mapping are split from orchestration helpers. |
| `crates/oulipoly-provider/src/process.rs` | Process execution, timeout/cancellation predicates, byte accumulation, and process outcome mapping are adjacent. | Confirm byte-window helpers, timeout predicates, termination diagnostics, and process-run orchestration are separated. |
| `crates/oulipoly-provider/src/testkit.rs` | Testkit fake-provider compilation, wrapper formatting, process spawning, leak probing, PID parsing, and assertions are adjacent. | Treat as test harness, but classify executable helpers normally. |
| `crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs` | Fake provider mode dispatch, protocol formatting, stdin parsing, process spawning, and heartbeat fixtures are adjacent. | Treat as test fixture, but classify helper functions normally; no generated/test-fixture exemption for mixed functions. |
| `crates/oulipoly-provider/tests/launch_stream_lifecycle.rs` | Launch lifecycle tests mix client construction, cancellation/timeout orchestration, and assertion helpers. | Confirm helpers and tests remain single-role or pure test orchestration. |
| `src-tauri/src/mailbox_delivery.rs` | Notification prompt formatting, nonce/hash marker construction, success/failure delivery mapping, and sidecar updates are adjacent. | Confirm marker formatting, delivery predicates, and DB update orchestration stay separated by function. |
| `src-tauri/src/wake_coordinator.rs` | Wake claim selection, PTY/headless branch selection, command construction, retry decisions, and sidecar writes are adjacent. | Confirm wake decision predicates and command formatting are not mixed into state mutation functions. |
| `crates/oulipoly-state/src/mailbox.rs` | Sidecar SQL accessors, row mapping, claim predicates, and delivery-attempt updates are adjacent. | Confirm SQL access/update helpers do not also perform unrelated policy decisions. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | Process identity access, validation, and sidecar owner/session persistence are adjacent. | Confirm process identity validation and sidecar writes are separated or pure orchestration. |
| `crates/oulipoly-runtime/src/executor/external_provider/dispatch.rs` | External provider policy/launch sequencing, process identity backfill, and launch result classification are adjacent. | Confirm dispatch functions are pure orchestration over helper calls. |
| `crates/oulipoly-runtime/src/executor/external_provider/error_mapper.rs` | Provider-client transport/category mapping and rotatable/terminal predicates are adjacent after the attempt-4 transport rotation fix. | Confirm rotatable classification predicates and error DTO/service-error mappers stay separated by function. |
| `crates/oulipoly-runtime/tests/age246_external_transport_rotation.rs` | Integration fixture formats provider scripts, writes model/provider configs, runs dispatch, and validates rotation results. | Confirm fixture helpers keep formatting, file materialization, runtime invocation, and assertions split or pure orchestration. |
| `scripts/opencode-turns` | Python adapter contains many parser/mapper/filter/formatter helpers and is included because S11 repairs its executable bit. | Existing adapter/harness classification should still be inspected because the touched-file rule includes the file. |

## Non-Waiver Notes

- `crates/oulipoly-provider/src/generated.rs` is not exempt as generated code.
- Historical `planning/s10b-gate/.scratch/**` logs are touched artifacts but contain no executable source functions.
- The executable-bit remediation for `scripts/opencode-turns` is a source delta even though it has no textual function-body change.
