# WU-E Proposal: PTY Broker Delivery for Live Interactive Sessions

## Scope

WU-E closes the remaining async-bash notification gap for live interactive
sessions:

```text
headless = queue + deliver at next successful resume        (already done)
PTY      = queue + forward whenever into the live session   (WU-E)
```

The implementation target is Unix/Linux v1. It must not change the versioned
`state.db` schema. All new durable or semi-durable state stays in the existing
PID identity sidecar DB and runtime socket files under XDG-isolated runtime or
state directories.

The current interactive path is
`crates/oulipoly-runtime/src/executor/cli/interactive.rs::execute_interactive_with_result_and_model_identity`.
It builds the provider command, sets `stdin/stdout/stderr` to
`Stdio::inherit()`, spawns the child, records the child PID identity, installs
the interactive signal guard, and waits. Because agent-runner keeps no writable
terminal handle, `src-tauri/src/commands/notify.rs` can only enqueue and then
ask `src-tauri/src/wake_coordinator.rs` to wake a headless resume. The wake
coordinator already returns `busy` for `session_runtime.mode='pty_interactive'`,
so queued mail for a live PTY session is not headless-woken, but it is also not
forwarded.

WU-E replaces inherited stdio for interactive launches with an agent-runner PTY
broker. The provider child still sees a real TTY. The user still sees an
interactive CLI. Agent-runner additionally owns the PTY master and a per-session
control socket, so a later `agents notify agent-bash-complete ...` process can
inject a rendered notification turn into the live child.

## Prior Art and Reuse Recommendation

The requested in-house prior-art repositories were checked first under
`/home/nes/projects`; no local checkout exists for `nestharus/cli-proxy` or
`nestharus/claude-cli-proxy`.

The upstream GitHub repositories are public but currently empty from this
environment:

| Repo | Observed state | Description exposed by GitHub |
|---|---|---|
| `nestharus/cli-proxy` | empty clone, GitHub API `size: 0` | `Generic CLI proxy substrate (PTY driver, sentinel-counter framework, completion-detection traits)` |
| `nestharus/claude-cli-proxy` | empty clone, GitHub API `size: 0` | `Claude-specific CLI proxy built on cli-proxy. Hook-driven completion detection for interactive Claude Code sessions.` |

The `cli-proxy` crate name on crates.io appears unrelated to this substrate and
is not attributable to `nestharus` or to the PTY-driver description.

Recommendation: implement a minimal in-tree broker in `oulipoly-runtime` now,
borrowing only the described architectural split: generic PTY driver substrate,
provider-specific completion detection outside the driver, and a control-plane
injection seam. Do not depend on or vendor the prior-art repos for WU-E because
there is no code to review, license, test, or version. If those repos are later
populated, compare their driver patterns against the in-tree broker and extract
only after the WU-E tests prove the local contract.

## Integration Points

Primary runtime changes:

| File | Change |
|---|---|
| `crates/oulipoly-runtime/src/executor/cli.rs` | Add a Unix-only `pty_broker` sibling module and expose only the small pieces needed by tests or by the notify-side control client. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | On Unix with a usable controlling terminal, call the PTY broker instead of configuring inherited stdio. Preserve the public entrypoint and `InteractiveExecutionResult` shape. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | Add a PTY-aware signal target that forwards external process signals to the child process group/session, not only to the child PID. Preserve terminal signal classification from `ExitStatus`. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | Carry the PTY control socket path into `SessionRuntimeRunningUpdate`, or perform an immediately-following guarded runtime update after child identity capture. |
| `crates/oulipoly-state/src/mailbox.rs` | No schema change. Use the existing `session_runtime.pty_control_path` column, clear it on invocation-guarded idle/stale cleanup, and make liveness helpers clear stale socket paths. |

Primary Tauri/CLI changes:

