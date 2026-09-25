# Private native F input preparation

## Source files

- `crates/oulipoly-state/src/mailbox/fresh_native_f.rs`
- `crates/oulipoly-state/src/mailbox/migrations/0040_fresh_native_f_preparation.sql`
- `src-tauri/src/native_f_preparation.rs`

## Preconditions

- A fresh v30 F grant names an accepted W source, exact original recipient, retained payload, and a pending row.
- A single running PTY generation for the same fresh session has the original recipient as creator, an exact live provider process, and a socket endpoint.
- The original Runner has preflighted the selected provider account and obtained a complete typed `session.read_turns` Tail page with a resume token before any input send.

## Input → Expected output

- An exact grant/token, resident generation, provider instance/settings, envelope nonce, and Tail token create one immutable pre-send record.
- The broker renders a deterministic text envelope from the retained W bytes and stores both the text and its hash separately from the W payload hash.
- Exact same-key retry and readback return identical record bytes after a lost reply or restart.

## Edge cases

- A changed F grant, original actor, generation, endpoint, settings, Tail token, or nonce cannot inherit an existing request key.
- One nonce cannot name two different F payload envelopes.
- An old mailbox WAL row cannot stand in for the fresh F grant or accepted W source.

## Error conditions

- Missing or ambiguous resident generation, missing socket, changed process, missing Tail anchor, and mismatched F token refuse preparation.
- The current ordinary private root flow has no live PTY generation or adapter Tail join, so it must leave this preparation absent.

## Boundaries

- Preparation does not write to the PTY, assert a provider turn, ACK F, launch headless resume, or publish an ordinary caller result.
- The broker stores endpoint and Tail fields supplied by the pinned original Runner; the private caller helper performs the adapter preflight and typed Tail read. A direct broker preparation alone is not native receipt authority.

## Declared test patterns

- `crates/oulipoly-kernel-broker/tests/age319_fresh_recipient_socket.rs` exercises clean fixture preparation, readback, lost reply, restart, refusal, old WAL isolation, and pending/manual ACK separation.
- Existing native page and PTY outbound tests cover the adapter Tail and transport mechanics separately; this slice does not join a physical send.

## Cross-references

- `AGENTS.md` sections on State migrations, fresh provider contract, and test audit infrastructure.
- `planning/coverage/spec-kernel-broker.md` and `planning/coverage/spec-state-db.md`.
