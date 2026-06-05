# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | pr-diff | 7 | n/a | Selected diff audit mode. |
| worktree_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar | n/a | n/a | Used to resolve repository-relative evidence. |
| report_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/code-quality/oc/reports/validation-integrity-auditor.md | n/a | n/a | Only written path. |
| runtime_claim | Fake-provider + isolated-XDG tests assert real behavior: opencode launch captures ses_ via step_start.sessionID; resume composes --session ses_; opencode_notify_idle_wakes_resume_with_ses_session delivers the wake; opencode JSON error 429->RateLimited / persistent quota->QuotaExhausted; opencode-turns ingests normalized JSONL counted by count_session_turns; 5 fake quota scripts route exhausted accounts away. | 411 | 8e6257941b30 | Claim is explicitly fake-provider / isolated-XDG scoped. |
| code_quality_convention | /home/nes/ai/conventions/code-quality.md | 30798 | fa8b6499cc2e | Required Phase 6 convention read. |
| contract_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/contracts/oc.contract.md | 16525 | c8cac2b917a9 | Required Phase 6 contract read before scoring. |
| proposal_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/opencode-contract/gap-matrix.md | 32975 | 61145931d3a4 | Required proposal/proof-intent context read before scoring. |
| diff_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/diff.patch | 81642 | 0f3998afb1a0 | Unified diff inspected by hunks. |
| touched_surfaces_path | /home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/touched-surfaces.md | 1156 | 6cbceb0ae602 | Supplemental touched surface context. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| VI-001 | VI-006 | id=VI-001; severity=MEDIUM; path=`crates/oulipoly-config/src/model.rs`; line_span_or_diff_hunk=`diff.patch:26-47`, with test expectation changed at `crates/oulipoly-runtime/tests/age_164_c5_resume_capture.rs` / `diff.patch:1219-1245`; validation_surface_change=`stdout_json_event` no longer requires `last_message_flag` and now accepts `json_args` as an alternative to `json_flag`; runtime_fix_claim_ref=opencode launch captures `ses_` from `step_start.sessionID` using `--format json`; ratification_ref=none supplied; runtime_artifact_validation_ref=none supplied; closure_expectation=explicit DECISIONS ratification plus runtime-artifact evidence if this relaxation is intended to stand as validation-surface weakening; blocks_pipeline=true. | MEDIUM | `- session_capture.kind = stdout_json_event requires last_message_flag` replaced by `+ requires json_flag or json_args`; `age230_stdout_json_event_capture_allows_missing_last_message_sidecar` expects success. | Capture support for OpenCode `--format json` without a last-message sidecar. | Unratified. Contract/proposal explain intent, but no `decisions_path` plus `runtime_artifact_evidence_path` pair was supplied for downgrade. | None supplied. |
| VI-002 | VI-006 | id=VI-002; severity=MEDIUM; path=`src-tauri/src/run/resume/validator.rs` and `crates/oulipoly-state/src/db.rs`; line_span_or_diff_hunk=`diff.patch:1479-1497` and `diff.patch:1766-1789`, with integration expectation changed at `src-tauri/tests/pr_f_resume_integration.rs` / `diff.patch:1813-1852`; validation_surface_change=resume input validation widened from UUID-only to any non-empty string, and StateDb removed the pre-resolution UUID gate; runtime_fix_claim_ref=resume composes `--session ses_` and wake resumes `ses_fixture`; ratification_ref=none supplied; runtime_artifact_validation_ref=none supplied; closure_expectation=explicit DECISIONS ratification plus runtime-artifact evidence if accepting non-UUID resume input is the intended validation-surface relaxation; blocks_pipeline=true. | MEDIUM | `Uuid::try_parse(input)` / `Uuid::parse_str(session_id)` removed; replacement rejects only blank input and unknown non-UUID now reaches DB lookup. | Resume/wake path accepts OpenCode `ses_` provider session IDs. | Unratified. Contract/proposal identify this as P0 work, but no ratification/evidence pair was supplied for ACR-254 downgrade. | None supplied. |
| VI-003 | VI-006 | id=VI-003; severity=MEDIUM; path=`crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs`; line_span_or_diff_hunk=`diff.patch:547-575`, with test fixture at `crates/oulipoly-runtime/tests/age_164_c5_resume_capture.rs` / `diff.patch:1423-1450`; validation_surface_change=missing-session predicate broadened from Claude/Codex phrases to OpenCode placeholder phrases, including tokenized `session ... not found`; runtime_fix_claim_ref=resume behavior around OpenCode `ses_` sessions; ratification_ref=none supplied; runtime_artifact_validation_ref=none supplied; closure_expectation=live or otherwise runtime-artifact evidence for the phrase set, or explicit ratification of the placeholder broadening; blocks_pipeline=true. | MEDIUM | Added comments say `OpenCode fixture; verify live phrasing in an isolated sandbox before applying production config`; predicate now accepts several new missing-session strings. | Resume acceptance surface for OpenCode session mismatch/missing-session handling. | Unratified. The diff itself labels the phrases as fixture-backed placeholders. | None supplied. |

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| VI-001 | Not supplied | Not supplied | None |
| VI-002 | Not supplied | Not supplied | None |
| VI-003 | Not supplied | Not supplied | None |

## Residual ambiguity / stop-condition notes

No skip markers, runtime-availability skips, unittest skips, or existing real-dependency-to-mock substitutions were detected in the inspected diff hunks. The fake-provider and isolated-XDG tests are not classified as VI-004/VI-005 because the supplied runtime claim is explicitly scoped to fake-provider evidence, and the tests assert observable argv, DB, mailbox, routing, terminal-signal, and adapter-script effects rather than only tautological helper calls.

No VI-007 finding is emitted: the claim does not assert a live OpenCode production invocation, container, deployed service, or production DB artifact, and the proposal explicitly recommends fake providers with isolated state/config roots as the safe proof environment for P0. The remaining findings are limited to unratified validation-surface broadening in the diff.

VERDICT: MEDIUM