| File | Change |
|---|---|
| `src-tauri/src/mailbox_delivery.rs` | Factor the existing pending-row batch selection and notification rendering so both headless resume and PTY injection use one envelope contract and one batch cap. |
| `src-tauri/src/commands/notify.rs` | After enqueue or already-enqueued, attempt live PTY injection for the owner session before triggering a headless wake. Mark delivered only after socket ack. |
| `src-tauri/src/wake_coordinator.rs` | Keep the no-headless-wake behavior for a live PTY session, but make the predicate live-aware so an exited/stale PTY row with no socket does not block future headless wake. |

## PTY Ownership and Relay

### Launch Shape

For Unix interactive launches, `interactive.rs` delegates to a new
`pty_broker::execute_interactive_child` helper after building the provider
`Command` and before result mapping.

The broker does this sequence:

1. Resolve the real user terminal by opening `/dev/tty` read-write. If no
   controlling terminal exists, v1 preserves current behavior by falling back to
   the existing inherited-stdio path with no `pty_control_path` and therefore no
   live injection. This avoids breaking non-terminal scripts while making normal
   interactive use brokered.
2. Read the current terminal window size with `TIOCGWINSZ` from the real
   terminal.
3. Allocate a PTY pair with `openpty`, passing the initial `winsize`.
4. Allocate and bind the per-session Unix control socket before spawning the
   child, so the path is ready as soon as runtime state is recorded.
5. Spawn the provider child with the PTY slave as fd `0`, `1`, and `2`.
6. In the child `pre_exec` hook, call `setsid()`, acquire the slave as the
   controlling TTY with `ioctl(TIOCSCTTY)`, make the child process group/session
   the foreground terminal process group, and close inherited broker-only fds.
7. In the parent, close the slave, keep the master, record child PID identity,
   record `session_runtime(mode='pty_interactive', run_state='running',
   pty_control_path=<socket>)`, enter raw mode on the real terminal, and start
   the relay loop.

The child should observe `isatty(0)`, `isatty(1)`, and `isatty(2)` exactly as it
does when launched directly from a terminal. Provider launch arguments, working
directory, resume arguments, policy transforms, and result mapping remain owned
by the existing `interactive.rs` flow.

### Raw Mode and Restore

The broker owns a `RealTerminalGuard`:

```text
open /dev/tty -> tcgetattr -> cfmakeraw/tcsetattr -> relay -> Drop restores original termios
```

The guard is created only after the PTY child is spawned and before the relay
starts. It restores with `tcsetattr(TCSANOW)` on every normal return and during
Rust unwinding. The interactive entrypoint should wrap the broker run in a
small `catch_unwind` or equivalent guard boundary only if needed to ensure the
guard is dropped before any panic continues outward. `SIGKILL` and process
abort cannot be restored by process-local code; tests cover normal exit,
non-zero exit, and panic/error paths.

Raw mode is applied to the real terminal, not to the child PTY. Keyboard bytes
are relayed to the PTY master. The child-side slave line discipline remains the
provider's terminal, so control characters like `Ctrl-C` written to the master
are interpreted by the slave TTY and delivered to the child foreground process
group like a normal terminal.

### Relay Loop

V1 uses one simple `poll(2)` loop rather than a full async runtime:

| FD | Direction | Action |
|---|---|---|
| real terminal input | user -> broker | read bytes, update input-line state, write all to PTY master |
| PTY master | child -> broker | read bytes, write all to real terminal output, flush |
| control listener | notify -> broker | accept one local connection, validate peer, read one frame, maybe inject |
| accepted control sockets | notify -> broker | process bounded request synchronously, then close |

The loop also polls child exit with `try_wait()` at a short interval, for
example 20-50 ms. After child exit, it drains remaining PTY output until EOF or
`EIO`, closes the listener, removes the socket, restores the real terminal, and
returns the child's `ExitStatus` to `interactive_result_from_status`.

Flow/perf rules:

- Use fixed-size buffers, for example 8 KiB or 16 KiB.
- Use `write_all` loops to avoid partial relay corruption.
- Treat `EINTR` as retry and `EIO` on PTY master after child exit as EOF.
- Do not parse provider output in the broker v1; provider-specific completion
  detection remains out of scope.
