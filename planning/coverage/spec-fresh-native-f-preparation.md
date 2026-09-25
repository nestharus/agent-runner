# Private native F input preparation

## Source files

- `crates/oulipoly-state/src/mailbox/fresh_native_f.rs`
- `crates/oulipoly-state/src/mailbox/migrations/0040_fresh_native_f_preparation.sql`
- `src-tauri/src/native_f_preparation.rs`
- `src-tauri/src/kernel_entry.rs`
- `src-tauri/src/kernel_entry/root_pty_control.rs`
- `src-tauri/src/main.rs`
- `crates/oulipoly-kernel-broker/src/linux_main.rs`
- `crates/oulipoly-kernel-broker/src/fresh_provider.rs`
- `crates/oulipoly-kernel-broker/src/protocol.rs`
- `crates/oulipoly-runtime/src/executor/cli/pty_broker/mod.rs`
- `crates/oulipoly-runtime/src/executor/cli/fresh_remote.rs`
- `crates/oulipoly-runtime/src/executor/cli/pty_broker/tui_control.rs`

## Preconditions

- A fresh v30 F grant names an accepted W source, exact original recipient, retained payload, and a pending row.
- A single running PTY generation for the same fresh session has the original recipient as creator, an exact live provider process, and a socket endpoint.
- The original Runner has preflighted the selected provider account and obtained a complete typed `session.read_turns` Tail page with a resume token before any input send.
- The resident PTY control server answers a fresh read-only challenge with its live generation, creator and child process identities, pinned account/instance/settings, and bound provider session. The broker compares the answer, socket inode, and peer credentials with its selected generation.

## Input → Expected output

- An exact grant/token, resident generation, provider instance/settings, envelope nonce, and Tail token create one immutable pre-send record.
- The broker renders a deterministic text envelope from the retained W bytes and stores both the text and its hash separately from the W payload hash.
- Exact same-key retry and readback return identical record bytes after a lost reply or broker restart while the selected resident endpoint remains live and unchanged.

## Edge cases

- A changed F grant, original actor, generation, endpoint, settings, Tail token, or nonce cannot inherit an existing request key.
- A replaced socket inode, wrong peer or process, mismatched generation/provider/account/session, or lost query reply refuses creation and live readback.
- One nonce cannot name two different F payload envelopes.
- An old mailbox WAL row cannot stand in for the fresh F grant or accepted W source.

## Error conditions

- Missing or ambiguous resident generation, missing socket, changed process, missing Tail anchor, and mismatched F token refuse preparation. Durable readback refuses when its live generation cannot be reattested.
- The current ordinary private root flow has no live PTY generation or adapter Tail join, so it must leave this preparation absent.
- The original recipient reads back the exact F key before calling preparation. A lost F token, changed grant/session, or absent resident PTY and selected adapter page authority refuses without creating a record or ACK.

## Boundaries

- Preparation does not write to the PTY, assert a provider turn, ACK F, launch headless resume, or publish an ordinary caller result.
- Private `^` PTY handoff is earlier and nonactivating: the original released Runner registers a distinct interactive argv/env/cwd/image plan for each configured account, then reads back the broker's one-use interactive selection for the held D/actor/source/config/account. It creates a real master/slave pair, serves the broker challenge from its own process, returns its still-open master via `SCM_RIGHTS`, and retains master/control custody through the current headless provider Q path. The broker requires the interactive role and exact selected digest; headless h/f/K/Q stay separate. This creates no interactive provider K, resident generation, F preparation, send, or ACK. Durable stamps alone cannot attest PTY or server liveness after broker restart; a fresh challenge is required.
- Private `{` rechecks the exact selected interactive recipe, pinned config source, image mount/inode, cwd, and current root-held PTY/control pair on the original-root broker path. Exact restart replay returns the same `interactive-k-preparation.json`; a headless recipe or changed binding refuses. This is pre-K admission only. It does not consume a K grant, fork the provider, relay PTY I/O, or settle interactive Q.
- The broker attests the resident endpoint before storing an F record. The private caller helper performs adapter preflight and a typed Tail read on creation and readback. A direct broker preparation still lacks independently proven Tail provenance and is not native receipt authority.

## Declared test patterns

- `crates/oulipoly-kernel-broker/tests/age319_fresh_recipient_socket.rs` exercises clean fixture preparation, readback, lost reply, restart, refusal, old WAL isolation, and pending/manual ACK separation.
- `crates/oulipoly-kernel-broker/tests/private_root_join.rs` exercises the actual Runner-owned interactive candidate/selection, `^` challenge, and `{` preparation with interactive args different from headless args, exact selected digest/readback, wrong role/digest/account refusal, headless-recipe preparation refusal, replay, broker restart and unchanged preparation bytes, held master beyond the first challenge while the headless provider runs, sibling/copied endpoint refusal, replacement fail-closed, root-exit custody loss and normal socket cleanup. It also exercises accepted W to original-root F and repeatable refusal when the headless private K/Q route lacks a resident PTY generation and typed adapter page authority.
- Broker unit tests exercise the challenged `^` frame, real PTY pair and live same-process control socket checks, including a same-actor response with the wrong master, wrong slave, no TTY, wrong account/session, changed pair, replaced socket inode and post-K preparation refusal. Runtime tests distinguish the separate interactive argv from headless argv. They do not claim a broker-launched interactive provider.
- PTY broker unit tests exercise both plain and TUI resident query handlers without child input.
- Existing native page and PTY outbound tests cover the adapter Tail and transport mechanics separately; this slice does not join a physical send.

## Cross-references

- `AGENTS.md` sections on State migrations, fresh provider contract, and test audit infrastructure.
- `planning/coverage/spec-kernel-broker.md` and `planning/coverage/spec-state-db.md`.
