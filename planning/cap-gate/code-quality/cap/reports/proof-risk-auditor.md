# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | phase-6 | n/a | n/a | Caller phase is Phase 6 per-component; scored as the supplied proposal proof-risk gate after reading the Phase 6 contract. |
| worktree_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar | n/a | n/a | Used to resolve repository evidence paths. |
| proposal_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/proposal.md | 4029 | 7151e895585e | Read. Exact `## Proof plan` at proposal.md:11. |
| contract_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/contracts/cap.contract.md | 5290 | f5c1a2d1a8d8 | Read before scoring. Contract scopes production substance to commit `9e00408` and test-only isolation sweep to `9ba1275` at cap.contract.md:3 and cap.contract.md:39-41. |
| diff_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/gates/diff.patch | 73746 | 583ecb85c9a0 | Read to verify named proof methods in the delta. |
| touched_surfaces_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/gates/touched-surfaces.md | 1393 | fbb96345368f | Read for incremental gate scope and test inventory. |
| evidence log | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/evidence/runtime-tests.log | 1922 | c99a229acb94 | Read for shipped test pass and poison-pin full-run evidence. |
| code-quality convention | /home/nes/ai/conventions/code-quality.md | 30798 | fa8b6499cc2e | Read per caller instruction. |
| report_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/code-quality/cap/reports/proof-risk-auditor.md | n/a | n/a | Written by this audit. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | Exact heading at proposal.md:11. |
| `Runtime claim` | Yes | Four claim rows at proposal.md:13, proposal.md:17, proposal.md:21, and proposal.md:25. |
| `Proof method` | Yes | Four proof-method rows at proposal.md:14, proposal.md:18, proposal.md:22, and proposal.md:26. |
| `Evidence-class match` | Yes | Four evidence-class rows at proposal.md:15, proposal.md:19, proposal.md:23, and proposal.md:27. |
| Self-certification only | No | The proof plan names concrete tests and an evidence log rather than relying only on the proof plan text or unnamed test-pass claims. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| None | LOW | No missing proof-plan structure, self-certification, proxy-only proof for a runtime-artifact-bound claim, or evidence-class mismatch found. | proposal.md:11-27 | Named state-backed runtime integration tests, E2E CLI integration test, and scoped harness-immunity evidence. | Not proxy-only for runtime claims; the poison-pin claim is itself a test-harness isolation claim. | Runtime executor/state DB sidecar path for capture claims; E2E runner binary path for mid-turn notify; XDG-isolated harnesses for poison-pin sweep. | age_pid_sidecar_spawn.rs:136-212, age_pid_sidecar_spawn.rs:216-251, wu_d_proactive_wake_integration.rs:637-718, age100_one_shot_quota_migration.rs:99-112, age100_one_shot_quota_migration.rs:217-263, runtime-tests.log:2-28 | no |

## Evidence-class decision

Claim 1 at proposal.md:13 is runtime-artifact-bound to the production executor/state sidecar behavior: a fresh stdout-json capture spawn must backfill the PID sidecar session id and write `session_runtime` running state. The proof method at proposal.md:14 names `stdout_json_event_capture_backfills_sidecar_and_marks_runtime_running`, added in the delta at diff.patch:796-873 and present in the current tree at crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs:136-212. The test drives `RuntimeExecutorService::execute` with a real stdout-json fixture at age_pid_sidecar_spawn.rs:147-170, then asserts exactly one sidecar row and the captured session id at age_pid_sidecar_spawn.rs:173-177, `session_runtime` running identity fields at age_pid_sidecar_spawn.rs:179-200, and the later idle transition at age_pid_sidecar_spawn.rs:202-212. The evidence class is state-backed runtime integration, not a mock-only proxy, and runtime-tests.log:2-7 records the shipped test passing.

Claim 2 at proposal.md:17 is a negative runtime integration claim: captured stdout session id without spawn identity must not create or backfill a sidecar row. The proof method at proposal.md:18 names `stdout_json_event_capture_without_spawn_identity_does_not_backfill_sidecar`, added in the delta at diff.patch:876-912 and present at crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs:216-251. The test executes the same capture provider with `parent_invocation_env: None` at age_pid_sidecar_spawn.rs:226-245 and asserts the sidecar DB path does not exist at age_pid_sidecar_spawn.rs:248-251. That evidence directly exercises the runtime precondition boundary named by the claim, and runtime-tests.log:2-7 records the test passing.

Claim 3 at proposal.md:21 is runtime-artifact-bound to the live race surface: mid-turn notify on an opencode-style stdout-capture spawn must resolve the owner via the capture-time sidecar row and deliver the mailbox row. The proof method at proposal.md:22 names `opencode_mid_turn_notify_resolves_capture_time_sidecar_owner`, added in the delta at diff.patch:1607-1689 and present at src-tauri/tests/wu_d_proactive_wake_integration.rs:637-718. The test writes an opencode stdout-json capture provider at wu_d_proactive_wake_integration.rs:640-657, triggers the mid-turn notify after capture at wu_d_proactive_wake_integration.rs:644-665, asserts notify `status = enqueued`, `owner_session_id = ses_capturemidturn`, `session_source = sidecar_session_id`, and `wake.status = busy` at wu_d_proactive_wake_integration.rs:668-689, asserts the sidecar row contains the captured session id at wu_d_proactive_wake_integration.rs:691-694, and verifies mailbox delivery plus final idle runtime at wu_d_proactive_wake_integration.rs:695-716. This is an E2E CLI integration test using `CARGO_BIN_EXE_oulipoly-agent-runner` through the fixture command path at wu_d_proactive_wake_integration.rs:74-105 and wu_d_proactive_wake_integration.rs:1013-1015; runtime-tests.log:8-19 records it passing.

Claim 4 at proposal.md:25 is scoped to test-harness isolation behavior, not production runtime behavior: XDG-isolated runtime and src-tauri test harnesses must be immune to a poison `OULIPOLY_DATA_DIR` pin, including the age100 live-failure class. The proof method at proposal.md:26 names the runtime evidence log plus the shipped age100 spot proof. The age100 harness scrub is in the delta at diff.patch:1007-1018 and in the current tree at src-tauri/tests/age100_one_shot_quota_migration.rs:99-112, while the shipped spot test and its behavioral assertions are at src-tauri/tests/age100_one_shot_quota_migration.rs:217-263. The runtime-tests.log evidence records the age100 poisoned spot proof passing at runtime-tests.log:23-25 and the full poisoned and clean workspace summaries at runtime-tests.log:26-28. Because the claim is explicitly about test-harness immunity and the proof plan admits that no single shipped test alone proves all 51 scrubbed harnesses at proposal.md:27, the full-run log is the matching evidence class for the sweep while the shipped age100 test exercises the named live-failure class.

## Residual ambiguity / stop-condition notes

No stop condition fired: the proposal, contract, diff, touched-surface inventory, runtime evidence log, and code-quality convention were readable, and the report path was writable. The caller supplied `mode=phase-6`; this report treats that as the Phase 6 per-component invocation context and uses the supplied proposal and contract as required Phase 6 inputs rather than blocking on artifact-context terminology.

The age100 spot-proof test function was not newly added in the diff; the delta adds the `OULIPOLY_DATA_DIR` scrub to its harness at diff.patch:1011-1018, and the current shipped test exists at src-tauri/tests/age100_one_shot_quota_migration.rs:217-263. This is not a proof-risk finding because the proof plan cites the shipped spot test plus the runtime evidence log, and the claim is harness-immunity scoped.

VERDICT: LOW