- Keep control requests bounded by the same 64 KiB rendered-prefix cap used by
  headless delivery.

### Window Size Propagation

Initial size is copied from the real terminal to the PTY at allocation. The
broker installs a `SIGWINCH` flag or signal-hook channel. When resize is seen,
it reads `TIOCGWINSZ` from the real terminal, writes `TIOCSWINSZ` to the PTY
master, and sends `SIGWINCH` to the child process group.

This must be process-group based because the child is a session leader after
`setsid`, and interactive providers commonly spawn subprocesses that should see
the resize too.

### Signal Forwarding

The existing `terminal_signal.rs::InteractiveSignalGuard` currently installs
handlers for `SIGINT`, `SIGTERM`, and `SIGHUP`, but the current forwarding logic
only sends the first `SIGTERM` to the child PID. That was acceptable while the
terminal was inherited, because keyboard-generated terminal signals reached the
child directly from the user's terminal.

With a brokered PTY, user keyboard `Ctrl-C` flows as byte `0x03` through the
PTY master and is converted by the child slave TTY into `SIGINT` for the child
foreground process group. External signals delivered to the runner process do
not flow through the PTY, so the guard should be extended for the brokered path:

- Forward external `SIGTERM`, `SIGHUP`, and `SIGINT` to `-child_pid`, the child
  process group created by `setsid()`.
- Keep the current single-forward guard for `SIGTERM` to avoid repeated
  termination spam.
- Restore the handler and join the signal thread before returning the final
  status, as the current guard already does.
- Preserve `classify_terminal_reason`, `exit_code_from_status`, and provider
  recognizer evidence from the real child `ExitStatus`.

## Control Socket and Injection Protocol

### Path Allocation

Socket directory selection:

1. Prefer `$XDG_RUNTIME_DIR/oulipoly-agent-runner/pty/`.
2. If no runtime dir exists, use `$XDG_STATE_HOME/oulipoly-agent-runner/runtime/pty/`.
3. If `XDG_STATE_HOME` is absent, fall back to the platform state/data directory
   already used by the sidecar, under `oulipoly-agent-runner/runtime/pty/`.

Tests must set `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`, `XDG_DATA_HOME`,
`XDG_CONFIG_HOME`, and `HOME` to temp directories. The socket directory must be
created with mode `0700`. The socket basename should include the provider
session id and running invocation UUID, or a stable hash of them if the full
path would exceed Unix `sun_path` limits. Example:

```text
$XDG_RUNTIME_DIR/oulipoly-agent-runner/pty/<session-short>.<invocation-short>.sock
```

The full path is recorded in `session_runtime.pty_control_path` when the child
is marked running and is cleared at invocation-guarded exit cleanup.

Stale path handling at launch:

- If the intended path exists, attempt a short connect/ping.
- If ping succeeds, treat another broker as active and refuse a second live PTY
  broker for that same session/invocation.
- If ping fails with `ENOENT`, `ECONNREFUSED`, timeout, or bad protocol, unlink
  the stale socket and bind a fresh listener.
- Never unlink a path outside the owned `0700` broker directory.

### Security

The socket is same-user control-plane state. V1 protects it with:

- `0700` parent directory.
- `umask(077)` during bind or post-bind chmod where supported.
- Linux `SO_PEERCRED` check on every accepted connection; the peer uid must
  equal the broker process uid.
- Request max length of 64 KiB.
- No shell interpretation. The broker writes exactly the already-rendered
  envelope bytes plus the final line delimiter to the PTY master.

Same-uid processes are not fully isolated from each other on Unix; a malicious
same-user process that can read the sidecar path could still attempt injection.
That is acceptable for v1 and should be documented. A future hardening pass can
add a random per-broker nonce stored only in memory and passed to the notify
process through a protected sidecar field if needed.

### Frame Format

Use one request per Unix-stream connection. The protocol is intentionally small
and binary so it can carry arbitrary UTF-8 notification bytes without escaping.

