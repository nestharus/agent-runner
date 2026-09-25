# Codex raw rollout verifier seam

## Source files

- `crates/oulipoly-kernel-broker/src/codex_raw_verifier.rs`

## Preconditions

- An independent caller captures a complete Codex rollout JSONL Tail before F on an `O_NOFOLLOW` descriptor and supplies the reviewed `codex.rollout.jsonl/user-input-text-v1` format.
- State has already prepared an immutable F envelope. Selected adapter fields are comparison targets only.

## Input → Expected output

- A stable, bounded, same-inode source with the unchanged pre-Tail prefix and exactly one complete post-Tail user `input_text` record can return a typed raw-turn certificate when its decoded UTF-8 bytes exactly equal the prepared envelope and selected page fields agree.
- The certificate names the source inode, byte offsets and independent raw hashes. It is evidence of file agreement, not a native-F receipt or ACK.

## Edge cases

- Changed body, nonce, session metadata, source inode, Tail byte frontier, CRLF text, and a delivery marker refuse the raw match even if an adapter projection or canonical digest matches.
- Extra, intervening, partial, oversized, unsupported, duplicate-key, ambiguous-content, or changing records refuse.

## Error conditions

- Source disappearance or revision change returns pending; unsupported format/record and size bounds return unknown. No result requests replay.

## Boundaries

- No selected resident Codex process-to-rollout custody or physical PTY-to-store transform is proven by this seam. It is not called by default Broker certification or ACK. A root path or adapter page alone cannot enable it.
- It does not read provider accounts, live sessions, credentials, or an installed native store.

## Declared test patterns

- `crates/oulipoly-kernel-broker/src/codex_raw_verifier.rs` unit tests use the exact offline synthetic JSONL fixture at `crates/oulipoly-kernel-broker/tests/fixtures/age319-codex-rollout-baseline.jsonl` and assert its reported hashes, offsets, typed page fields and refusal cases.
- Default Broker receipt and ACK closure and private physical F behavior remain covered by the existing `linux_main.rs`, `age319_fresh_recipient_socket.rs`, and `private_root_join.rs` controls.

## Cross-references

- `planning/coverage/spec-fresh-native-f-preparation.md`, `planning/coverage/spec-kernel-broker.md`, and `AGENTS.md`.
