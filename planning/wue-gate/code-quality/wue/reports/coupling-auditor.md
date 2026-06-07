# Coupling Audit

## Inputs Read

| Input | Path | Evidence |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source files resolved from this worktree. |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same as worktree. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate` | Report written under this planning tree. |
| `wu_id` | `wue` | Used for report identity. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wu-e/proposal.md` | Read. PTY broker launch/control/socket/notify design at lines 80-342. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md` | Read. Adapter declarations at lines 127-153; intrinsic-surface declaration at lines 155-175. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/diff.patch` | Read. Production touched hunks include `pty_broker.rs` addition at lines 115-1250 and integration hunks at lines 1-114, 1251-1395, and 1806-2433. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/touched-surfaces.md` | Read. Production touched files listed at lines 3-11. |

## References Read

| Reference | Relevant binding |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | Auditor scope boundary lines 21-27; touched-file ownership lines 143-149; adapter declarations lines 180-210; intrinsic-surface declarations lines 212-253; A1 coupling row lines 291-301. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and no proposer rewrite at lines 29-40. |
| `/home/nes/ai/conventions/risk-profile.md` | Touched-file ownership cross-reference at lines 11-16. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 coupling/cohesion split at lines 403-416; per-component code-quality contract-read requirement and blocking semantics at lines 489-491. |

Metric binding verified: `/home/nes/ai/conventions/code-quality.md` line 300 contains `Coupling by distinct external symbols/modules referenced` with LOW `0-2`, MEDIUM `3-5`, HIGH `>= 6`.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli.rs` | Touched surface list line 3; diff exposes `pub mod pty_broker` at `diff.patch` lines 1-13; source line 75. | CLI facade module; WU-E adds one Unix sibling module edge. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | Touched surface list line 4; WU-E broker branch at source lines 92-97; fallback stdio path at lines 100-114. | Existing interactive orchestrator; WU-E adds broker-vs-inherited-stdio sequencing. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::pty-driver` | Contract adapter declaration lines 131-135; source driver entry and PTY setup at lines 85-111, 191-339, 554-784. | Declared adapter subcomponent inside touched `pty_broker.rs`; translates Unix PTY/termios/signal and provider TTY stdio contracts. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control-socket-protocol` | Contract adapter declaration lines 136-140; source client frame and server frame code at lines 49-63, 181-188, 824-933, 935-961. | Declared adapter subcomponent inside touched `pty_broker.rs`; translates inject protocol and Unix stream peer credential contracts. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | Touched surface list line 5; contract intrinsic declaration lines 159-175; source imports and whole broker body lines 1-1130. | Whole touched file inspected. Intrinsic declaration covers the dense Unix PTY/termios/signal kernel surface; non-kernel edges are scored as raw component pairs or declared adapter subcomponents. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | Touched surface list line 6; source context accessors and PTY control-path mapping at lines 30-55 and 128-145. | WU-E adds `pty_control_path` state threading. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | Touched surface list line 7; source process-group signal target at lines 223-342. | WU-E adds process-group forwarding target while retaining existing terminal-signal mapping. |
| `crates/oulipoly-state/src/mailbox.rs` | Touched surface list line 8; source runtime row structs at lines 67-120; SQL mapping/clearing at lines 573-598 and 671-765. | Sidecar mailbox/runtime storage; WU-E uses existing `pty_control_path` column and clears it on idle/stale cleanup. |
| `src-tauri/src/commands/notify.rs::attempt_pty_delivery` | Contract adapter declaration lines 141-147; source function at lines 239-332. | Declared adapter subcomponent for mailbox pending delivery, PTY inject protocol, diagnostic DTO, and wake behavior. |
| `src-tauri/src/commands/notify.rs` | Touched surface list line 9; source delivery orchestration at lines 186-237 and response mapping at lines 785-914. | Whole notify command inspected; WU-E adds live PTY attempt before wake. |
| `src-tauri/src/mailbox_delivery.rs::render_notification_prefix` | Contract adapter declaration lines 148-152; source formatter at lines 222-249. | Declared adapter for mailbox-row storage to notification envelope rendering. |
| `src-tauri/src/mailbox_delivery.rs` | Touched surface list line 10; source PTY preparation at lines 17-38 and shared batch/render functions at lines 58-249. | Shared mailbox delivery envelope/batch selection surface. |
| `src-tauri/src/wake_coordinator.rs` | Touched surface list line 11; source PTY liveness-aware busy predicate at lines 261-358. | WU-E makes headless wake predicate liveness-aware and delegates stale socket unlink. |

