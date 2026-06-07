# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| operator | `/home/nes/ai/agents/validation-integrity-auditor.md` | 11070 | `6983abb608061d91` | Required operator file read before scoring. |
| mode | `pr-diff` | 7 | N/A | Selected unified diff input surface. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | N/A | N/A | Directory exists and was used to resolve supplied absolute paths. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/gates/diff.patch` | 66052 | `4bd867837ef237f6` | Unified diff read and inspected by hunk. |
| runtime_claim | `LSV makes launch streams volume-safe: a valid launch stream larger than the transport capture limit completes from its exit event instead of failing stdout_limit_exceeded, with bounded host retention honestly recorded; truncation without a valid final exit stays a transport error; non-launch one-shot invocations keep capped stdout semantics; external launch/resume finalization and terminal-error honesty semantics are unchanged for ordinary streams.` | 430 | N/A | Runtime claim is artifact-bound and names launch stream transport behavior, bounded retention, truncation, one-shot caps, and external launch/resume finalization. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/evidence/runtime-tests.log` | 18619 | `9b931c76b76bc65c` | Non-empty runtime evidence log read; includes XDG-isolated `cargo test -p oulipoly-provider`, `cargo test -p oulipoly-runtime`, and `cargo test -p oulipoly-agent-runner --test s10_external_provider_resume`. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 490293 | `ad14421ed08c82ba` | Read for possible ratification; no LSV validation-surface weakening ratification entry found. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/contracts/lsv.contract.md` | 10099 | `04020420c6a5df6a` | Phase 6 contract read before scoring; declares proof surfaces and evidence-class match. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/proposal.md` | 1458 | `a81a136723ff4075` | Proposal context read before scoring; identifies intended LSV runtime behavior and audited range. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/code-quality/lsv/reports/validation-integrity-auditor.md` | N/A | N/A | Destination only; this is the only path written. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | None | No validation-weakening pattern fired. | LOW | Diff adds launch-stream volume regression tests and bounded-retention assertions; no added skips, removed test assertions, mock substitutions, fixture-to-stub replacements, or schema relaxations were detected. | LSV runtime claim. | Not needed. | `runtime-tests.log` includes passing provider, runtime, and external-provider resume runtime test commands, including `launch_accepts_valid_stream_larger_than_transport_capture_limit`, `launch_stdout_truncation_takes_precedence_over_parseable_exit_prefix`, and `external_provider_launch_stream_over_capture_limit_finalizes_succeeded`. |

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | No fired finding required ratification. | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/evidence/runtime-tests.log` | None. |

## Residual ambiguity / stop-condition notes

No stop condition fired. The diff changes runtime launch validation by replacing the old whole-stdout capture-limit rejection with incremental JSONL parsing plus bounded retention. That is the claimed runtime behavior change, not a weakened proof surface: the diff adds positive over-limit launch-stream integration coverage, preserves/uses transport-error coverage for truncation without a valid final exit, and leaves non-launch one-shot capped stdout semantics covered by existing client tests recorded in the runtime evidence log. The fake-provider additions are additive fixture modes exercised as compiled subprocesses, not substitutions for an existing real dependency or hard-coded success path.

Required finding-record fields are not instantiated because there are no findings: `id`, `severity`, `path`, `line_span_or_diff_hunk`, `pattern_id`, `validation_surface_change`, `runtime_fix_claim_ref`, `ratification_ref`, `runtime_artifact_validation_ref`, `closure_expectation`, and `blocks_pipeline` are all not applicable.

LOW