Request:

```text
magic       4 bytes   "OPTY"
version     1 byte    1
op          1 byte    1 = inject
flags       2 bytes   big-endian, v1 must be 0
length      4 bytes   big-endian, 0 < length <= 65536
payload     N bytes   rendered notification envelope, UTF-8 text
```

Response:

```text
magic       4 bytes   "OPTY"
version     1 byte    1
status      1 byte    0 = ack, 1 = err
reserved    2 bytes   0
length      4 bytes   big-endian, length of UTF-8 message
message     N bytes   "ok" or a short error reason
```

The broker sends `ack` only after the envelope bytes and one trailing newline
or carriage return have been written successfully to the PTY master. The ack
does not prove the provider semantically processed the turn; it proves delivery
to the live terminal input queue. That matches the WU-E delivery boundary and
is the point where `notify` can mark mailbox rows delivered.

### Injection Safety

The broker tracks an approximate input-line state from bytes relayed from the
real terminal to the PTY master:

- `at_line_boundary = true` at broker start.
- Printable input, paste bytes, or other non-newline text set it to `false`.
- `\r`, `\n`, `Ctrl-C`, `Ctrl-D`, or `Ctrl-U` reset it to `true`.
- Any user keystroke starts a short debounce window, for example 250 ms.

V1 injection rule:

```text
inject only when at_line_boundary == true and no user input arrived during the debounce window
```

If the rule is not satisfied, the broker waits up to a small bounded timeout,
for example 1500 ms. If the session becomes safe, it injects and acks. If it
does not, it returns `err unsafe_mid_line`; `notify` leaves rows pending.

This is deliberately conservative. It prevents appending a notification into a
half-typed user command or prompt. It does not solve every terminal UI case:
the provider may be mid-render, in an alternate-screen editor, or in a custom
textarea that is visually at a prompt while the broker cannot know that. Better
provider-aware readiness detection, prompt sentinels, bracketed-paste handling,
and pending-in-broker queues are deferred.

Envelope transport v1 should reuse the headless notification content but render
it through one shared function with explicit PTY suitability constraints:

- No inline log content.
- Paths quoted and newline-sanitized as today.
- Batch cap: 20 rows or 64 KiB, same as headless.
- Append exactly one final submit delimiter after the envelope.

If provider CLIs prove that multiline injected envelopes submit too early,
the next iteration should add a PTY-specific single-line renderer or a
provider capability flag for bracketed paste. Do not add provider-specific
terminal heuristics to the generic broker in v1.

## Notify Integration: Forward Whenever

`notify` remains durability-first. The ordering is:

1. Parse metadata and resolve owner exactly as WU-B does.
2. Enqueue or return the already-enqueued row using the existing mailbox
   idempotency contract.
3. If enqueue conflicts with another session, return the existing conflict and
   do not inject.
4. For inserted or already-enqueued rows, read `session_runtime` for the owner
   session.
5. If runtime says live `pty_interactive` with a usable `pty_control_path`, list
   pending rows for the session, select one ordered batch with the shared
   mailbox batch cap, render the notification envelope, send an `inject` frame
   to the socket, and wait for ack/err.
6. On ack, mark every injected seq delivered with
   `delivered_by_invocation_uuid = session_runtime.running_invocation_uuid`.
   Do not trigger a headless wake for those delivered rows.
7. On any failure, including missing path, dead socket, refused connection,
   peer/protocol error, unsafe mid-line, oversized frame, or mark-delivered
   failure, leave rows pending.
8. If no rows were delivered, call `wake_coordinator::trigger_notify_wake` as
   today. A truly live PTY session returns `busy`. A stale/exited PTY row should
   be cleaned by liveness and may allow a headless wake.

The inject batch must be selected from all pending rows, not only the newly
enqueued row. This preserves per-session `seq ASC` ordering and avoids marking
a later completion delivered before older pending mail.

Recommended diagnostics in `NotifyResponse`:

