# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `pr-diff` | 7 | n/a | Caller-supplied mode. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Required worktree path; used to resolve supplied evidence paths. |
| runtime_claim | `XDG-isolated fixture tests assert real PTY behavior: the broker relays a fixture child on a real pty pair; injection appears as input to the live child only at a line boundary; socket ack marks delivered and failure leaves rows pending with wake-busy preserved; stale sockets are cleaned; fixture_interactive_session_agent_bash_completion_arrives_live proves the live E2E.` | 354 | `4f6e93f24a35` | Inline runtime claim. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Required convention read. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md` | 23131 | `83885665a8dc` | Required Phase 6 contract read before scoring. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wu-e/proposal.md` | 33818 | `970d789f7972` | Required Phase 6 proposal read before scoring. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/diff.patch` | 165397 | `9e28b41e89e9` | PR diff evidence parsed by hunks. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/touched-surfaces.md` | 493 | `dcf5d45922cd` | Context only. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 487336 | `3dd8fe295119` | Read for possible ratification; no fired findings required ratification. |
| existing report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/code-quality/wue/reports/validation-integrity-auditor.md` | 5849 | `01cad2e63606` | Stale prior report was read only to determine update mode, then overwritten. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/code-quality/wue/reports/validation-integrity-auditor.md` | n/a | n/a | Only written path. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | None | No ACR-254 validation-weakening pattern fired. | LOW | n/a | Runtime claim remains supported by real PTY, real Unix-socket, sidecar DB, and compiled-runner fixture tests in the supplied diff. | n/a | No separate runtime-artifact evidence path supplied; not required because no weakening pattern fired and the claim is framed as test proof. |

### Finding Records
| id | severity | path | line_span_or_diff_hunk | pattern_id | validation_surface_change | runtime_fix_claim_ref | ratification_ref | runtime_artifact_validation_ref | closure_expectation | blocks_pipeline |
|---|---|---|---|---|---|---|---|---|---|---|

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | n/a | n/a | n/a |

## Residual ambiguity / stop-condition notes

No required input was missing or unreadable, and the diff was inspectable.

No removed assertions were found in the PTY validation surfaces. The PTY broker unit and integration additions assert concrete runtime behavior: `crates/oulipoly-runtime/tests/wu_e_pty_broker.rs` drives the runtime interactive path under an outer PTY and asserts TTY stdio, winsize, relay IO, exit status, and termios restore; `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` unit tests exercise the real control-frame parser, Unix stream pairs, pipe fds, line-boundary wait, payload write, delimiter write, and unsafe-midline refusal; `src-tauri/tests/wu_e_pty_delivery_integration.rs` exercises notify-side sidecar DB effects, real Unix socket client framing, stale-socket cleanup, and a compiled-runner live E2E under an outer PTY.

The helper tests `helper_runs_no_controlling_terminal_fallback` and `helper_runs_broker_session` return when their harness env var is absent, but their parent tests invoke them under that env var and own the assertions. I did not count those helper entrypoints as runtime-condition skips because the asserted validation surface is in the parent tests, and the helper branch is not the claimed proof surface.

The notify-side ack/failure tests use a local Unix control server to exercise the notify client boundary. I did not count that as VI-004 because the diff does not replace a previously real broker dependency with a fake, the server still validates the real frame shape enough to capture the payload, and the live E2E separately exercises the real broker path through the compiled runner and fixture child.

The added `planning/wu-e/**` exclusions in existing source-guard tests narrow unrelated grep guards over planning artifacts. They do not remove PTY-runtime assertions, do not substitute proxy proof for the PTY claim, and did not fire one of the validation-integrity weakening patterns for this runtime claim.

VERDICT: LOW
