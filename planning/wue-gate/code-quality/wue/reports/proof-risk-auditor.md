# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-3-proposal` | n/a | n/a | Valid mode. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve repository evidence paths. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wu-e/proposal.md` | 33818 | `970d789f7972` | Read successfully. Exact `## Proof plan` is present. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md` | 23131 | `83885665a8dc` | Read before scoring. Declares PTY broker adapters and intrinsic Unix PTY/termios/signal surface. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Read successfully; confirms Phase 6 proposal and contract visibility requirement. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/touched-surfaces.md` | 493 | `dcf5d45922cd` | Read as context for WU-E touched production files. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/code-quality/wue/reports/proof-risk-auditor.md` | n/a | n/a | Written as requested. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| Exact `## Proof plan` section | Yes | `planning/wu-e/proposal.md:489-513` contains the exact section heading and six claim/method/match triples. |
| `Runtime claim` | Yes | Runtime claims are present at `planning/wu-e/proposal.md:491`, `planning/wu-e/proposal.md:495`, `planning/wu-e/proposal.md:499`, `planning/wu-e/proposal.md:503`, `planning/wu-e/proposal.md:507`, and `planning/wu-e/proposal.md:511`. |
| `Proof method` | Yes | Proof methods are present at `planning/wu-e/proposal.md:492`, `planning/wu-e/proposal.md:496`, `planning/wu-e/proposal.md:500`, `planning/wu-e/proposal.md:504`, `planning/wu-e/proposal.md:508`, and `planning/wu-e/proposal.md:512`. |
| `Evidence-class match` | Yes | Evidence-class match statements are present at `planning/wu-e/proposal.md:493`, `planning/wu-e/proposal.md:497`, `planning/wu-e/proposal.md:501`, `planning/wu-e/proposal.md:505`, `planning/wu-e/proposal.md:509`, and `planning/wu-e/proposal.md:513`. |

## Findings
| Finding ID | Severity | Runtime claim | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|
| None | n/a | No PR findings. | No missing proof-plan fields, self-certification, proxy-only runtime proof, or evidence-class mismatch found. | n/a | n/a | See claim-by-claim evidence-class decision below. | no |

## Evidence-class decision

The proposal proof plan is structurally complete. It names runtime behavior, shipped proof methods, and explicit evidence-class matches for relay correctness, line-boundary injection safety, ack/failure mailbox outcomes, stale-socket cleanup, no-controlling-terminal fallback, and live-session E2E delivery.

Runtime-artifact-bound classification: the reviewed claims are runtime-bound rather than documentation-only or static-schema claims. The Phase 6 contract declares the PTY broker as an adapter over Unix kernel PTY/termios/signal and provider TTY contracts at `planning/wue-gate/contracts/wue.contract.md:131-157`, and as an intrinsic Unix PTY/termios/signal surface at `planning/wue-gate/contracts/wue.contract.md:159-179`.

