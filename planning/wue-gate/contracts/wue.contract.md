# WU-E Phase-6a Code-Quality Contract

## Component declared roles

Component: WU-E PTY broker delivery touched production component.

Roles: `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator`.

Rationale: the change spans a new Unix PTY/control-socket broker, notify-side live delivery integration, mailbox envelope reuse, signal forwarding, spawn identity state threading, and runtime-state liveness cleanup. This is the honest minimal component union, but per-file cohesion scoring is preferred because the broker, notify integration, and runtime-state surfaces are separate concerns.

## Per-file declared roles

| File | Declared roles | Rationale |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration` | The touched line exposes the Unix broker module through the CLI facade; no production function changed. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `formatter`, `mapper`, `orchestration`, `validator` | Existing interactive entrypoint roles remain; WU-E adds broker selection and spawn-identity mapping. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | New broker owns PTY relay orchestration, control-frame parsing/formatting, terminal/kernel accessors, safety predicates, and protocol/peer validators. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser` | WU-E adds context accessors and PTY control-path mapping while retaining existing identity recording and env parsing roles. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` | WU-E extends signal forwarding target selection while preserving existing terminal status mapping and signal predicates. |
| `crates/oulipoly-state/src/mailbox.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | The touched storage surface is already broad; WU-E threads `pty_control_path` through running/idle/stale runtime row mapping. |
| `src-tauri/src/commands/notify.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `validator` | WU-E adds live PTY delivery orchestration, live socket filtering, and diagnostic mapping to the existing notify command. |
| `src-tauri/src/mailbox_delivery.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate` | WU-E adds PTY preparation and max-size access while reusing existing mailbox batch selection and envelope formatting. |
| `src-tauri/src/wake_coordinator.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | WU-E makes PTY busy checks liveness-aware and unlinks stale sockets while retaining wake orchestration and diagnostics. |