## Per-Pair Coupling

| Source component | Target component | Distinct external symbols/modules referenced | Adapter declaration artifact path | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic declaration artifact path | Declared intrinsic component | `Domain:` | `Owns:` set or summary | Domain count | Intrinsic-surface verdict | Final verdict | blocking_or_residual | Evidence |
|---|---:|---|---|---|---|---:|---|---|---|---|---|---:|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli.rs` | `executor/cli/pty_broker` module | 1 module | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | `cli.rs` line 75 exposes exactly one new Unix module; `diff.patch` lines 1-13. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `executor/cli/pty_broker` | 2 symbols: `controlling_terminal_available`, `execute_interactive_child` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Import at `interactive.rs` lines 32-33; calls at lines 92-97. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `executor/cli/spawn_identity` | 1 module surface: spawn-identity context/recording | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | `interactive.rs` imports the spawn identity surface at lines 35-38 and sequences context/recording at lines 85-107; `spawn_identity.rs` owns that mapping at lines 30-55 and 128-145. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `executor/cli/terminal_signal` | 1 module surface: interactive signal guard/result mapping | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | `interactive.rs` imports `terminal_signal` at line 39 and uses its guard/result functions at lines 105-114 and 193-209. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::pty-driver` | PTY/termios/kernel + provider TTY contracts | raw references exceed 6, but declared adapter counts contracts | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md` | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::pty-driver` | `unix-kernel-pty-termios-signal-contract`; `provider-interactive-tty-stdio-contract` | 2 | declared adapter LOW | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md` | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `unix_pty_termios_signal_kernel_surface` | `/dev/tty`, `openpty`, `tcgetattr`, `cfmakeraw`, `tcsetattr`, `TIOCSCTTY`, `tcsetpgrp`, `TIOCGWINSZ`, `TIOCSWINSZ`, `SIGWINCH`, `setsid`, `poll/read/write` relay | 1 | declared intrinsic-surface LOW | LOW | blocking | Contract adapter at lines 131-135 and intrinsic at lines 159-175. Source PTY/kernel references are subordinate: `/dev/tty` at `pty_broker.rs` lines 264-265; `openpty` at lines 197-218; termios raw/restore at lines 239-261 and 268-280; `TIOCGWINSZ`/`TIOCSWINSZ` at lines 283-296; `setsid`, `TIOCSCTTY`, `tcsetpgrp` at lines 318-339; relay `poll/read/write` at lines 613-784. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control-socket-protocol` | Control inject protocol + Unix stream socket peer credential contracts | raw references exceed 6, but declared adapter counts contracts | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md` | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control-socket-protocol` | `wue-pty-control-inject-protocol-contract`; `unix-stream-socket-peercred-contract` | 2 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Contract adapter at lines 136-140. Source protocol references are subordinate: client connect/frame at `pty_broker.rs` lines 49-63 and 181-188; server accept/process/response at lines 824-933; Linux `SO_PEERCRED` validation at lines 935-956. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `executor/cli/spawn_identity` | 2 symbols: `SpawnIdentityContext`, `record_child_identity` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | `pty_broker.rs` import at line 3; use at lines 85-103 and 349-356. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `executor/cli/terminal_signal` | 2 symbols: `InteractiveSignalGuard`, `exit_code_from_status` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | `pty_broker.rs` import at line 4; process-group guard and exit-code mapping at lines 106-110. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `oulipoly-state` mailbox runtime sidecar | 2 symbols: `MailboxDb`, `SessionRuntimeIdleUpdate` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | `pty_broker.rs` import at line 6; idle guard opens sidecar and marks idle at lines 520-551. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `oulipoly-state` mailbox runtime update | 2 symbols: `MailboxDb`, `SessionRuntimeRunningUpdate` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Import at `spawn_identity.rs` line 11; runtime update mapping including `pty_control_path` at lines 111-145. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | Unix signal forwarding contract | 2 module/symbol surfaces: `signal_hook`, `libc::kill` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | `terminal_signal.rs` signal-hook imports at lines 45-48; install/forward/send path at lines 223-342. |
| `crates/oulipoly-state/src/mailbox.rs` | PID identity sidecar/process identity | 1 module surface: `crate::pid_identity` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Import at `mailbox.rs` line 15; identity/liveness checks at lines 356-377 and 805-813. |
| `crates/oulipoly-state/src/mailbox.rs` | SQLite/rusqlite storage contract | 1 module surface: `rusqlite` SQL execution/mapping | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Import at `mailbox.rs` line 11; WU-E affected runtime update SQL at lines 573-598 and 671-765. |
| `src-tauri/src/commands/notify.rs::attempt_pty_delivery` | Mailbox pending delivery + PTY inject + diagnostics + wake contracts | raw references are subordinate to declared contracts | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md` | `src-tauri/src/commands/notify.rs::attempt_pty_delivery` | `mailbox-pending-delivery-contract`; `wue-pty-control-inject-protocol-contract`; `notify-pty-delivery-diagnostic-contract`; `wake-coordinator-notify-wake-contract` | 4 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Contract adapter at lines 141-147. Source is subordinate: runtime lookup/filter at `notify.rs` lines 239-274; pending batch preparation at lines 275-308; injection/ack/error mapping at lines 309-332; wake sequencing at lines 226-237. |
| `src-tauri/src/commands/notify.rs` | `mailbox_delivery` module | 2 symbols: `prepare_pty_mailbox_delivery`, `mailbox_prefix_max_bytes` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Calls at `notify.rs` lines 275-308; definitions at `mailbox_delivery.rs` lines 22-38. |
| `src-tauri/src/commands/notify.rs` | `wake_coordinator` module | 1 symbol: `trigger_notify_wake` plus `WakeDiagnostic` DTO | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Import at `notify.rs` line 20; wake call at lines 226-237. |
| `src-tauri/src/mailbox_delivery.rs::render_notification_prefix` | Mailbox row storage + notification envelope contracts | raw row field references are subordinate to declared contracts | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md` | `src-tauri/src/mailbox_delivery.rs::render_notification_prefix` | `mailbox-row-storage-contract`; `oulipoly-notification-envelope-contract` | 2 | declared adapter LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Contract adapter at lines 148-152. Source renders only `MailboxRow` fields into the envelope at `mailbox_delivery.rs` lines 222-249. |
| `src-tauri/src/mailbox_delivery.rs` | `oulipoly-state` mailbox/resume storage | 2 symbols/modules: `MailboxDb`/`MailboxRow` and `ResolvedResume` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Imports at `mailbox_delivery.rs` lines 5-6; PTY/headless preparation at lines 22-52 and runtime upsert at lines 137-161. |
| `src-tauri/src/wake_coordinator.rs` | `oulipoly-state` mailbox runtime/liveness/wake claim surface | 1 module surface: mailbox runtime + wake claim APIs | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Import at `wake_coordinator.rs` lines 6-9; liveness-aware busy check at lines 261-358; wake claim flow at lines 360-421. |
| `src-tauri/src/wake_coordinator.rs` | `executor/cli/pty_broker` stale socket helper | 1 symbol: `unlink_control_socket_if_owned` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Import at `wake_coordinator.rs` lines 15-16; delegated unlink at lines 350-358. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| n/a | n/a | n/a | No MEDIUM or HIGH per-pair score found. | The worst applicable per-pair verdict is LOW. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired.

The Step 6a contract declarations are well-formed and resolve to touched boundaries: adapter declarations at `wue.contract.md` lines 127-153 name components inside `pty_broker.rs`, `notify.rs`, and `mailbox_delivery.rs`; the intrinsic declaration at lines 155-175 names touched `pty_broker.rs`, has exactly one `Domain:`, and has a non-empty `Owns:` list.

Standard library support modules were treated as implementation primitives unless they formed the declared Unix kernel, Unix stream socket, or storage/protocol component pair being scored. The blocking scope remains the whole touched production files/components from `touched-surfaces.md` lines 3-11.

VERDICT: LOW
