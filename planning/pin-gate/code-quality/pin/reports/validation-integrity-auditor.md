# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `pr-diff` | 7 | `59c7d3a7eb7b` | Selected diff mode. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | 57 | `8dbb15e39ec1` | Used to resolve evidence paths. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/code-quality/pin/reports/validation-integrity-auditor.md` | n/a | n/a | Destination only; created by this audit. |
| runtime_claim | `XDG-isolated tests assert the real pin behavior: default-path resolution prefers OULIPOLY_DATA_DIR over XDG_DATA_HOME and falls back when unset (data_dir_precedence.rs); provider spawn env carries the pin without clobbering a pre-existing pin (age_pid_sidecar_spawn.rs); a child whose env carries a DIFFERENT XDG_DATA_HOME still notifies into the spawning runner's sidecar and the wake fires (wu_d_proactive_wake_integration.rs - the live-bug reproduction); wu_b/wu_e harnesses isolate OULIPOLY_DATA_DIR.` | 506 | `ce10b69acb2a` | Runtime behavior claim under validation-integrity review. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/contracts/pin.contract.md` | 5782 | `6a6de10779ee` | Required Phase 6 contract read before scoring. Declares `paths.rs` and `command_format.rs` validation-relevant adapter/intrinsic surfaces. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/proposal.md` | 4461 | `f61075b98992` | Required Phase 6 proof-intent context read before scoring. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/diff.patch` | 38349 | `bc23c1d17931` | Unified PR diff inspected by hunks. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/touched-surfaces.md` | 1709 | `3c4b32d06d04` | Supplemental touched-surface summary. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/evidence/runtime-tests.log` | 5364 | `af1037591291` | Runtime test log references the named pin tests and reports no ignored tests. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 487336 | `3dd8fe295119` | Read for possible weakening ratification; no ratification needed because no pattern fired. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Required by caller; confirms Phase 6 contract/proposal visibility. |
| evidence source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/crates/oulipoly-state/tests/data_dir_precedence.rs` | 2867 | `5ae230359ec0` | Read for validation-surface line evidence. |
| evidence source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs` | 8862 | `786463b5635e` | Read for validation-surface line evidence. |
| evidence source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/tests/wu_d_proactive_wake_integration.rs` | 29057 | `87931737426a` | Read for validation-surface line evidence. |
| evidence source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/tests/wu_b_mailbox_integration.rs` | 29144 | `527cb09a4be5` | Read for harness isolation line evidence. |
| evidence source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/tests/wu_e_pty_delivery_integration.rs` | 21993 | `14b59d092bf6` | Read for harness isolation line evidence. |
| evidence source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs` | 1783 | `4f05c04fe7be` | Read for runtime spawn-pin line evidence. |
| evidence source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/crates/oulipoly-state/src/paths.rs` | 563 | `76437d3ca5bd` | Read for runtime path-precedence line evidence. |
| evidence source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/crates/oulipoly-runtime/src/quota/marker_verification/tests.rs` | 20519 | `b29a9177a341` | Read for env-precedence line evidence. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | None | No validation-weakening pattern fired. The diff adds assertions and env-isolation cleanup rather than removing assertions, adding skips, relaxing schemas, or replacing a real validation surface with mock-only proof. | LOW | `diff.patch:107-162`, `diff.patch:255-371`, `diff.patch:443-545`, `diff.patch:716-792` | Proposal proof plan lines 9-35 and runtime claim supplied by caller. | Not applicable. | `runtime-tests.log:1-72` names the runtime proof commands/tests and reports `0 ignored` for the pin-relevant test sets. |

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | Not needed. | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/evidence/runtime-tests.log` | None. |

## Residual ambiguity / stop-condition notes

No stop condition fired. `contract_path`, `proposal_path`, `diff_path`, `runtime_claim`, `worktree_path`, and `report_path` were present/readable or writable as required.

No VI-001 removed-assertion finding: the diff scan did not find removed assertion lines. The changed validation hunks add assertions in `crates/oulipoly-state/tests/data_dir_precedence.rs:46-78`, `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs:122-160`, `src-tauri/tests/wu_d_proactive_wake_integration.rs:664-705`, and `crates/oulipoly-runtime/src/quota/marker_verification/tests.rs:291-358`.

No VI-002/VI-003 skip finding: the grep hit for `skips_headless_wake` is a test name in the runtime log/diff, not a skip marker or conditional skip call. The runtime evidence reports `0 ignored` for the pin-relevant test groups at `runtime-tests.log:2-19`, `runtime-tests.log:46-72`.

No VI-004/VI-005 mock/stub replacement finding: the added provider fixture scripts are not a replacement for a previously real dependency in the diff. The validation path still executes `RuntimeExecutorService::execute` in `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs:138-150`, and the live-bug reproduction runs the actual agent binary/notify path with a spawned provider script in `src-tauri/tests/wu_d_proactive_wake_integration.rs:668-705`.

No VI-006 schema-relaxation finding: the diff contains no validation schema, required-field, type, format, or failure-condition relaxation. Production deltas centralize path resolution in `crates/oulipoly-state/src/paths.rs:8-18` and pin provider child env in `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs:48-59`.

No VI-007 proxy-only artifact-bound proof finding: the runtime claim is artifact/runtime-path bound, but runtime-artifact evidence was supplied and is non-empty. It names the new data-dir precedence tests, the sidecar spawn tests, the proactive wake shadow-XDG reproduction, and wu_b/wu_e isolation runs at `runtime-tests.log:1-72`.

Harness env isolation is not weakened. `crates/oulipoly-state/tests/data_dir_precedence.rs:19-28` explicitly sets or removes `OULIPOLY_DATA_DIR` and `XDG_DATA_HOME` under a lock. `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs:24-31` removes ambient `OULIPOLY_DATA_DIR` for the unpinned spawn case, while `age_pid_sidecar_spawn.rs:39-46` sets a deliberate custom pin for the preservation case. `src-tauri/tests/wu_b_mailbox_integration.rs:79-85`, `src-tauri/tests/wu_d_proactive_wake_integration.rs:73-82`, `src-tauri/tests/wu_d_proactive_wake_integration.rs:651-657`, and `src-tauri/tests/wu_e_pty_delivery_integration.rs:90-98` / `135-142` remove ambient `OULIPOLY_DATA_DIR` from harness-launched subprocesses.

Finding records: none. Because no finding fired, there are no `id`, `severity`, `path`, `line_span_or_diff_hunk`, `pattern_id`, `validation_surface_change`, `runtime_fix_claim_ref`, `ratification_ref`, `runtime_artifact_validation_ref`, `closure_expectation`, or `blocks_pipeline` records to emit.

VERDICT: LOW