## Function inventory

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs::execute_interactive_with_result_and_model_identity` | `orchestration` | Sequences provider args, spawn identity, broker-vs-inherited-stdio launch, wait, guard lifetime, and result mapping. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs::interactive_command` | `mapper` | Maps provider launch inputs into a `Command` without configuring stdio. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs::interactive_spawn_identity_context` | `mapper` | Maps parent invocation, provider, model, resume session, mode, and cwd into optional spawn identity context. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::inject_control_envelope` | `orchestration` | Sequences client payload validation, Unix socket connection, timeout setup, frame write, and response read. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control_socket_accepts_connection` | `predicate` | Answers whether the socket path accepts a Unix-stream connection. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::unlink_control_socket_if_owned` | `orchestration` | Delegates owned-path selection to a predicate and removes the socket only when that predicate passes. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control_socket_path_is_owned` | `predicate` | Answers whether a candidate socket path is under the runner-owned broker directory. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::controlling_terminal_available` | `predicate` | Answers whether `/dev/tty` can be opened for brokered interactive launch. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::execute_interactive_child` | `orchestration` | Sequences terminal open, PTY allocation, socket bind, child spawn, identity record, signal guard, raw mode, relay, and exit status return. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::validate_client_payload` | `validator` | Accepts non-empty bounded payload bytes or returns client validation failure. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::connect_error` | `mapper` | Maps an `io::Error` into the structured connect client error. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::protocol_error` | `mapper` | Maps an `io::Error` into the structured protocol client error. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::read_control_response` | `parser` | Parses the binary control response frame into `PtyControlResponse` while rejecting malformed fields. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::write_inject_frame` | `formatter` | Formats payload bytes into the declared binary inject request frame. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::PtyPair::open` | `orchestration` | Sequences `openpty` allocation and conversion of returned fds into owned `File`s. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::RealTerminal::open` | `orchestration` | Sequences `/dev/tty` open and original terminal attribute capture. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::RealTerminal::fd` | `accessor` | Exposes the real terminal raw fd without changing its meaning. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::RealTerminal::into_raw_mode` | `orchestration` | Sequences raw termios construction, application, and guard construction. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::RawTerminalGuard::fd` | `accessor` | Exposes the guarded real terminal raw fd. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::RawTerminalGuard::drop` | `orchestration` | Restores original termios on guard drop. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::open_real_terminal` | `accessor` | Retrieves the process controlling terminal as a read-write file. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::terminal_attrs` | `accessor` | Retrieves termios attributes for a raw fd. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::set_terminal_attrs` | `orchestration` | Applies supplied termios attributes through `tcsetattr`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::terminal_winsize` | `accessor` | Retrieves terminal window size through `TIOCGWINSZ`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::set_pty_winsize` | `orchestration` | Applies PTY window size through `TIOCSWINSZ`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::configure_child_pty` | `orchestration` | Sequences slave fd cloning, child stdio assignment, and child `pre_exec` session/ctty setup. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::ControlSocket::bind_for` | `orchestration` | Sequences context/session guards, socket path selection, directory setup, stale socket handling, bind, and permissions. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::ControlSocket::fd` | `accessor` | Exposes the listener raw fd for polling. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::ControlSocket::path_string` | `formatter` | Formats the socket path into a lossy string for sidecar storage. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::ControlSocket::drop` | `orchestration` | Removes the owned socket path when the listener guard drops. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control_socket_dir` | `mapper` | Maps XDG/platform environment state into the broker-owned socket directory path. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::create_private_dir` | `orchestration` | Sequences directory creation and `0700` permission application. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control_socket_location` | `mapper` | Maps session and invocation identifiers into a directory/path pair. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control_socket_location_for_dir` | `mapper` | Maps a candidate directory and identifiers into normal or fallback socket location. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control_socket_path` | `mapper` | Maps directory and identifiers into a bounded Unix socket path. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::short_control_socket_dir` | `mapper` | Maps effective uid into the short fallback broker socket directory. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::short_component` | `formatter` | Formats an identifier into a sanitized bounded socket-name component. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::stable_socket_hash` | `formatter` | Formats session and invocation identifiers into a stable hex socket hash. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::unlink_stale_or_refuse_active` | `orchestration` | Sequences absent-socket predicate, active-socket validator, and stale owned-socket unlink before bind. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control_socket_is_absent` | `predicate` | Answers whether the target control socket path is absent. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control_socket_is_active` | `predicate` | Answers whether the target control socket accepts a Unix-stream connection. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::validate_control_socket_not_active` | `validator` | Rejects an already-active control socket before stale cleanup and bind. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::unlink_owned_socket` | `orchestration` | Removes a socket path only after the owned-directory guard passes. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::SessionRuntimeIdleGuard::new` | `mapper` | Maps optional spawn identity context into idle-guard state. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::SessionRuntimeIdleGuard::drop` | `orchestration` | Opens the default sidecar and marks the guarded session idle on drop. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::relay_until_exit` | `orchestration` | Runs the relay loop by sequencing named window, poll, input, output, control, and child-status helpers. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::poll_relay_fds` | `mapper` | Maps `poll(2)` readiness over real terminal, PTY master, and optional control socket into `ReadyFds`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::readable` | `predicate` | Answers whether poll revents indicate readable or terminal fd state. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::relay_real_input` | `orchestration` | Sequences real-terminal read, input-line state update, and PTY master write. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::relay_pty_output` | `orchestration` | Sequences PTY master read and real-terminal write while treating PTY EOF as success. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::drain_pty_output` | `orchestration` | Loops through poll/read/write helpers to drain remaining PTY output after child exit. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::poll_single_fd` | `predicate` | Answers whether a single fd is readable after a bounded poll. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::read_fd` | `accessor` | Reads bytes from a raw fd into a caller buffer while retrying `EINTR`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::write_all_fd` | `orchestration` | Loops through raw `write` calls until all bytes are written or an error occurs. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::is_pty_eof_error` | `predicate` | Answers whether an `io::Error` is the PTY EOF `EIO` condition. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::maybe_propagate_winsize` | `orchestration` | Sequences window-size read, comparison, PTY resize, child-group signal, and cached-size update. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::winsize_eq` | `predicate` | Answers whether two `winsize` values are identical. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::send_signal_to_child_group` | `orchestration` | Sends a supplied signal to the child process group. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::InputLineState::default` | `mapper` | Constructs the initial line-boundary/debounce state. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::InputLineState::observe_user_input` | `mapper` | Maps observed user bytes into updated line-boundary and debounce state. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::InputLineState::is_safe_to_inject` | `predicate` | Answers whether line boundary and debounce conditions permit injection. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::handle_control_request` | `orchestration` | Sequences accept, timeout setup, request processing, and response write. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::process_control_request` | `orchestration` | Sequences peer validation, request parsing, safety wait, PTY payload write, submit delimiter, and line-state reset. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::read_control_request` | `parser` | Parses the binary inject request frame and payload into validated bytes. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::wait_until_safe_to_inject` | `orchestration` | Runs a bounded relay/wait loop until the named safety predicate passes or fails. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::write_control_response` | `formatter` | Formats ack/error status and message into the declared binary response frame. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::validate_peer_uid` | `validator` | Accepts same-uid Linux peers through `SO_PEERCRED` or accepts all peers on non-Linux Unix. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::SpawnIdentityContext::invocation_uuid` | `accessor` | Exposes the invocation UUID stored in the spawn identity context. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::SpawnIdentityContext::session_id` | `accessor` | Exposes the optional provider session id stored in the spawn identity context. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::SpawnIdentityContext::with_pty_control_path` | `mapper` | Maps an existing spawn identity context into a cloned context with PTY control path attached. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::context_from_parent_invocation_env` | `mapper` | Maps parsed parent invocation state into spawn identity context with explicit empty PTY path. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::session_runtime_running_update` | `mapper` | Maps spawn identity context and process identity into a runtime-running sidecar update including PTY path. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs::InteractiveSignalGuard::install` | `orchestration` | Delegates child-PID target selection into the shared signal-guard install path. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs::InteractiveSignalGuard::install_process_group` | `orchestration` | Delegates process-group target selection into the shared signal-guard install path. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs::InteractiveSignalGuard::install_for_target` | `orchestration` | Sequences signal iterator installation, handle capture, forwarding-thread spawn, and guard construction. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs::spawn_interactive_signal_thread` | `orchestration` | Spawns the forwarding thread with the selected child PID or process-group target. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs::forward_interactive_signals` | `orchestration` | Runs the signal relay loop by sequencing the forwarding predicate and signal sender. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs::should_forward_interactive_signal` | `predicate` | Answers whether a received signal should be forwarded for the selected target type. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs::send_signal` | `orchestration` | Sends the selected signal to either the child PID or the negative process-group id. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::mark_session_running` | `orchestration` | Sequences run-state validation, timestamp capture, turn-start seq calculation, and running-row update. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::mark_session_idle` | `orchestration` | Sequences idle-state validation, timestamp capture, and invocation-guarded idle-row update. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::clear_stale_running_row` | `mapper` | Maps a stale running runtime row into an idle SQL update that clears `pty_control_path`. |
| `crates/oulipoly-state/src/mailbox.rs::mark_session_running_row` | `mapper` | Maps `SessionRuntimeRunningUpdate` into the sidecar `session_runtime` upsert including `pty_control_path`. |
| `crates/oulipoly-state/src/mailbox.rs::mark_session_idle_row` | `mapper` | Maps `SessionRuntimeIdleUpdate` into an invocation-guarded idle SQL update that clears `pty_control_path`. |
| `src-tauri/src/commands/notify.rs::enqueue_owner_notification` | `orchestration` | Sequences payload/enqueue construction, sidecar open, enqueue result handling, and post-enqueue PTY/wake delivery. |
| `src-tauri/src/commands/notify.rs::delivery_and_wake_after_enqueue` | `orchestration` | Sequences PTY delivery attempt and headless wake trigger only when no PTY rows were delivered. |
| `src-tauri/src/commands/notify.rs::attempt_pty_delivery` | `orchestration` | Drives runtime lookup, pending batch preparation, control-socket injection, ack handling, and failure diagnostics; MULTI-CLASSIFIER-RISK: also performs inline status selection, socket filtering, and size validation. |
| `src-tauri/src/commands/notify.rs::live_pty_control_path` | `filter` | Selects a non-empty control path only from running PTY runtime rows. |
| `src-tauri/src/commands/notify.rs::mark_pty_batch_delivered` | `orchestration` | Sequences running-invocation guard, mailbox mark-delivered call, and diagnostic result construction; MULTI-CLASSIFIER-RISK: mixes validation and diagnostic mapping with delivery mutation. |
| `src-tauri/src/commands/notify.rs::pty_client_error_status` | `mapper` | Maps PTY client error kind and message into a notify PTY diagnostic. |
| `src-tauri/src/commands/notify.rs::pending_count` | `accessor` | Retrieves pending mailbox count for diagnostics, degrading storage errors to `None`. |
| `src-tauri/src/commands/notify.rs::pty_status` | `mapper` | Maps status fields into a `PtyDeliveryDiagnostic` DTO. |
| `src-tauri/src/commands/notify.rs::notify_success_response` | `mapper` | Maps `NotifyOutcome` variants into response DTOs including PTY delivery and optional wake diagnostics. |
| `src-tauri/src/commands/notify.rs::render_notify_error_value` | `formatter` | Formats notify error state into the JSON response shape. |
| `src-tauri/src/commands/notify.rs::notify_response` | `mapper` | Maps notify command fields into the serializable response DTO. |
| `src-tauri/src/mailbox_delivery.rs::prepare_pty_mailbox_delivery` | `orchestration` | Sequences pending-row retrieval, empty check, batch selection, seq extraction, envelope render, and PTY delivery preparation. |
| `src-tauri/src/mailbox_delivery.rs::mailbox_prefix_max_bytes` | `accessor` | Exposes the shared mailbox envelope size cap. |
| `src-tauri/src/mailbox_delivery.rs::headless_session_runtime_upsert` | `mapper` | Maps resolved headless resume state into runtime upsert with explicit no PTY control path. |
| `src-tauri/src/wake_coordinator.rs::start_wake_chain` | `orchestration` | Sequences mailbox open, runtime read, live PTY busy check, wake-claim acquisition, detached resume spawn, and diagnostics. |
| `src-tauri/src/wake_coordinator.rs::pty_runtime_is_busy` | `predicate` | Answers whether a PTY runtime is currently busy; MULTI-CLASSIFIER-RISK: also triggers stale liveness cleanup and socket unlink. |
| `src-tauri/src/wake_coordinator.rs::unlink_stale_pty_socket` | `orchestration` | Delegates stale owned socket unlinking to the broker helper when a path is present. |

