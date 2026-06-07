# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `pr-diff` | 7 | n/a | Selected diff audit mode. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve repository-relative evidence. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/code-quality/oc/reports/validation-integrity-auditor.md` | n/a | n/a | Only written path. |
| runtime_claim | `Fake-provider + isolated-XDG tests assert real behavior: opencode launch captures ses_ via step_start.sessionID; resume composes --session ses_; opencode_notify_idle_wakes_resume_with_ses_session delivers the wake; opencode JSON error 429->RateLimited / persistent quota->QuotaExhausted; opencode-turns ingests normalized JSONL counted by count_session_turns; 5 fake quota scripts route exhausted accounts away.` | 411 | `8e6257941b30` | Claim is explicitly fake-provider / isolated-XDG scoped. |
| code_quality_convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Required Phase 6 convention read. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/contracts/oc.contract.md` | 21371 | `109bb3a4d88b` | Required Phase 6 contract read before scoring. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/opencode-contract/gap-matrix.md` | 36842 | `04d08fb2eb8d` | Required proposal/proof-intent context read before scoring. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/diff.patch` | 328349 | `10701a405b34` | Unified diff inspected by hunks. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 487336 | `3dd8fe295119` | Read for explicit validation-surface ratification. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/evidence/runtime-tests.log` | 373 | `3970779194b3` | Read for runtime-artifact validation evidence. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/touched-surfaces.md` | 1316 | `80c20f2bf087` | Supplemental touched surface context. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| VI-001 | VI-006 | id=VI-001; severity=LOW after ratification; path=`crates/oulipoly-config/src/model.rs`, `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs`, `crates/oulipoly-runtime/tests/age_164_c5_resume_capture.rs`; line_span_or_diff_hunk=`diff.patch:57-115`, `diff.patch:509-582`, `diff.patch:1478-1537`, `diff.patch:1554-1644`; validation_surface_change=`stdout_json_event` no longer requires the legacy `json_flag` plus `last_message_flag` shape for all providers and now accepts a strict alternative `json_args` shape with no last-message sidecar; runtime_fix_claim_ref=opencode launch captures `ses_` from `step_start.sessionID` using `--format json`; ratification_ref=`DECISIONS.md:3-8` / `D-OC-VI-001-stdout-json-event-dual-shape`; runtime_artifact_validation_ref=`runtime-tests.log:1-4`; closure_expectation=keep the two exclusive shapes strict and rerun the named fake-provider/config tests if this surface changes again; blocks_pipeline=false. | LOW | Removed unconditional `last_message_flag` requirement for `stdout_json_event`, then added non-empty `json_args`, no `json_flag` mixing, and no `last_message_flag` mixing guards; tests assert rejection of stray/empty args and success for OpenCode `--format json` capture. | `opencode launch captures ses_ via step_start.sessionID`. | Ratified and downgraded from MEDIUM to LOW. DECISIONS names the specific validation surface and preserving strict exclusive shapes. | Supplied runtime log is non-empty and names `strict dual-shape capture`; test-result lines are present. |
| VI-002 | VI-006 | id=VI-002; severity=LOW after ratification; path=`crates/oulipoly-state/src/db.rs`, `src-tauri/src/run/resume/validator.rs`, `src-tauri/tests/pr_f_resume_integration.rs`, `src-tauri/tests/wu_d_proactive_wake_integration.rs`; line_span_or_diff_hunk=`diff.patch:1788-1824`, `diff.patch:5338-5370`, `diff.patch:5571-5623`, `diff.patch:5876-5920`; validation_surface_change=resume input validation widened from UUID-only to a strict dual grammar accepting UUIDs or OpenCode provider session IDs matching `ses_` plus an alphanumeric minimum-length suffix; runtime_fix_claim_ref=resume composes `--session ses_` and `opencode_notify_idle_wakes_resume_with_ses_session` delivers the wake; ratification_ref=`DECISIONS.md:10-15` / `D-OC-VI-002-dual-grammar-resume-input`; runtime_artifact_validation_ref=`runtime-tests.log:1-4`; closure_expectation=keep malformed inputs rejected before DB/config initialization and rerun the named resume/wake tests if the grammar changes; blocks_pipeline=false. | LOW | Replaced `Uuid::parse_str` as the only accepted resume input with `Uuid::parse_str(session_id).is_ok() || is_opencode_provider_session_id(session_id)`; tests reject `not-a-uuid`, `ses_ab`, and `ses_fixture-1`, while accepting `ses_fixture`. | `resume composes --session ses_`; `opencode_notify_idle_wakes_resume_with_ses_session delivers the wake`. | Ratified and downgraded from MEDIUM to LOW. DECISIONS names the specific dual-grammar validation surface and malformed-input guard. | Supplied runtime log is non-empty and names `dual-grammar resume`; test-result lines are present. |

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| VI-001 | `D-OC-VI-001-stdout-json-event-dual-shape — strict OpenCode capture support` (`DECISIONS.md:3-8`) | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/evidence/runtime-tests.log` | MEDIUM -> LOW |
| VI-002 | `D-OC-VI-002-dual-grammar-resume-input — strict provider session IDs` (`DECISIONS.md:10-15`) | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/evidence/runtime-tests.log` | MEDIUM -> LOW |

## Residual ambiguity / stop-condition notes

No stop condition fired: the selected diff, Phase 6 contract, proposal, worktree path, decisions file, runtime evidence, and report path were readable/writable enough to complete the audit.

No VI-002/VI-003 skip finding was detected. No added pytest/unittest/runtime-availability skip marker appears in the inspected hunks.

No VI-004/VI-005 finding is emitted for the fake-provider, fake-quota, fake `OPENCODE_BIN`, temp DB, or isolated-XDG harnesses. The proposal explicitly scopes the proof plan to runner-owned behavior under fake-provider and isolated-XDG evidence (`planning/opencode-contract/gap-matrix.md:4934-4944`), and the added tests assert observable argv composition, DB resume resolution, wake delivery, terminal-signal mapping, adapter-script invocation, and generic routing behavior rather than replacing an existing live OpenCode proof surface.

No VI-006 finding is emitted for `crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs`: the current top-level diff only records that the OpenCode phrase list remains intentionally empty until live wording is verified (`planning/oc-gate/gates/diff.patch:722-728`), and the added test asserts guessed `session ... not found` wording does not map to `resume_session_mismatch` (`planning/oc-gate/gates/diff.patch:1717-1749`). That preserves, rather than relaxes, the phrase validation surface.

No VI-007 finding is emitted. The runtime claim and proposal do not claim a live OpenCode binary, production DB, deployed service, container, or production account/auth mapping; they expressly narrow proof to runner-owned fake-provider and isolated-XDG behavior, while recording live OpenCode capture/auth/storage as residual follow-up risk (`planning/opencode-contract/gap-matrix.md:4934-4944`). The supplied runtime-artifact evidence is sufficient only for ratifying the capture and resume broadenings; the P1 terminal/turn/quota tests are not subject to a fired weakening pattern in this diff.

I did not treat the nested added copy of `planning/oc-gate/gates/diff.patch` as active runtime/test code. It is a planning evidence artifact embedded in the PR diff; active validation-surface findings above cite the top-level product and test hunks.

The source-guard exclusions for `planning/*-gate/**` and `planning/opencode-contract/**` were inspected as validation-harness changes, but they are not proof surfaces for the stated runtime claim. They avoid counting planning/report artifacts in provider-name source guards and do not substitute proxy evidence for the OpenCode capture/resume/wake/turn/routing claim.

VERDICT: LOW
