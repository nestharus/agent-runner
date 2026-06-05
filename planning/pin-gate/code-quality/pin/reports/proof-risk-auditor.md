# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | phase-6 | N/A | N/A | Treated as Phase 6 per-component code-quality context per caller; contract was required and read before scoring. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | N/A | N/A | Used to resolve repository evidence paths. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/proposal.md` | 4461 | `f61075b98992` | Readable; exact `## Proof plan` found at `proposal.md:7`. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/contracts/pin.contract.md` | 5782 | `6a6de10779ee` | Readable and non-blank; declares the data-dir mapper and spawn formatter surfaces. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/diff.patch` | N/A | N/A | Read to verify named tests exist in the delta. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/touched-surfaces.md` | N/A | N/A | Read for delta scope. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | N/A | N/A | Read as required by caller; Phase 6 contract visibility rule is at `code-quality.md:169-173`. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | `planning/pin-gate/proposal.md:7` |
| `Runtime claim` | Yes | Seven runtime claims at `planning/pin-gate/proposal.md:9`, `proposal.md:13`, `proposal.md:17`, `proposal.md:21`, `proposal.md:25`, `proposal.md:29`, `proposal.md:33`. |
| `Proof method` | Yes | Each claim names a test method at `planning/pin-gate/proposal.md:10`, `proposal.md:14`, `proposal.md:18`, `proposal.md:22`, `proposal.md:26`, `proposal.md:30`, `proposal.md:34`. |
| `Evidence-class match` | Yes | Each proof method has an explicit evidence-class statement at `planning/pin-gate/proposal.md:11`, `proposal.md:15`, `proposal.md:19`, `proposal.md:23`, `proposal.md:27`, `proposal.md:31`, `proposal.md:35`. |
| Self-certification | No | No claim relies on the proof plan itself or unnamed pass/fail status; each proof method names a concrete test path and method. |

## Findings
| Finding ID | Severity | Runtime claim | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|
| None | LOW | No missing proof-plan structure, self-certification, proxy-only runtime proof, or evidence-class mismatch found. | N/A | N/A | N/A | Structure: `planning/pin-gate/proposal.md:7-35`; contract surfaces: `planning/pin-gate/contracts/pin.contract.md:32-37`, `pin.contract.md:54-67`; test existence and assertions: `planning/pin-gate/gates/diff.patch:443-545`, `diff.patch:255-371`, `diff.patch:716-792`; runtime pass evidence: `planning/pin-gate/evidence/runtime-tests.log:2-72`. | No |

## Evidence-class decision

The contract binds the component to runtime data-dir resolution and provider spawn environment materialization: `crates/oulipoly-state/src/paths.rs::data_dir` maps the process pin/default platform data source into the canonical app data directory at `planning/pin-gate/contracts/pin.contract.md:32`, `default_data_dir` maps `dirs::data_dir()` into `oulipoly-agent-runner` at `pin.contract.md:33`, `StateDb::default_path` and `PidIdentityDb::default_path` consume that canonical data dir at `pin.contract.md:34-35`, and `command_format.rs::command_from_parts` plus `pin_agent_data_dir_if_unset` materialize spawned provider `Command` env shape at `pin.contract.md:36-37`. The adapter declarations name the same external contracts at `pin.contract.md:56-67`.

The state path claims are runtime API claims, not helper-internal proxy claims. The named `crates/oulipoly-state/tests/data_dir_precedence.rs` tests exist in the delta at `planning/pin-gate/gates/diff.patch:443-545`. The first sets both `OULIPOLY_DATA_DIR` and `XDG_DATA_HOME`, then asserts shipped default path APIs resolve under the pinned app data dir at `crates/oulipoly-state/tests/data_dir_precedence.rs:46-53` and `data_dir_precedence.rs:65-78`. The second removes the pin, sets `XDG_DATA_HOME`, and asserts the same shipped APIs resolve under `XDG_DATA_HOME/oulipoly-agent-runner` at `data_dir_precedence.rs:56-63` and `data_dir_precedence.rs:65-78`. This evidence class matches the claimed runtime default-path resolution behavior.