## Adapter declarations

```yaml
adapter_declarations:
  - component: crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::pty-driver
    role: adapter
    Translates:
      - unix-kernel-pty-termios-signal-contract
      - provider-interactive-tty-stdio-contract
  - component: crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control-socket-protocol
    role: adapter
    Translates:
      - wue-pty-control-inject-protocol-contract
      - unix-stream-socket-peercred-contract
  - component: src-tauri/src/commands/notify.rs::attempt_pty_delivery
    role: adapter
    Translates:
      - mailbox-pending-delivery-contract
      - wue-pty-control-inject-protocol-contract
      - notify-pty-delivery-diagnostic-contract
      - wake-coordinator-notify-wake-contract
  - component: src-tauri/src/mailbox_delivery.rs::render_notification_prefix
    role: adapter
    Translates:
      - mailbox-row-storage-contract
      - oulipoly-notification-envelope-contract
```

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: crates/oulipoly-runtime/src/executor/cli/pty_broker.rs
    role: intrinsic-surface
    Domain: unix_pty_termios_signal_kernel_surface
    Owns:
      - /dev/tty controlling-terminal open
      - openpty PTY pair allocation
      - tcgetattr original termios capture
      - cfmakeraw raw-mode construction
      - tcsetattr raw-mode apply and restore
      - TIOCSCTTY controlling TTY acquisition
      - tcsetpgrp foreground process-group assignment
      - TIOCGWINSZ real-terminal window-size read
      - TIOCSWINSZ PTY window-size write
      - SIGWINCH propagation to child process group
      - setsid child session/process-group creation
      - poll/read/write real-terminal and PTY-master relay
```

Other touched production files: none.