```json
{
  "status": "enqueued",
  "seq": 42,
  "pty_delivery": {
    "attempted": true,
    "status": "acked | no_runtime | not_pty | no_socket | unsafe_mid_line | connect_error | protocol_error | mark_delivered_error",
    "control_path": "...",
    "delivered_seqs": [40, 41, 42],
    "remaining_pending": 0,
    "message": "..."
  },
  "wake": null
}
```

If PTY delivery fails and `trigger_notify_wake` runs, include both
`pty_delivery` and `wake`. If PTY delivery acks, `wake` should be `null` or a
diagnostic with `status='skipped_pty_delivered'`, but it must not spawn a
headless resume.

## Wake Coordinator Interplay

Current `src-tauri/src/wake_coordinator.rs::pty_runtime_is_busy` returns true
for any `session_runtime.mode == 'pty_interactive'`. That is safe for WU-D
because it avoids headless-waking a live interactive session, but WU-E needs it
to become liveness-aware so a stale PTY record does not block delivery forever.

Proposed behavior:

- A PTY runtime is busy only if `mode='pty_interactive'`,
  `run_state='running'`, `running_invocation_uuid` is present, the recorded PID
  identity is still live, and `pty_control_path` points to a socket that either
  accepts ping or has not yet failed liveness probing.
- If the recorded PID is dead or identity-mismatched, clear the stale running
  fields and `pty_control_path`, unlink the socket if it is under the owned
  broker directory, and continue as idle.
- If the process is live but injection returns `unsafe_mid_line`, wake must
  remain `busy`; the pending row stays queued for later delivery rather than
  starting a competing headless resume.
- Existing headless behavior remains unchanged for rows whose owner session is
  idle or whose runtime row is absent.

This keeps the WU-B/WU-D guarantee: no headless wake races against a live PTY
turn, and at-least-once mailbox durability is preserved when live injection
cannot be completed.

## Liveness and Cleanup

Interactive runtime state should be recorded and cleared with invocation guards.

At spawn:

- The existing spawn identity path already calls
  `record_interactive_child_identity` with `SpawnRuntimeMode::PtyInteractive`.
- Extend that path so `SessionRuntimeRunningUpdate` carries
  `pty_control_path: Some(path)` for brokered launches.
- If threading the path through `SpawnIdentityContext` is too invasive, record
  child identity first and immediately call a new sidecar helper to set
  `pty_control_path` only when `session_id` and `running_invocation_uuid` match
  the current interactive invocation.

At exit:

- Close the control listener so no new injection requests are accepted.
- Unlink the socket path if it is inside the owned broker directory.
- Restore the real terminal termios.
- Mark `session_runtime` idle with the current interactive invocation UUID and
  exit code.
- Clear `pty_control_path` in the same invocation-guarded update.
- Keep `mode='pty_interactive'` as historical last mode, but ensure wake logic
  uses `run_state` and liveness rather than mode alone.

On crash or kill:

- The sidecar row may remain `run_state='running'` and the socket file may
  remain on disk.
- `MailboxDb::session_liveness` already compares recorded PID identity against
  live `/proc` identity; WU-E should make stale clearing also clear
  `pty_control_path`.
- Notify-side socket connect failures should run stale cleanup before deciding
  whether to call `trigger_notify_wake`.

## Repl Resume Drain Decision

V1 does not add pre-launch mailbox drain for `agents repl --resume` or
top-level interactive resume. Draining pending mail into the initial interactive
prompt is not cheap because the broker cannot know when the provider UI has
finished startup and is ready for a new turn. Injecting before the prompt is
ready risks sending the envelope into startup output, a login flow, or a
provider-specific editor state.

WU-E only delivers mailbox rows that arrive while a brokered PTY session is
live, plus any older pending rows included in the ordered batch triggered by
that notify. Headless resume drain remains the reliable catch-up path for
mail queued while no live PTY exists. A later work unit can add provider-aware
readiness detection and `repl --resume` startup drain if product usage shows it
is needed.

## Scope Boundaries

