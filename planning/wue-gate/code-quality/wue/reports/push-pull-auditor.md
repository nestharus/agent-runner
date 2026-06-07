# Push/Pull Coupling Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wu-e/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/diff.patch`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/touched-surfaces.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/code-quality/wue/reports/push-pull-auditor.md`
- `mode=phase-6`

## References Read

- `/home/nes/ai/conventions/code-quality.md` lines 21-27, 106-131, 143-149, 169-173, 291-310
- `/home/nes/ai/conventions/agent-questions-and-session-graph.md` lines 230-242
- `planning/wu-e/proposal.md` lines 80-193, 195-278, 319-346, 371-431
- `planning/wue-gate/contracts/wue.contract.md` lines 127-175
- `planning/wu-b/proposal.md` lines 67-83, 113-188
- `planning/wu-d/proposal.md` lines 35-149
- `planning/abd-gate/contracts/abd.contract.md` lines 500-648

A1 preservation check: PASS. The metric source still contains the Push-vs-pull system coupling section, the session-graph Pull-vs-Push Policy disambiguator, the `uncontrolled-source coupler` failure mode, touched-file ownership, and numerical threshold context.

## Pull Sites Inspected

| ID | Puller | Source | Pull mechanism | Ownership/interface evidence | Verdict | Evidence |
|---|---|---|---|---|---|---|
| PP-001 | `crates/oulipoly-runtime/src/executor/cli.rs` CLI facade | Executor CLI sibling component set | Module exposure and public re-exports | LOW source-control proof: the facade and sibling modules are in the same runner-owned crate/component set. | LOW | `crates/oulipoly-runtime/src/executor/cli.rs` lines 64-101; `planning/wue-gate/gates/touched-surfaces.md` lines 3-7 |
| PP-002 | `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | Brokered PTY launch availability and provider interactive launch contract | `pty_broker::controlling_terminal_available`, `execute_interactive_child`, fallback inherited stdio spawn | LOW common-interface/source-control proof: WU-E declares the broker handoff and fallback; provider launch construction remains owned by existing runner interactive flow. | LOW | `crates/oulipoly-runtime/src/executor/cli/interactive.rs` lines 76-114; `planning/wu-e/proposal.md` lines 80-112 |
| PP-003 | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::pty-driver` | Unix kernel PTY, termios, signal, and fd surfaces | `/dev/tty`, `tcgetattr`, `openpty`, `TIOCSCTTY`, `tcsetpgrp`, `poll`, `read`, `write`, `TIOCGWINSZ`, `TIOCSWINSZ`, process-group signals | LOW common-interface proof: WU-E contract declares this component as an intrinsic surface owning the Unix PTY/termios/signal kernel surface; the proposal declares the PTY ownership and relay sequence. | LOW | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` lines 85-112, 264-340, 613-659, 722-783; `planning/wue-gate/contracts/wue.contract.md` lines 155-175; `planning/wu-e/proposal.md` lines 80-193 |
| PP-004 | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control-socket-protocol` | Runner-owned Unix control socket path and binary frame protocol | `UnixStream::connect`, `UnixListener::bind/accept`, fixed 12-byte headers, `OPTY` magic, version/op/status fields, length-bounded payload, `SO_PEERCRED` | LOW common-interface proof: WU-E contract declares the control-socket protocol adapter; WU-E proposal declares path allocation, security, request/response frame format, and ack boundary. | LOW | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` lines 49-64, 144-188, 396-518, 824-955; `planning/wue-gate/contracts/wue.contract.md` lines 127-153; `planning/wu-e/proposal.md` lines 195-278 |
| PP-005 | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | Parent invocation env and PID identity sidecar runtime update | Parse `OULIPOLY_PARENT_INVOCATION` payload into `CompositeInvocationId`, record child identity, map PTY path into `SessionRuntimeRunningUpdate` | LOW source-control/common-interface proof: runner owns the parent invocation env mapping and child spawn identity capture; ABD contract declares `child_spawn_identity_capture`. | LOW | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` lines 57-75, 77-145; `planning/abd-gate/contracts/abd.contract.md` lines 630-637; `planning/wue-gate/contracts/wue.contract.md` lines 92-96 |
| PP-006 | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `ExitStatus`, Unix signal constants, signal-hook stream, child PID/process group target | `status.code`, Unix `status.signal`, `Signals::new`, forwarded `SIGINT`/`SIGTERM`/`SIGHUP`, `kill` | LOW common-interface proof: terminal signal mapping and signal forwarding are declared adapter surfaces; WU-E proposal explicitly extends external signal forwarding to the child process group. | LOW | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` lines 63-78, 118-149, 230-341; `planning/wu-e/proposal.md` lines 173-193; `planning/wue-gate/contracts/wue.contract.md` lines 97-103 |
| PP-007 | `crates/oulipoly-state/src/mailbox.rs` | Runner-owned PID mailbox sidecar DB schema and rows | SQLite `mailbox`, `session_runtime`, and `session_wake_claim` queries/updates, row mappers, liveness checks | LOW source-control proof: `mailbox.rs` owns the sidecar schema and state transitions; ABD contract declares `mailbox_sidecar` ownership. | LOW | `crates/oulipoly-state/src/mailbox.rs` lines 227-285, 331-377, 671-783, 907-974, 1111-1170; `planning/abd-gate/contracts/abd.contract.md` lines 621-629; `planning/wu-b/proposal.md` lines 113-188; `planning/wu-d/proposal.md` lines 35-99 |
| PP-008 | `src-tauri/src/commands/notify.rs` | Agent-bash completion metadata files and rc file | `read_to_string` for `meta.json` and rc path, JSON field extraction for `caller_chain`, accepted PID identity field aliases | LOW common-interface proof: WU-B metadata contract declares the `caller_chain` shape, aliases, rc file read, and extra metadata preservation; prior Phase 6 adapter declaration names the agent-bash async spooler completion contract. | LOW | `src-tauri/src/commands/notify.rs` lines 472-605; `planning/wu-b/proposal.md` lines 67-83; `planning/abd-gate/contracts/abd.contract.md` lines 517-524 |
| PP-009 | `src-tauri/src/commands/notify.rs::attempt_pty_delivery` | Runner-owned `session_runtime`, pending mailbox rows, and PTY control socket | `MailboxDb::session_runtime`, `live_pty_control_path`, `prepare_pty_mailbox_delivery`, `inject_control_envelope`, `mark_delivered` | LOW common-interface/source-control proof: WU-E declares the notify live-PTY sequence and the control socket protocol; mailbox and runtime storage are runner-owned. | LOW | `src-tauri/src/commands/notify.rs` lines 226-430; `planning/wu-e/proposal.md` lines 319-346; `planning/wue-gate/contracts/wue.contract.md` lines 141-147 |
| PP-010 | `src-tauri/src/mailbox_delivery.rs` | Mailbox row storage contract and notification envelope contract | `MailboxDb::list_pending`, batch selection, `MailboxRow` field formatting into `[OULIPOLY NOTIFICATIONS]` envelope | LOW common-interface proof: mailbox row fields and stable payload fields are declared by WU-B; WU-E requires the same envelope renderer and batch cap for PTY delivery. | LOW | `src-tauri/src/mailbox_delivery.rs` lines 22-38, 58-102, 167-248; `planning/wu-b/proposal.md` lines 167-188; `planning/wu-e/proposal.md` lines 306-312 |
| PP-011 | `src-tauri/src/wake_coordinator.rs` | Runner-owned auto-wake env family, sidecar runtime liveness, stale PTY socket cleanup, current executable path | Env reads for `OULIPOLY_AUTO_WAKE*`, `MailboxDb::session_runtime/session_liveness`, broker-owned socket unlink helper, `std::env::current_exe` | LOW source-control/common-interface proof: ABD declares `auto_wake_lifecycle`; WU-D declares runtime liveness and detached wake shape; WU-E declares stale PTY row/socket behavior. | LOW | `src-tauri/src/wake_coordinator.rs` lines 117-131, 261-291, 331-358, 423-499, 523-568; `planning/abd-gate/contracts/abd.contract.md` lines 638-645; `planning/wu-d/proposal.md` lines 83-149; `planning/wu-e/proposal.md` lines 371-431 |
| PP-012 | Diff-touched WU-E validation/source-guard files | Test fixtures, runner CLI JSON, temporary XDG paths, planning exclusion literals | Test helper env, fixture PTYs, temp sidecars, output JSON assertions, source-guard path exclusions | LOW source-control proof for this audit context: these are validation-only pull sites authored and consumed inside the same repo/test boundary; production scoring target remains the supplied touched production component. | LOW | `planning/wue-gate/gates/diff.patch` lines 1397-1549, 1550-1805, 2438-2494, 2495-3154; `planning/wue-gate/gates/touched-surfaces.md` lines 1-11 |

## Uncontrolled-Source Coupler Findings

| ID | Puller | Source | Implicit contract evidence | Missing proof | Decoupling direction | Failure mode |
|---|---|---|---|---|---|---|
| none | none | none | No HIGH pull site found in the touched production component. | none | none | none |

## Residual Ambiguity / Stop-Condition Notes

- No `BLOCKED` condition fired: required diff, proposal, contract, A1 metric source, and output directory were readable.
- No `NEEDS_INPUT` condition fired: ownership/interface proof was available for the cross-system seams called out by the prompt.
- The kernel PTY surface is scored LOW only because `planning/wue-gate/contracts/wue.contract.md` declares `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` as an intrinsic surface for that domain.
- The control socket protocol is scored LOW only because both producer and consumer are runner-owned and WU-E declares the binary protocol and ack boundary.
- No deployment-level private endpoint, private database outside the runner-owned sidecar, cache, or service-topology pull was introduced by the touched production component.

Verdict: LOW

VERDICT: LOW