| Runtime claim | Required evidence class | Shipped proof method and evidence-class match | Decision |
|---|---|---|---|
| Brokered relay correctness: TTY stdio, user input, child output, initial winsize, clean-exit raw restore. | Runtime PTY execution under a real controlling terminal. | `crates/oulipoly-runtime/tests/wu_e_pty_broker.rs::broker_child_sees_tty_relays_io_preserves_exit_and_restores_raw_mode` is shipped at `crates/oulipoly-runtime/tests/wu_e_pty_broker.rs:19-49`. It spawns a helper under an outer PTY, calls the production interactive entrypoint at `crates/oulipoly-runtime/tests/wu_e_pty_broker.rs:52-68`, and uses a fixture child that asserts TTY stdio, prints `stty size`, echoes input, and exits 7 at `crates/oulipoly-runtime/tests/wu_e_pty_broker.rs:144-158`. This matches the proposal's runtime-artifact explanation at `planning/wu-e/proposal.md:491-493`. | LOW |
| Injection only at a safe line boundary; otherwise `unsafe_mid_line` and no injection. | Production broker control-request frame-processing seam with real Unix IO. | The proof plan names shipped tests at `planning/wu-e/proposal.md:495-498`. The production live listener calls `process_control_request` through `handle_control_request` at `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs:835-858`; that path validates peer/request, waits for safety, writes payload plus delimiter, and only then returns ack at `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs:861-875`. The shipped tests exercise newline wait and unsafe refusal using real `UnixStream::pair` and pipe FDs at `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs:1057-1085` and `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs:1089-1107`. This is component-level runtime evidence scoped to the exact broker frame-processing seam, not a mock-only proxy for provider semantics. | LOW |
| Socket ack marks the injected mailbox batch delivered; socket failure leaves pending and preserves live PTY wake-busy behavior. | Notify command execution against real sidecar state and Unix socket response surfaces. | The proof plan names shipped integration tests at `planning/wu-e/proposal.md:499-501`. The tests invoke the compiled `oulipoly-agent-runner notify` command at `src-tauri/tests/wu_e_pty_delivery_integration.rs:100-119`, seed real sidecar runtime state at `src-tauri/tests/wu_e_pty_delivery_integration.rs:175-190`, and assert ack-to-delivered/no-wake at `src-tauri/tests/wu_e_pty_delivery_integration.rs:263-293` plus failure-to-pending/wake-busy at `src-tauri/tests/wu_e_pty_delivery_integration.rs:297-317`. The fixture Unix server is scoped to producing protocol ack/error responses; the runtime claim under this row is notify-side state transition at that socket boundary. | LOW |
| Stale PTY socket/runtime state is cleaned so it does not permanently block future wake behavior. | Notify/wake path with sidecar runtime state, PID liveness, and filesystem socket cleanup. | The proof plan names `src-tauri/tests/wu_e_pty_delivery_integration.rs::notify_stale_socket_cleans_runtime_and_does_not_report_busy` at `planning/wu-e/proposal.md:503-505`. The shipped test seeds a dead runtime identity and stale socket path, runs compiled notify, verifies wake is not busy, verifies runtime row is idle, verifies `pty_control_path` is cleared, and verifies the stale socket file is removed at `src-tauri/tests/wu_e_pty_delivery_integration.rs:321-348`. | LOW |
| No controlling terminal falls back to inherited stdio and records no `pty_control_path`. | Runtime launch without `/dev/tty`, plus sidecar runtime-state inspection. | The proof plan names the shipped fallback test at `planning/wu-e/proposal.md:507-509`. The test starts a `setsid()` helper, verifies `/dev/tty` cannot be opened, calls the production interactive entrypoint, and inspects the sidecar row for `mode='pty_interactive'` with `pty_control_path == None` at `crates/oulipoly-runtime/tests/wu_e_interactive_fallback.rs:22-75` and `crates/oulipoly-runtime/tests/wu_e_interactive_fallback.rs:78-113`. | LOW |
| A real live interactive session receives an `agent-bash-complete` notification through the PTY control socket without exiting. | Compiled runner live-session E2E under an outer PTY plus compiled notify invocation and sidecar cleanup inspection. | The proof plan names the shipped E2E at `planning/wu-e/proposal.md:511-513`. The test writes a real fixture model/provider, starts compiled `oulipoly-agent-runner repl --resume` under an outer PTY, waits for the running invocation/identity, runs compiled notify, observes PTY ack, verifies the live fixture child receives `[OULIPOLY NOTIFICATIONS]`, asserts the interactive process is still live at notification receipt, and verifies cleanup at `src-tauri/tests/wu_e_pty_delivery_integration.rs:352-401`. The compiled runner command paths are visible at `src-tauri/tests/wu_e_pty_delivery_integration.rs:121-132`, `src-tauri/tests/wu_e_pty_delivery_integration.rs:451-483`, and `src-tauri/tests/wu_e_pty_delivery_integration.rs:486-514`. | LOW |

Overall evidence-class result: LOW. The proposal does not self-certify, and each proof-plan claim is bound to a shipped test file. Mixed fixture evidence is scoped to the correct runtime boundary: the notify tests fixture only the peer socket response while exercising compiled notify and sidecar mutation, and the broker control-request tests exercise the actual production frame-processing seam rather than a separate mock adapter. The live-session E2E closes the successful end-to-end runtime path with the actual broker socket.

## Residual ambiguity / stop-condition notes

No stop condition fired. Required `mode`, `proposal_path`, `report_path`, `worktree_path`, and Phase 6 `contract_path` were supplied; `proposal_path` and `contract_path` were readable; `report_path` was writable. This audit inspected shipped test files and proposal/contract text read-only; it did not execute tests or create validation evidence.

VERDICT: LOW