V1 includes:

- Unix/Linux PTY broker for interactive launches with a real controlling TTY.
- Child slave as stdin/stdout/stderr and controlling TTY.
- Raw-mode real-terminal relay with restore on normal exit and unwind.
- Initial and `SIGWINCH` window-size propagation.
- External signal forwarding to child process group.
- Per-session Unix control socket recorded in `session_runtime.pty_control_path`.
- Framed local injection protocol with peer uid check and bounded request size.
- Notify-side enqueue-first, inject-if-live, ack-then-mark-delivered flow.
- Stale socket and stale runtime cleanup.
- XDG-isolated tests.

V1 excludes:

- Windows PTY support.
- Provider-specific prompt readiness detection.
- Broker-side durable pending queues.
- Bracketed-paste negotiation by default.
- Any `state.db` schema change.
- Any change to headless delivery semantics.

## Risks

| Risk | Mitigation |
|---|---|
| Termios restore failure leaves user terminal raw | RAII guard, explicit restore before return, panic/unwind test, and manual fallback instructions in error logs. |
| Injection while user is mid-line corrupts input | Conservative line-boundary tracking plus debounce and bounded refusal. Failure leaves mailbox pending. |
| Injection while provider is mid-render or in alternate screen | V1 cannot fully detect this. Keep envelope small, inject only after user-input debounce, and defer provider-aware readiness. |
| Multiline envelope semantics vary by provider CLI | Share renderer initially, test fixtures, and be ready to add PTY-specific single-line renderer or provider capability flags. |
| Ack before provider semantically consumes the turn | Define ack as successful write to PTY input queue. This is the delivery boundary. Include stable handles for semantic dedupe. |
| Crash after PTY write but before mark-delivered duplicates later | At-least-once semantics already allow duplicates. Stable `handle` remains in the envelope. |
| Crash after mark-delivered but before provider acts loses notification | Small residual risk inherent in ack-at-PTY boundary. Keep mark only after full write succeeds; do not mark on broker errors. |
| Stale socket blocks future wake | Stale cleanup checks PID identity and unlinks only sockets under the owned broker directory. Wake predicate becomes live-aware. |
| Socket injection by another same-uid process | `0700` dir, `0600` socket posture, Linux `SO_PEERCRED`, random-ish basename. Stronger nonce deferred. |
| Process group signal mistakes kill the broker or miss grandchildren | Child uses `setsid`; forward to negative child pid only after spawn; tests cover SIGTERM and Ctrl-C behavior. |
| Unix `sun_path` length overflow | Use short basenames or hash session/invocation when needed. Tests use long XDG temp paths. |

## Proof plan

Runtime claim: Brokered relay correctness: the child sees a TTY on stdio, user input reaches the child, child output reaches the user terminal, initial winsize is visible to the child, and raw mode is restored after clean exit.
Proof method: `crates/oulipoly-runtime/tests/wu_e_pty_broker.rs::broker_child_sees_tty_relays_io_preserves_exit_and_restores_raw_mode`.
Evidence-class match: The test launches the runtime interactive path under an outer PTY, so `/dev/tty`, termios, winsize, relay reads/writes, and child exit all exercise the actual Unix PTY broker rather than a mocked adapter.

Runtime claim: Control-socket injection occurs only at a safe line boundary; otherwise the broker returns `unsafe_mid_line` and does not inject.
Proof method: `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::tests::control_request_wait_observes_newline_from_real_input` and `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::tests::control_request_unsafe_midline_returns_err`.
Evidence-class match: These tests call the broker's real control-request processing path with real Unix stream pairs and pipe fds, so the line-boundary wait/refusal logic is exercised at the same frame-processing seam used by live notify injection.