The spawn-side sidecar claim is backed by a real `RuntimeExecutorService::execute` provider spawn and sidecar DB read. The named `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs::spawn_capture_writes_verified_sidecar_row_without_state_schema_change` test exists in the delta at `planning/pin-gate/gates/diff.patch:255-310`. It removes the harness pin while setting `XDG_DATA_HOME` at `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs:23-31`, executes the runtime executor at `age_pid_sidecar_spawn.rs:88-104`, reads the actual PID identity sidecar DB and row at `age_pid_sidecar_spawn.rs:106-115`, and verifies state schema stability at `age_pid_sidecar_spawn.rs:117-119`. This is runtime sidecar evidence for the sidecar-row claim.

The pre-existing pin preservation claim is backed by a real fixture provider child that records its environment. The named `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs::spawn_preserves_preexisting_oulipoly_data_dir_in_provider_child` test exists in the delta at `planning/pin-gate/gates/diff.patch:312-371`. The test sets both parent `OULIPOLY_DATA_DIR` and isolated `XDG_DATA_HOME` at `age_pid_sidecar_spawn.rs:39-46` and `age_pid_sidecar_spawn.rs:135-138`, executes the runtime executor at `age_pid_sidecar_spawn.rs:138-152`, records `${OULIPOLY_DATA_DIR:?missing OULIPOLY_DATA_DIR}` in the provider script at `age_pid_sidecar_spawn.rs:202-207`, asserts the child-recorded env equals the custom pin at `age_pid_sidecar_spawn.rs:154-156`, and asserts the runner's XDG-derived default app data dir was not created at `age_pid_sidecar_spawn.rs:157-160`. This is child-process environment evidence, not a static or mocked proxy.

The spawned-provider pin, shadow-XDG notify, and wake-resume pin claims are all bound to the same live-bug reproduction test. The named `src-tauri/tests/wu_d_proactive_wake_integration.rs::provider_shadow_xdg_notify_uses_pinned_data_dir_and_wakes` test exists in the delta at `planning/pin-gate/gates/diff.patch:716-792`. The fixture calculates the expected pinned app data dir and starts from an unpinned harness env at `src-tauri/tests/wu_d_proactive_wake_integration.rs:69-80`. The provider script exits if the real spawned child lacks `OULIPOLY_DATA_DIR`, then shadows `XDG_DATA_HOME` before running descendant `notify` at `wu_d_proactive_wake_integration.rs:664-675`. The test verifies the resumed prompt contains `handle: h-shadow-xdg` at `wu_d_proactive_wake_integration.rs:682-683`, verifies the resumed provider child recorded the pinned app data dir at `wu_d_proactive_wake_integration.rs:684-687`, verifies the original mailbox row is delivered and the wake claim is cleared at `wu_d_proactive_wake_integration.rs:688-696`, and verifies the shadow XDG app state directory was not created at `wu_d_proactive_wake_integration.rs:697-705`. This is production-shaped command/process/mailbox/sidecar evidence for the runtime claims, not proxy-only fixture success.

The runtime evidence log records that all named tests completed successfully: the state precedence tests at `planning/pin-gate/evidence/runtime-tests.log:2-5`, the sidecar spawn test at `runtime-tests.log:6-8`, the proactive wake integration test at `runtime-tests.log:9-19`, the pre-existing pin preservation focused run at `runtime-tests.log:67-69`, and the wake-resume focused run at `runtime-tests.log:70-72`.

## Residual ambiguity / stop-condition notes

No stop condition fired. The proposal and contract were readable; the contract was non-blank and was read before scoring; `## Proof plan` was parseable; named tests exist in the diff and assert the runtime surfaces claimed by the plan. The caller supplied `mode=phase-6`; this report treats that as the Phase 6 per-component code-quality context described by the prompt and by `~/ai/conventions/code-quality.md:169-173`, rather than as a Phase 3/RCA proposal mode.

VERDICT: LOW
