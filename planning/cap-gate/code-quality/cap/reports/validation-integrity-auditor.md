# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | N/A | N/A | Repository root used to resolve evidence paths. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/contracts/cap.contract.md` | 5290 | `f5c1a2d1a8d8` | Required Phase 6 contract. Read before scoring. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/proposal.md` | 4029 | `7151e895585e` | Required Phase 6 proof intent and runtime claim context. Read before scoring. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/gates/diff.patch` | 73746 | `583ecb85c9a0` | Unified incremental diff over `42200fb..9ba1275`. |
| runtime_claim | inline caller value | 679 | `4ed5fdc3156e` | Claim covers capture-time sidecar backfill, mark-running, mid-turn notify, negative no-identity case, and poison `OULIPOLY_DATA_DIR` immunity. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 487336 | `3dd8fe295119` | Read for possible validation-surface weakening ratification; no ratification was needed. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/evidence/runtime-tests.log` | 1922 | `c99a229acb94` | Runtime test evidence supplied by caller. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Required by caller; confirms active ACR-254 layer and Phase 6 contract visibility. |
| touched surfaces context | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/gates/touched-surfaces.md` | 1393 | `fbb96345368f` | Context for production delta and test-only isolation sweep. |
| source context | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/tests/wu_d_proactive_wake_integration.rs` | 33126 | `b89b193414b7` | Read to verify the mid-turn ordering inside the fixture, not just the patch hunk. |
| source context | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs` | 14925 | `f2a0a67ef248` | Read to verify state-backed assertions and the retained provider-child data-dir behavior. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | N/A | No validation-weakening pattern fired. The diff adds assertions and env isolation, with no assertion removals, runtime-condition skips, schema relaxations, or real-to-mock substitutions detected. | LOW | Added state-backed assertions in `diff.patch:795-872`; added mid-turn notify assertions in `diff.patch:1607-1688`; `env_remove("OULIPOLY_DATA_DIR")` sweep appears as harness isolation additions such as `diff.patch:1007-1693`. | Capture-time persistence and poison-pin immunity claim. | N/A | `runtime-tests.log:2-19` records target runtime integration tests passing; `runtime-tests.log:23-28` records poison spot-proof plus full poisoned and clean workspace summaries. |

No finding records were emitted. Therefore the required per-finding fields `id`, `severity`, `path`, `line_span_or_diff_hunk`, `pattern_id`, `validation_surface_change`, `runtime_fix_claim_ref`, `ratification_ref`, `runtime_artifact_validation_ref`, `closure_expectation`, and `blocks_pipeline` are not applicable.

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | N/A | N/A | N/A |

## Residual ambiguity / stop-condition notes

The Phase 6 contract identifies commit `9e00408` as the production capture-time backfill delta and commit `9ba1275` as test-only `OULIPOLY_DATA_DIR` scrub infrastructure, not production function inventory (`cap.contract.md:3`, `cap.contract.md:39-41`). The proposal proof plan explicitly scopes the validation to state-backed runtime integration, end-to-end CLI integration for the opencode-style mid-turn reproduction, and full-run evidence for the 51-file scrub sweep (`proposal.md:13-27`).

The capture-time sidecar test is not vacuous against the stated claim. It asserts the captured stdout-json session id, exactly one sidecar row, `row.session_id == CAPTURED_SESSION_ID`, `session_runtime` running fields tied to invocation/provider/model/PID identity, and the final idle transition (`diff.patch:795-872`; current source `age_pid_sidecar_spawn.rs:135-213`). The no-spawn-identity negative case asserts the same capture provider reports a session id but the sidecar DB is not created (`diff.patch:875-911`; current source `age_pid_sidecar_spawn.rs:215-251`).

The mid-turn notify test is sequenced before provider exit, not post-exit. The provider body prints the stdout-json capture event, sleeps, runs `notify_handle h-capture-midturn 0`, sleeps again, and only then reaches the provider-script `exit 0` (`diff.patch:1615-1628`; current source `wu_d_proactive_wake_integration.rs:644-649` and `wu_d_proactive_wake_integration.rs:922-947`). The assertions check `status = enqueued`, `owner_session_id = ses_capturemidturn`, `session_source = sidecar_session_id`, `wake.status = busy`, sidecar session-id backfill, resumed prompt delivery, delivered mailbox row metadata, and final idle runtime (`diff.patch:1636-1688`; current source `wu_d_proactive_wake_integration.rs:665-717`).

The test-only `OULIPOLY_DATA_DIR` scrub sweep does not match VI-002 or VI-003 because no runtime-condition skip or ignore was added. It does not match VI-004 or VI-005 because it does not replace a real runtime dependency with a mock or stub; it removes a higher-precedence parent env pin from XDG-isolated harnesses so those harnesses actually use their fixture data homes. The runtime-artifact evidence covers the artifact-bound part of the sweep: target tests pass, the age100 poison-pin spot proof passes, and both poisoned and clean full-workspace summaries are green (`runtime-tests.log:1-28`).

The env scrub also does not mask the production/provider-child `OULIPOLY_DATA_DIR` behavior in this diff. The current source still has a dedicated provider-child preservation check that sets a custom `OULIPOLY_DATA_DIR`, executes the real runtime executor path, asserts the provider observes that custom value, and asserts the XDG-derived default data dir was not created (`age_pid_sidecar_spawn.rs:254-292`). The proactive wake suite also retains a provider-shadow XDG test that requires a pinned runner data dir and asserts the shadow XDG dir does not receive agent-runner state (`wu_d_proactive_wake_integration.rs:799-839`).

No ratification entry was required because no validation-integrity finding fired. There is no `NEEDS_INPUT` ambiguity: the supplied runtime-artifact evidence is readable and non-empty, and it references the runtime artifacts named by the runtime claim.

VERDICT: LOW
LOW
