# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Required Phase 6 convention read. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | 4096 | n/a | Directory used to resolve evidence paths. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/contracts/abd.contract.md` | 66134 | `30009ef3bd34` | Phase 6 contract readable; declares sidecar, notify, mailbox, resume, and wake surfaces. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wu-b/proposal.md` | 25980 | `dee3e6977911` | Proposal read for mailbox proof intent and runtime claim identity. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/gates/diff.patch` | 228971 | `2d31e96e0a19` | Unified PR diff inspected by hunks. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/gates/touched-surfaces.md` | 1262 | `0418aa14870c` | Supplemental touched-surface context. |
| runtime_claim | The wu_a/wu_b/wu_d tests exercise real sidecar DBs, real notify/resume/wake process flows under XDG isolation, asserting: spawn capture writes a verified row with no state.db schema change; death-safe resolution after caller death + reuse/boot mismatch rejection; idempotent enqueue + ordered drain + mark-delivered-after-success; idle wake delivers, busy-then-turn-end delivers, single-flight, auto-wake cap, loop termination. | 427 | `57e555a1d8dd` | Claim text supplied by caller. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/code-quality/abd/reports/validation-integrity-auditor.md` | n/a | n/a | Only output path written. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | None | No validation-weakening pattern fired. The diff adds new Rust unit/integration validation rather than removing assertions, adding runtime-condition skip markers, relaxing schemas, or replacing real runtime paths with mocks/stubs. | LOW | `diff.patch:447` adds `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs`; `diff.patch:4693`, `diff.patch:4966`, and `diff.patch:5797` add CLI/integration tests that invoke `CARGO_BIN_EXE_oulipoly-agent-runner`, set isolated XDG env vars, and inspect real SQLite sidecar state. | Full supplied runtime claim. | Not needed. | Not supplied; not required because no weakening pattern fired and the diff proof is itself scoped to real local test execution surfaces rather than a deployed artifact claim. |

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | n/a | n/a | n/a |

## Residual ambiguity / stop-condition notes

No stop condition triggered. Required Phase 6 `contract_path` and `proposal_path` were readable before scoring.

Validation-integrity observations:

The diff contains no removed assertions or weaker replacements. Grep over the diff found no removed `assert*` test lines and no added `#[ignore]` or skip marker in the changed validation surfaces.

The test surfaces use real local runtime paths for the claims under review. `src-tauri/tests/wu_b_mailbox_integration.rs` and `src-tauri/tests/wu_d_proactive_wake_integration.rs` spawn the built `oulipoly-agent-runner` test binary through `Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))`, set `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, and `HOME` to temp directories, and assert sidecar DB rows and prompt files after command execution. The WU-D tests also spawn notify/resume chains through provider scripts and wait for delivered mailbox state, wake-claim release, single-flight behavior, auto-wake cap behavior, and loop termination.

The fixture provider scripts are not a validation-surface substitution for the runtime behavior claimed here. They stand in for external AI provider CLIs while the proof claim is about agent-runner sidecar DB persistence, notify ownership resolution, resume prompt delivery, and wake coordination. Those agent-runner paths remain exercised through the production binary and sidecar database APIs.

Platform `cfg` gates are present (`target_os = "linux"` for PID identity tests and `unix` for shell/process integration tests), but they are not a detected validation weakening in this diff: the implementation and fixtures depend on Unix/Linux process identity and shell behavior, and no prior broader validation surface was narrowed.

VERDICT: LOW
