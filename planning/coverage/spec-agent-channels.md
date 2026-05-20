# spec-agent-channels — Sub-agent IPC: store, scratchpad, messenger

## Source files

- `crates/oulipoly-agent-messenger/src/lib.rs`
- `crates/oulipoly-agent-messenger/src/main.rs`
- `crates/oulipoly-agent-scratchpad/src/lib.rs`
- `crates/oulipoly-agent-scratchpad/src/main.rs`
- `crates/oulipoly-agent-store/src/lib.rs`
- `crates/oulipoly-agent-store/src/main.rs`

## Preconditions

- A sub-agent has been spawned by the runtime with the relevant channel
  binaries available on PATH (or invoked as in-process libraries).
- For the store: a configured durable storage root (filesystem path).
- For the scratchpad: an ephemeral working directory.
- For the messenger: a routing target (in-process or out-of-process
  recipient).

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| Store `put` with a content-addressed key. | Writes the blob durably; subsequent `get` returns the same bytes. |
| Store `get` on a missing key. | Returns a structured "not found" error; does NOT create a phantom entry. |
| Store `init` on an empty root. | Creates the bootstrap directory structure with documented permissions. |
| Store `tombstone` on an existing key. | Marks the key tombstoned; subsequent `get` reports tombstoned-not-deleted; `list` filters tombstones unless asked. |
| Scratchpad `write` then `read`. | Round-trips bytes (no encoding mangling). |
| Scratchpad `publish` to a destination. | Promotes the scratchpad entry to a durable target per the documented addressing scheme. |
| Scratchpad `gc`. | Removes ephemeral entries past the configured TTL; leaves published entries. |
| Messenger `return` from a sub-agent. | Transports the return envelope to the parent; the parent's invocation scope receives it. |
| Messenger `show` / `list`. | Returns the available messages without consuming them. |

## Edge cases

- Concurrent store writers with the same content-addressed key — last
  writer wins; the address invariant guarantees byte-identical
  payloads (race is harmless).
- Scratchpad gc during an active write — gc skips files whose mtime is
  within the configured grace window.
- Messenger returns a payload larger than the configured size limit —
  surface a structured "envelope too large" error rather than
  truncating.
- Store init on a non-empty root that is already a store — idempotent;
  do NOT clobber.
- CLI invocation outside the documented invocation scope — exit with a
  scope error (see `cli_invocation_scope.rs` tests).

## Error conditions

- `StoreInitFailed` / `StorePutFailed` / `StoreGetFailed` /
  `StoreTombstoneFailed` — typed per operation.
- `ScratchpadWriteFailed` / `ScratchpadReadFailed` /
  `ScratchpadPublishFailed`.
- `MessengerReturnFailed` — transport error or recipient unavailable.
- `InvalidInvocationScope` — caller is not inside an allowed
  sub-agent context.

## Boundaries

- Channels do NOT route real provider invocations — the runtime
  balancer/executor own that.
- Channels do NOT mutate session metadata — they carry sub-agent
  artifacts only.
- The three crates are intentionally separate single-role surfaces;
  they DO NOT call each other directly. Composition is the caller's
  responsibility.

## Declared test patterns

Per `~/ai/conventions/testing.md`: CLI invocation + exit-code tests,
library addressing tests, scope-violation tests, README-alignment
contract tests.

- `crates/oulipoly-agent-messenger/tests/agent_return_convention_alignment.rs`
- `crates/oulipoly-agent-messenger/tests/cli_exit_codes.rs`
- `crates/oulipoly-agent-messenger/tests/cli_invocation_scope.rs`
- `crates/oulipoly-agent-messenger/tests/cli_list_returned.rs`
- `crates/oulipoly-agent-messenger/tests/cli_return.rs`
- `crates/oulipoly-agent-messenger/tests/cli_return_channel.rs`
- `crates/oulipoly-agent-messenger/tests/cli_show.rs`
- `crates/oulipoly-agent-messenger/tests/cli_version.rs`
- `crates/oulipoly-agent-messenger/tests/library_channel.rs`
- `crates/oulipoly-agent-messenger/tests/library_invocation_scope.rs`
- `crates/oulipoly-agent-messenger/tests/library_return.rs`
- `crates/oulipoly-agent-messenger/tests/library_show_list.rs`
- `crates/oulipoly-agent-messenger/tests/library_verdict.rs`
- `crates/oulipoly-agent-messenger/tests/readme_alignment.rs`
- `crates/oulipoly-agent-scratchpad/tests/cli_delete.rs`
- `crates/oulipoly-agent-scratchpad/tests/cli_exit_codes.rs`
- `crates/oulipoly-agent-scratchpad/tests/cli_gc.rs`
- `crates/oulipoly-agent-scratchpad/tests/cli_invocation_scope.rs`
- `crates/oulipoly-agent-scratchpad/tests/cli_list.rs`
- `crates/oulipoly-agent-scratchpad/tests/cli_publish.rs`
- `crates/oulipoly-agent-scratchpad/tests/cli_read.rs`
- `crates/oulipoly-agent-scratchpad/tests/cli_write.rs`
- `crates/oulipoly-agent-scratchpad/tests/library_addressing.rs`
- `crates/oulipoly-agent-scratchpad/tests/library_gc.rs`
- `crates/oulipoly-agent-scratchpad/tests/library_metadata.rs`
- `crates/oulipoly-agent-scratchpad/tests/library_publish.rs`
- `crates/oulipoly-agent-scratchpad/tests/readme_alignment.rs`
- `crates/oulipoly-agent-store/tests/cli_exit_codes.rs`
- `crates/oulipoly-agent-store/tests/cli_get.rs`
- `crates/oulipoly-agent-store/tests/cli_get_meta.rs`
- `crates/oulipoly-agent-store/tests/cli_init.rs`
- `crates/oulipoly-agent-store/tests/cli_list.rs`
- `crates/oulipoly-agent-store/tests/cli_put.rs`
- `crates/oulipoly-agent-store/tests/cli_tombstone.rs`
- `crates/oulipoly-agent-store/tests/library_addressing.rs`
- `crates/oulipoly-agent-store/tests/library_bootstrap.rs`
- `crates/oulipoly-agent-store/tests/library_concurrency.rs`
- `crates/oulipoly-agent-store/tests/library_metadata.rs`
- `crates/oulipoly-agent-store/tests/library_tombstone.rs`
- `crates/oulipoly-agent-store/tests/readme_alignment.rs`

## Cross-references

- `planning/coverage/spec-executor.md` — sub-agent processes are
  spawned via the executor pattern.
- `AGENTS.md` § Rust Workspace Structure.
