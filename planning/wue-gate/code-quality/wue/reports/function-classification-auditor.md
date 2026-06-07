# Function Classification Audit

## Inputs Read

| Input | Path / value |
|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| repo_root | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate` |
| wu_id | `wue` |
| mode | `phase-6` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wu-e/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/touched-surfaces.md` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/code-quality/wue/reports/function-classification-auditor.md` |

## References Read

| Reference | Evidence used |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | A1 category list at lines 60-69, single-classification rule at lines 52-58, touched-file ownership at lines 143-149, threshold row at lines 295-300, failure mode at lines 304-310. A1 preservation verified. |
| `planning/wue-gate/contracts/wue.contract.md` | Phase 6 component and per-file roles at lines 3-23, function inventory at lines 25-126, adapter declarations at lines 131-157, intrinsic-surface declaration at lines 159-179. |
| `planning/wu-e/proposal.md` | PTY broker scope and control-socket protocol context at lines 29-33, 80-193, 195-317, notify/wake interplay at lines 319-395. |
| `planning/wue-gate/gates/diff.patch` | Touched-file and changed-function evidence for added/changed production bodies. |
| `planning/wue-gate/gates/touched-surfaces.md` | Production touched-surface list at lines 3-11. |

Scope note: the caller explicitly requested `Single-classification over production functions ADDED/CHANGED`, stated that the substantive surface is `pty_broker.rs`, and stated that other touched files are pre-gated LOW small WU-E edits. Test functions in added test files were excluded. Markdown sections, diff prose, and YAML carriers were excluded from inventory.

## Functions In Touched Files

| Path | Function / symbol | Line span or diff hunk | Inferred category | Verdict | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli.rs` | n/a | diff lines 1-13 | n/a | LOW | Module export only; no production function-like symbol added or changed. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `execute_interactive_with_result_and_model_identity` | lines 76-115 | `orchestration` | LOW | Sequences provider arg preparation, spawn identity construction, broker-vs-inherited launch, wait, and result mapping via named helpers. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `interactive_command` | lines 130-143 | `mapper` | LOW | Maps provider launch inputs into a `Command` by delegating to `build_command`. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `interactive_spawn_identity_context` | lines 156-171 | `mapper` | LOW | Maps parent invocation, provider, model, session, mode, and cwd into optional spawn identity context. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `inject_control_envelope` | lines 49-64 | `orchestration` | LOW | Sequences payload validation, Unix socket connect, timeout setup, frame write, and response read through named helpers. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `control_socket_accepts_connection` | lines 66-68 | `predicate` | LOW | Answers whether a socket path accepts a Unix-stream connection. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `unlink_control_socket_if_owned` | lines 70-73 | `orchestration` | LOW | Delegates owned-path selection to `control_socket_path_is_owned` and performs the unlink action; helper roles are not attributed to the dispatcher. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `control_socket_path_is_owned` | lines 75-77 | `predicate` | LOW | Answers whether a path is under the runner-owned broker directory. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `controlling_terminal_available` | lines 79-81 | `predicate` | LOW | Answers whether `/dev/tty` can be opened. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `execute_interactive_child` | lines 83-110 | `orchestration` | LOW | Sequences terminal open, winsize read, PTY allocation, socket bind, child spawn, identity record, signal guard, raw mode, relay, and exit status. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `validate_client_payload` | lines 112-126 | `validator` | LOW | Accepts bounded non-empty payload bytes or returns structured client validation failures. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `connect_error` | lines 128-133 | `mapper` | LOW | Maps `io::Error` into a connect client error. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `protocol_error` | lines 135-140 | `mapper` | LOW | Maps `io::Error` into a protocol client error. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `read_control_response` | lines 142-177 | `parser` | LOW | Parses binary response header/message into `PtyControlResponse`, rejecting malformed protocol fields as parse failures. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `write_inject_frame` | lines 179-187 | `formatter` | LOW | Formats payload bytes into the binary inject frame. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `PtyPair::open` | lines 195-218 | `orchestration` | LOW | Sequences `openpty`, error handling, and owned `File` construction. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `RealTerminal::open` | lines 226-231 | `orchestration` | LOW | Sequences terminal open and termios capture through named helpers. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `RealTerminal::fd` | lines 233-235 | `accessor` | LOW | Exposes raw fd without changing meaning. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `RealTerminal::into_raw_mode` | lines 237-243 | `orchestration` | LOW | Sequences raw termios construction, apply, and guard construction. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `RawTerminalGuard::fd` | lines 250-253 | `accessor` | LOW | Exposes guarded terminal fd. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `RawTerminalGuard::drop` | lines 256-260 | `orchestration` | LOW | Restores original terminal attrs on guard drop. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `open_real_terminal` | lines 262-264 | `accessor` | LOW | Retrieves the controlling terminal file. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `terminal_attrs` | lines 266-272 | `accessor` | LOW | Retrieves termios attributes for a raw fd. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `set_terminal_attrs` | lines 274-279 | `orchestration` | LOW | Applies supplied termios attributes through `tcsetattr`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `terminal_winsize` | lines 281-287 | `accessor` | LOW | Retrieves terminal window size through ioctl. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `set_pty_winsize` | lines 289-294 | `orchestration` | LOW | Applies PTY window size through ioctl. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `configure_child_pty` | lines 296-338 | `orchestration` | LOW | Sequences slave fd cloning, stdio assignment, and child pre-exec PTY/session setup. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `configure_child_pty::pre_exec closure` | lines 316-336 | `orchestration` | LOW | Sequences child `setsid`, controlling TTY acquisition, foreground process-group assignment, fd cleanup, and `Ok`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `ControlSocket::bind_for` | lines 347-377 | `orchestration` | LOW | Sequences context/session guards, path selection, private dir creation, stale socket handling, bind, chmod, and guard construction. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `ControlSocket::fd` | lines 379-381 | `accessor` | LOW | Exposes listener fd. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `ControlSocket::path_string` | lines 383-385 | `formatter` | LOW | Formats socket path as lossy owned string. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `ControlSocket::drop` | lines 388-392 | `orchestration` | LOW | Delegates owned socket cleanup on guard drop. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `control_socket_dir` | lines 394-404 | `mapper` | LOW | Maps XDG/platform environment state into the broker socket directory path. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `create_private_dir` | lines 406-419 | `orchestration` | LOW | Sequences directory creation and permission application. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `control_socket_location` | lines 421-431 | `mapper` | LOW | Maps session and invocation identifiers into a directory/path pair. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `control_socket_location_for_dir` | lines 433-450 | `mapper` | LOW | Maps candidate dir and identifiers into normal or fallback socket location. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `control_socket_path` | lines 452-466 | `mapper` | LOW | Maps directory and identifiers into bounded socket path. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `short_control_socket_dir` | lines 468-471 | `mapper` | LOW | Maps effective uid into fallback socket directory path. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `short_component` | lines 473-484 | `formatter` | LOW | Formats an identifier into a sanitized bounded socket-name component. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `stable_socket_hash` | lines 486-496 | `formatter` | LOW | Formats session/invocation identifiers into stable hex socket hash. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `stable_socket_hash::map closure` | lines 492-495 | `formatter` | LOW | Formats each digest byte as two hex digits. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `unlink_stale_or_refuse_active` | lines 498-505 | `orchestration` | LOW | Sequences absent-socket predicate, active-socket validator, and stale owned-socket unlink through named helpers; pure orchestrator rule applies. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `control_socket_is_absent` | lines 507-509 | `predicate` | LOW | Answers whether the target socket path is absent. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `control_socket_is_active` | lines 511-513 | `predicate` | LOW | Answers whether the target socket accepts a Unix-stream connection. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `validate_control_socket_not_active` | lines 515-523 | `validator` | LOW | Rejects an already-active control socket before cleanup/bind. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `unlink_owned_socket` | lines 525-529 | `orchestration` | LOW | Applies local ownership guard and removes the socket. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `SessionRuntimeIdleGuard::new` | lines 538-546 | `mapper` | LOW | Maps optional spawn identity context into idle-guard state. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `SessionRuntimeIdleGuard::drop` | lines 549-563 | `orchestration` | LOW | Sequences guard field extraction, sidecar open, and idle update. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `relay_until_exit` | lines 565-615 | `orchestration` | LOW | Relay loop sequences named winsize, poll, relay, control, child-status, and drain helpers; pure orchestrator rule applies. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `poll_relay_fds` | lines 624-667 | `mapper` | LOW | Maps `poll(2)` readiness over real terminal, PTY master, and optional control socket into `ReadyFds`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `readable` | lines 669-671 | `predicate` | LOW | Answers whether poll revents indicate readable/terminal fd state. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `relay_real_input` | lines 673-688 | `orchestration` | LOW | Sequences raw fd read, line-state observation, and PTY write. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `relay_pty_output` | lines 690-698 | `orchestration` | LOW | Sequences PTY read and terminal write while handling EOF. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `drain_pty_output` | lines 700-714 | `orchestration` | LOW | Loops through poll/read/write helpers to drain PTY output. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `poll_single_fd` | lines 716-731 | `predicate` | LOW | Answers whether a single fd is readable after bounded poll. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `read_fd` | lines 733-745 | `accessor` | LOW | Reads bytes from raw fd, retrying EINTR. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `write_all_fd` | lines 747-761 | `orchestration` | LOW | Loops through raw writes until all bytes are written or error occurs. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `is_pty_eof_error` | lines 763-765 | `predicate` | LOW | Answers whether an error is the PTY EOF `EIO` condition. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `maybe_propagate_winsize` | lines 767-783 | `orchestration` | LOW | Sequences window-size read, comparison, PTY resize, signal propagation, and cache update. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `winsize_eq` | lines 785-790 | `predicate` | LOW | Answers whether two `winsize` values are identical. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `send_signal_to_child_group` | lines 792-795 | `orchestration` | LOW | Sends supplied signal to child process group. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `InputLineState::default` | lines 803-810 | `mapper` | LOW | Constructs initial line-boundary/debounce state. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `InputLineState::observe_user_input` | lines 813-824 | `mapper` | LOW | Maps observed user bytes into updated line-boundary/debounce state. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `InputLineState::is_safe_to_inject` | lines 826-832 | `predicate` | LOW | Answers whether line boundary and debounce conditions permit injection. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `handle_control_request` | lines 835-859 | `orchestration` | LOW | Sequences accept, timeout setup, request processing, result-to-response dispatch, and response write. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `process_control_request` | lines 861-876 | `orchestration` | LOW | Sequences peer validation, request parsing, safety wait, PTY payload write, delimiter write, and line-state reset. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `read_control_request` | lines 878-908 | `parser` | LOW | Parses binary inject frame into validated UTF-8 payload bytes. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `wait_until_safe_to_inject` | lines 910-933 | `orchestration` | LOW | Runs bounded relay/wait loop until the named safety predicate passes or fails. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `write_control_response` | lines 935-944 | `formatter` | LOW | Formats ack/error status and message into binary response frame. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `validate_peer_uid` Linux | lines 946-967 | `validator` | LOW | Accepts same-uid Linux peer credentials or rejects credential/mismatch failures. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `validate_peer_uid` non-Linux | lines 969-972 | `validator` | LOW | Accepts peers on non-Linux Unix. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `SpawnIdentityContext::invocation_uuid` | lines 41-44 | `accessor` | LOW | Exposes invocation UUID. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `SpawnIdentityContext::session_id` | lines 46-48 | `accessor` | LOW | Exposes optional session id. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `SpawnIdentityContext::with_pty_control_path` | lines 50-54 | `mapper` | LOW | Maps an existing context into a cloned context with PTY control path. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `context_from_parent_invocation_env` | lines 57-75 | `mapper` | LOW | Maps parsed parent invocation state into spawn identity context with empty PTY path. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `session_runtime_running_update` | lines 128-145 | `mapper` | LOW | Maps spawn identity context and process identity into runtime-running sidecar update. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::install` | lines 230-233 | `orchestration` | LOW | Delegates child-PID target selection into shared install path. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::install_process_group` | lines 235-237 | `orchestration` | LOW | Delegates process-group target selection into shared install path. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::install_for_target` | lines 239-248 | `orchestration` | LOW | Sequences signal iterator installation, handle capture, forwarding-thread spawn, and guard construction. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `spawn_interactive_signal_thread` | lines 274-284 | `orchestration` | LOW | Spawns forwarding thread with selected signal target. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `spawn_interactive_signal_thread::thread closure` | lines 281-283 | `orchestration` | LOW | Delegates signal-forward loop to named helper. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `forward_interactive_signals` | lines 287-297 | `orchestration` | LOW | Sequences signal iteration, forwarding predicate, and send helper. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `should_forward_interactive_signal` | lines 300-314 | `predicate` | LOW | Answers whether a received signal should be forwarded for selected target type. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `send_signal` | lines 332-342 | `orchestration` | LOW | Sends selected signal to child PID or process group. |
| `crates/oulipoly-state/src/mailbox.rs` | `MailboxDb::mark_session_running` | lines 331-339 | `orchestration` | LOW | Sequences run-state validation, timestamp capture, turn-start seq resolution, and SQL update helper. |
| `crates/oulipoly-state/src/mailbox.rs` | `MailboxDb::mark_session_idle` | lines 341-348 | `orchestration` | LOW | Sequences idle-state validation, timestamp capture, and SQL update helper. |
| `crates/oulipoly-state/src/mailbox.rs` | `MailboxDb::clear_stale_running_row` | lines 573-598 | `mapper` | LOW | Maps stale running runtime row into an idle SQL update that clears `pty_control_path`. |
| `crates/oulipoly-state/src/mailbox.rs` | `mark_session_running_row` | lines 671-735 | `mapper` | LOW | Maps `SessionRuntimeRunningUpdate` into sidecar upsert including `pty_control_path`. |
| `crates/oulipoly-state/src/mailbox.rs` | `mark_session_idle_row` | lines 737-765 | `mapper` | LOW | Maps `SessionRuntimeIdleUpdate` into invocation-guarded idle update clearing `pty_control_path`. |
| `src-tauri/src/commands/notify.rs` | `enqueue_owner_notification` | lines 186-224 | `orchestration` | LOW | Sequences payload construction, mailbox open/enqueue, and post-enqueue delivery/wake helper. |
| `src-tauri/src/commands/notify.rs` | `delivery_and_wake_after_enqueue` | lines 226-237 | `orchestration` | LOW | Sequences PTY delivery attempt and wake trigger selection from delivery outcome. |
| `src-tauri/src/commands/notify.rs` | `attempt_pty_delivery` | lines 240-332 | `orchestration` | LOW | Sequences runtime lookup, live path helper, pending batch preparation, size guard, control-socket injection, ack handling, and diagnostic helper dispatch. |
| `src-tauri/src/commands/notify.rs` | `attempt_pty_delivery` non-Unix | lines 334-337 | `mapper` | LOW | Maps non-Unix PTY delivery attempt to `not_pty` diagnostic. |
| `src-tauri/src/commands/notify.rs` | `live_pty_control_path` | lines 339-346 | `filter` | LOW | Selects non-empty control path only from running PTY runtime rows. |
| `src-tauri/src/commands/notify.rs` | `mark_pty_batch_delivered` | lines 348-384 | `orchestration` | LOW | Sequences running-invocation guard, mark-delivered mutation, and diagnostic helper dispatch. |
| `src-tauri/src/commands/notify.rs` | `pty_client_error_status` | lines 386-408 | `mapper` | LOW | Maps PTY client error kind/message into notify PTY diagnostic. |
| `src-tauri/src/commands/notify.rs` | `pending_count` | lines 410-412 | `accessor` | LOW | Retrieves pending mailbox count, degrading storage errors to `None`. |
| `src-tauri/src/commands/notify.rs` | `pty_status` | lines 414-430 | `mapper` | LOW | Maps status fields into `PtyDeliveryDiagnostic` DTO. |
| `src-tauri/src/commands/notify.rs` | `notify_success_response` | lines 794-829 | `mapper` | LOW | Maps `NotifyOutcome` variants into response DTOs. |
| `src-tauri/src/commands/notify.rs` | `render_notify_error_value` | lines 862-884 | `formatter` | LOW | Formats notify error state into JSON response value. |
| `src-tauri/src/commands/notify.rs` | `notify_response` | lines 891-914 | `mapper` | LOW | Maps notify command fields into serializable response DTO. |
| `src-tauri/src/mailbox_delivery.rs` | `prepare_pty_mailbox_delivery` | lines 22-34 | `orchestration` | LOW | Sequences pending-row retrieval, empty check, batch selection, seq extraction, envelope render, and prepared DTO construction. |
| `src-tauri/src/mailbox_delivery.rs` | `mailbox_prefix_max_bytes` | lines 36-38 | `accessor` | LOW | Exposes shared mailbox envelope cap. |
| `src-tauri/src/mailbox_delivery.rs` | `headless_session_runtime_upsert` | lines 147-161 | `mapper` | LOW | Maps resolved headless resume state into runtime upsert with no PTY control path. |
| `src-tauri/src/wake_coordinator.rs` | `start_wake_chain` | lines 261-291 | `orchestration` | LOW | Sequences mailbox open, runtime read, PTY busy check, wake claim acquisition, detached spawn, and diagnostics. |
| `src-tauri/src/wake_coordinator.rs` | `pty_runtime_is_busy` | lines 331-348 | `orchestration` | LOW | Sequences runtime guards, liveness read, stale socket cleanup helper, and busy result. |
| `src-tauri/src/wake_coordinator.rs` | `unlink_stale_pty_socket` Unix | lines 350-355 | `orchestration` | LOW | Delegates stale owned socket unlinking when a path is present. |
| `src-tauri/src/wake_coordinator.rs` | `unlink_stale_pty_socket` non-Unix | lines 357-358 | `orchestration` | LOW | No-op platform branch for stale socket unlinking. |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|
| None | n/a | n/a | n/a | No production added/changed function-like symbol admitted by this pass inferred two or more A1 categories after applying the pure-orchestrator rule. | n/a | n/a | n/a | n/a |

## Residual Ambiguity / Stop-Condition Notes

No stop-condition ambiguity. The required Phase 6 contract and proposal were readable before scoring. A1 preservation was verified before applying the metric. Tests, planning Markdown, and non-production diff carriers were excluded under the caller's production added/changed scope. Other touched production files were treated as pre-gated LOW per caller instruction, with their WU-E changed functions still listed above for boundary evidence.

Verdict: LOW

VERDICT: LOW