Runtime claim: Socket ack marks the injected mailbox batch delivered; socket failure leaves the row pending and preserves wake-busy behavior for a live PTY session.
Proof method: `src-tauri/tests/wu_e_pty_delivery_integration.rs::notify_live_pty_ack_marks_delivered_and_skips_headless_wake` and `src-tauri/tests/wu_e_pty_delivery_integration.rs::notify_live_pty_failure_leaves_pending_and_wake_busy`.
Evidence-class match: These integration tests run the notify command against an isolated sidecar DB and real Unix control socket, then inspect `mark_delivered` side effects, pending rows, PTY diagnostics, and wake diagnostics instead of asserting only on helper return values.

Runtime claim: Stale PTY socket/runtime state is cleaned so it does not permanently block future wake behavior.
Proof method: `src-tauri/tests/wu_e_pty_delivery_integration.rs::notify_stale_socket_cleans_runtime_and_does_not_report_busy`.
Evidence-class match: The test seeds a dead runtime identity plus stale socket path in the sidecar, runs the actual notify/wake path, and verifies the runtime row is idle, `pty_control_path` is cleared, and the stale socket file is removed.

Runtime claim: When no controlling terminal exists, interactive launch falls back to inherited stdio and records no `pty_control_path`.
Proof method: `crates/oulipoly-runtime/tests/wu_e_interactive_fallback.rs::no_controlling_terminal_fallback_records_no_pty_control_path`.
Evidence-class match: The test spawns the runtime launch under a `setsid()` child with isolated XDG/HOME state, asserts the child cannot open `/dev/tty`, and then verifies the actual sidecar `session_runtime` row has `mode='pty_interactive'` with `pty_control_path == None`.

Runtime claim: A real live interactive session receives an `agent-bash-complete` notification through the PTY control socket without exiting.
Proof method: `src-tauri/tests/wu_e_pty_delivery_integration.rs::fixture_interactive_session_agent_bash_completion_arrives_live`.
Evidence-class match: The test starts the compiled runner in `repl --resume` under an outer PTY, runs the compiled notify command with real metadata artifacts and caller identity resolution, observes PTY delivery ack, verifies the fixture child receives the `[OULIPOLY NOTIFICATIONS]` envelope while still live, and inspects the sidecar cleanup.

## Test Plan

All tests must isolate `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`, `XDG_DATA_HOME`,
`XDG_CONFIG_HOME`, and `HOME` under `tempfile::TempDir`. Tests must assert that
default user state paths are untouched, matching the WU-B integration pattern in
`src-tauri/tests/wu_b_mailbox_integration.rs`.

### Runtime PTY Broker Tests

These can live in `crates/oulipoly-runtime/tests/wu_e_pty_broker.rs` or a
Unix-only module near `executor/cli/pty_broker.rs`.

| Test | Proof |
|---|---|
| `broker_child_sees_tty_on_all_stdio` | Fixture child asserts `isatty(0/1/2)` and exits 0. |
| `broker_relays_user_input_and_child_output` | Outer test PTY drives runner input; fixture child echoes a line; test sees echoed output on outer PTY. |
| `broker_preserves_exit_status` | Fixture child exits 7 and `InteractiveExecutionResult.exit_code` is 7. |
| `broker_restores_raw_mode_on_clean_exit` | Capture outer terminal termios before launch and assert restored after child exits. |
| `broker_restores_raw_mode_on_error_path` | Force broker error after raw mode entry and assert termios restored. |
| `broker_initial_winsize_propagates` | Set outer PTY winsize, child reads `TIOCGWINSZ`, and output matches. |
| `broker_sigwinch_updates_child_pty` | Resize outer PTY, signal runner, child reads updated winsize. |
| `broker_ctrl_c_reaches_child_foreground_group` | Fixture child traps or observes SIGINT after test writes `0x03` through outer PTY. |
| `broker_external_sigterm_forwards_to_child_group` | Send SIGTERM to runner and assert child process group terminates and status maps through terminal signal logic. |

### Control Socket Protocol Tests

