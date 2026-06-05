# Cap-Gate Phase 6a Contract

Scope: incremental gate `42200fb..9ba1275`. Production substance is commit `9e00408`; commit `9ba1275` is a test-only `OULIPOLY_DATA_DIR` scrub sweep and is treated here as test infrastructure evidence, not production function inventory.

## Component declared roles

Roles: accessor, formatter, mapper, orchestration, parser, predicate, validator.

- accessor: touched files expose existing context fields, terminal fds, sidecar identities, and PTY state without changing their meaning.
- formatter: touched files format warnings, stable validation errors, socket path strings, protocol frames, and bounded socket-name/hash strings.
- mapper: touched files derive supervisor/spawn context values, sidecar/runtime update records, command/result DTOs, socket paths, and relay readiness state.
- orchestration: touched files sequence supervised child lifecycles, interactive launch paths, spawn identity sidecar updates, captured-session backfill, PTY relay, and runtime-state transitions.
- parser: touched files parse parent invocation env values and PTY control request/response frames.
- predicate: touched files answer live-signal, terminal availability, socket ownership/liveness, fd-readiness, line-safety, and PTY EOF conditions.
- validator: touched files validate interactive provider args, client/control payloads, active socket state, and peer credentials.

## Per-file declared roles

| File | Declared roles | Justification |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | mapper, orchestration, predicate | Matches the file-local `## Declared roles` header. The file maps supervisor configuration/output state, orchestrates the supervised child lifecycle and capture-time backfill, and predicates live terminal-signal handling and one-shot capture observation. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | accessor, formatter, mapper, orchestration, parser | The file-local header declares formatter/mapper/orchestration/parser and the code also has narrow context accessors. It records child identity, maps env/context/runtime records, parses parent invocation env, and formats non-fatal warning logs. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | formatter, mapper, orchestration, validator | Matches the file-local `## Declared roles` header. The file sequences interactive launch, validates provider interactive args, formats the missing-args error, and maps terminal status into `InteractiveExecutionResult`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | accessor, formatter, mapper, orchestration, parser, predicate, validator | No file-local `## Declared roles` header is present in this file; this is the WU-E-gated full set verified against the code. The broker owns PTY relay orchestration, protocol parsing/formatting, path and readiness mapping, terminal/fd access, socket/line predicates, and payload/peer/socket validators. |

## Function inventory

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs::execute_with_supervisor` | orchestration | Sequences supervised child setup, identity recording, output drains, capture observation, terminal outcome handling, and output finalization. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs::observe_streamed_session_id` | orchestration | Gates the single-fire capture transition, delegates parsing to the session-capture parser, invokes the backfill seam, and stores the captured id. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::record_child_identity` | orchestration | Coordinates optional spawn-context handling, verified PID sidecar recording, spawn-known session-runtime marking, error logging, and returning the recorded identity. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::backfill_captured_session_id` | orchestration | Requires both spawn context and process identity, then sequences sidecar session_id backfill and session-runtime mark-running for the captured id. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::mark_session_running` | orchestration | Preserves the spawn-known session path by extracting the existing context session_id and delegating to the shared mark-running seam. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::mark_session_running_with_session_id` | orchestration | Opens the mailbox and applies the existing `mark_session_running` upsert, with non-fatal warning on failure. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::backfill_pid_identity_session_id` | orchestration | Opens the PID identity sidecar and applies `set_session_id`, routing missing-row and failure outcomes to warning helpers. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::warn_pid_identity_session_backfill_missing` | formatter | Formats the structured warning for a missing sidecar row during captured-session backfill. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::warn_pid_identity_session_backfill_failed` | formatter | Formats the structured warning for a sidecar write failure during captured-session backfill. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs::execute_interactive_with_result_and_model_identity` | orchestration | Sequences the existing interactive launch path and adjusts the identity-recording call site to ignore the optional return value. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::execute_interactive_child` | orchestration | Sequences the existing PTY launch/relay lifecycle and adjusts the identity-recording call site to ignore the optional return value. |

No `MULTI-CLASSIFIER-RISK` entries are declared for the production delta. Import-only/type-path-only edits are not inventoried as meaningful function changes.

## Test infrastructure note

Commit `9ba1275` changes test and fixture environment isolation so XDG-isolated harnesses scrub the higher-precedence `OULIPOLY_DATA_DIR` pin. It introduces no production behavior and is not included in the production function inventory above.

## Adapter declarations

```yaml
adapter_declarations:
  - component: crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs
    role: adapter
    Translates:
      - "pid_identity sidecar contract: record_live_process_identity lookup/identity return and PidIdentityDb::set_session_id backfill"
      - "mailbox session_runtime running contract: MailboxDb::mark_session_running via SessionRuntimeRunningUpdate"
      - "executor spawn-identity seam contract: SpawnIdentityContext, record_child_identity, backfill_captured_session_id"
```

`supervision/mod.rs` consumes the spawn-identity seam through its narrow API only: `SpawnIdentityContext`, `record_child_identity`, and `backfill_captured_session_id`. It does not reach into the PID identity sidecar or mailbox/session-runtime storage contracts directly.

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