| Test | Proof |
|---|---|
| `control_socket_created_under_xdg_runtime_with_0700_parent` | Broker records path in sidecar and filesystem permissions are correct. |
| `control_socket_rejects_bad_magic_or_oversize_frame` | Client receives `err`, child receives no bytes. |
| `control_socket_rejects_wrong_peer_uid_when_simulatable` | Linux-only helper or lower-level unit around peer credential validation. |
| `control_socket_inject_ack_writes_to_child_tty` | Fixture child reads from its TTY and prints the injected envelope; client receives ack. |
| `control_socket_unsafe_midline_returns_err` | Test types partial input, sends inject, broker returns `unsafe_mid_line`, child receives no envelope. |
| `control_socket_injects_after_line_boundary_debounce` | Test types input plus newline, sends inject, broker waits/debounces and then acks. |

### Notify and Mailbox Integration Tests

These should live beside WU-B tests in `src-tauri/tests/wu_e_pty_delivery_integration.rs` and run the compiled
`oulipoly-agent-runner` binary with isolated XDG/HOME.

| Test | Proof |
|---|---|
| `notify_live_pty_ack_marks_delivered` | Seed owner identity/runtime or launch fixture interactive session, run notify, socket acks, row has `delivered_at` and `delivered_by_invocation_uuid` equal to live invocation. |
| `notify_live_pty_failure_leaves_pending` | Break socket or force protocol err; notify exits 0, row remains pending. |
| `notify_live_pty_unsafe_midline_leaves_pending_and_wake_busy` | User mid-line causes inject refusal; notify diagnostics show PTY failure and wake `busy`. |
| `notify_stale_socket_cleans_runtime_and_wakes_headless` | Seed stale `pty_control_path` and dead PID identity; notify leaves pending, clears socket/runtime, and wake path is allowed to proceed. |
| `notify_injects_pending_batch_in_seq_order` | Seed older pending rows, enqueue new row, live injection envelope contains rows in `seq ASC`, and all injected seqs mark delivered. |
| `notify_ack_skips_headless_wake` | Live ack path returns no spawned wake diagnostic and no detached resume process is started. |
| `notify_no_live_socket_preserves_headless_behavior` | Runtime absent or idle; notify enqueues and existing wake behavior matches WU-D. |

### End-to-End Test

`fixture_interactive_session_agent_bash_completion_arrives_live`:

1. Start a fixture interactive model under the compiled runner through an outer
   PTY harness.
2. Fixture child prints readiness, reads turns from its TTY, and logs every
   received turn to a temp file.
3. The test seeds or captures a caller identity chain resolving to the fixture
   provider session.
4. Run `agents notify agent-bash-complete --caller-ppid ... --handle ...` with
   real temp `meta.json`, `log`, and `rc` files.
5. Assert notify reports PTY delivery ack.
6. Assert the fixture child receives the `[OULIPOLY NOTIFICATIONS]` envelope in
   the live session without the interactive process exiting.
7. Assert the mailbox row is delivered and no default user XDG path was touched.

### Regression Tests

- Existing WU-B headless mailbox tests must continue to pass unchanged.
- Existing terminal signal mapping tests in `terminal_signal.rs` must continue
  to pass.
- Add a non-Unix or no-controlling-terminal test that verifies fallback does not
  record `pty_control_path` and leaves PTY immediate injection unavailable.

## 12-Line Summary

1. WU-E makes agent-runner the owner of interactive PTYs.
2. Interactive children get a PTY slave for stdin, stdout, stderr, and controlling TTY.
3. Agent-runner relays between the user's real terminal and the PTY master.
4. Raw mode is guarded and restored on exit or unwind.
5. Window size and external signals propagate to the child process group.
6. A per-session Unix socket is recorded in `session_runtime.pty_control_path`.
7. `notify` still enqueues first for durable at-least-once behavior.
8. If a live PTY socket exists, `notify` injects the ordered pending batch.
9. Socket ack means the envelope was written to the PTY input queue.
10. Acked rows are marked delivered; failures stay pending.
11. Live PTY sessions are not headless-woken; stale PTY rows are cleaned.
12. V1 is Unix-only, sidecar-only, XDG-isolated, and uses a minimal in-tree broker.
