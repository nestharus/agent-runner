# 06 Import-Replace CodeRabbit Audit History

## Pass 1

- Source: `.tmp/phase7/coderabbit-pass1.md`
- Findings: 14
- Real applied: 14
- Skipped: 0
- Determination: continue after amend
- Tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`

Applied finding IDs:
- `R1-F01`: Proposal module paths updated to landed `session_replace` module.
- `R1-F02`: Markdown code block language tags added.
- `R1-F03`: Test SHA-256 helper made in-process.
- `R1-F04`: Malformed preimage hash keeps exit 2 but now reports `invalid-argument`.
- `R1-F05`: Fixture DB initialization side effect documented.
- `R1-F06`: Fixture child stdin write errors surface and stdin is closed.
- `R1-F07`: Canonical input timestamps must parse as RFC3339.
- `R1-F08`: Stdin reader now waits for EOF instead of truncating after idle polls.
- `R1-F09`: Directory fsync failures now fail the operation.
- `R1-F10`: Provider-native canonical parsing preserves resolved provider aliases.
- `R1-F11`: Unsupported native roles/content are marked unsupported instead of silently dropped as supported content.
- `R1-F12`: Read-only schema preflight runs before journal/staging writes.
- `R1-F13`: Lock files are removed on post-create write/fsync failure.
- `R1-F14`: Transcript locator receives `SESSION_ID` via environment.

Watch signals for pass 2:
- Schema preflight may invite a follow-up asking for a shared schema-probe API; current implementation is local because no reusable read-only probe exists in this branch.
- Unsupported native multimodal payloads are marked as unsupported records; the current canonical `ContentChunk` only supports text, so preserving binary payloads would require a larger canonical schema change outside Rev 4.

## Pass 2

- Source: `.tmp/phase7/coderabbit-pass2.md`
- Findings: 10
- Real applied: 8
- Skipped: 2
- Determination: continue after amend
- Tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`

Applied finding IDs:
- `R2-F01`: Process-tree audit wording varied without changing facts.
- `R2-F03`: `StateDb::open_default()` side-effect intent documented before raw connection use.
- `R2-F04`: `sha256_hex` avoids per-byte temporary string allocation.
- `R2-F05`: Unsupported-storage errors preserve provider context.
- `R2-F06`: Orphan canonical recovery window documented, implemented, and tested.
- `R2-F07`: Lock creation now uses synced temp files plus no-overwrite hard-link publication.
- `R2-F09`: Lock files are scoped by provider/session pair.
- `R2-F10`: Provider-native source provenance records byte offsets and line hashes.

Skipped finding IDs:
- `R2-F02`: Flip-flop with pass 1 and Rev 4; malformed `--preimage-sha256` remains exit 2 `invalid-argument`, not exit 15 `invalid-input-transcript`.
- `R2-F08`: Gated-design expansion; lease renewal belongs to shared `SessionLock`/pause-handshake semantics rather than import-replace inventing a second lock format.

Watch signals for pass 3:
- CodeRabbit may continue to ask for lease renewal. Treat as churn unless the proposal/contract changes.
- Lock temp-file cleanup is intentionally best-effort; stale temp files are not final locks and do not block acquisition.

## Pass 3

- Source: `.tmp/phase7/coderabbit-pass3.md`
- Findings: 11
- Real applied: 9
- Skipped: 2
- Determination: continue after amend
- Tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`

Applied finding IDs:
- `R3-F01`: Removed unreachable fallback block from `jsonl_data_lines`.
- `R3-F02`: Documented canonical import lineage reset.
- `R3-F03`: Documented intentional locator shell execution with UUID-validated `SESSION_ID`.
- `R3-F04`: Documented dummy source provenance in fixtures.
- `R3-F05`: Recovery quarantines a bad journal and continues instead of aborting all sessions.
- `R3-F06`: Hookpoint research now includes orphan canonical side-file scanning.
- `R3-F07`: Unparsable lock metadata fails closed as busy.
- `R3-F09`: Supported-surface R4-F05 marked resolved by Phase 6 orphan cleanup.
- `R3-F10`: Drop-time lock removal checks this owner's token and active expiry before unlink.

Skipped finding IDs:
- `R3-F08`: Repeated lease-renewal scope expansion; still gated-design churn.
- `R3-F11`: Multimodal `ContentChunk` schema expansion conflicts with Rev 4 v1 canonical schema and unsupported-record handling.

Watch signals for pass 4:
- Lease renewal and multimodal schema requests are now repeated design expansions; classify as churn unless paired with a concrete bug inside the approved v1 contract.

## Pass 4

- Source: `.tmp/phase7/coderabbit-pass4.md`
- Findings: 8
- Real applied: 5
- Skipped: 3
- Determination: continue after amend
- Tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`

Applied finding IDs:
- `R4-F01`: Added `ReplaceError::InvalidArgument` for malformed preimage hash usage errors.
- `R4-F02`: Added a short-lived acquire guard around stale-lock cleanup and publish.
- `R4-F03`: Contract recovery prose now includes orphan canonical and missing-preimage branches.
- `R4-F05`: Supported-surface count/rationale now treats R4-F05 as resolved.
- `R4-F08`: Proposal temp cleanup is scoped to the resolved transcript path prefix.

Skipped finding IDs:
- `R4-F04`: Repeated lease-renewal request; gated-design churn.
- `R4-F06`: False positive; the recovery trigger command also exports the intentionally corrupted transcript, so exit status is not the recovery success signal in T8.
- `R4-F07`: Public API refactor suggestion; current implementation already buffers bytes before mutation and keeps the existing CLI wrapper API.

Watch signals for pass 5:
- Lease renewal and buffered API reshaping are repeated or non-contractual. Stop if they are the only remaining items.

## Pass 5

- Source: `.tmp/phase7/coderabbit-pass5.md`
- Findings: 16
- Real applied: 10
- Skipped: 6
- Determination: continue after amend
- Tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`

Applied finding IDs:
- `R5-F01`: Removed duplicate top-level `line` from `InvalidInputTranscript` JSON.
- `R5-F03`: Made the ambiguous-session fixture deterministic while preserving the recent/ambiguous branch.
- `R5-F05`: Made fixture timestamps use two-digit seconds formatting.
- `R5-F06`: Staging canonical side-file cleanup now runs if rename to the canonical records path fails.
- `R5-F07`: Import-replace sleep test hook now accepts configurable milliseconds.
- `R5-F10`: Added recovery coverage for orphan canonical side files while a live session lock exists.
- `R5-F11`: Recovery orphan canonical cleanup now checks live session locks before deleting.
- `R5-F12`: Aligned journal contract/code on `postimage_sha256`, temp-file naming, and ambiguous-recovery side-file behavior.
- `R5-F14`: Proposal orphan canonical cleanup now documents the live-lock guard.
- `R5-F16`: Lock file names sanitize both provider name and session id.

Skipped finding IDs:
- `R5-F02`: The helper intentionally consumes stderr up to the marker; the returned `Output` is used for process status only, now documented inline.
- `R5-F04`: Multimodal chunk preservation is gated-design expansion; Rev 4 v1 marks unsupported records instead of extending canonical schema.
- `R5-F08`: Locator shell trust boundary was already documented in pass 3; no user-facing config guide exists in this branch.
- `R5-F09`: Repeated multimodal renderer expansion; outside Rev 4 v1 supported surface.
- `R5-F13`: Repeated lease-renewal request; gated-design churn for shared `SessionLock` semantics.
- `R5-F15`: Duplicate of multimodal canonical-schema expansion; skipped for the same Rev 4 v1 reason.

Watch signals for pass 6:
- Lease renewal and multimodal/binary canonical schema requests are repeated design expansions. Stop if they are the only remaining items.

## Pass 6

- Source: `.tmp/phase7/coderabbit-pass6.md`
- Findings: 9
- Real applied: 6
- Skipped: 3
- Determination: continue after amend
- Tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`

Applied finding IDs:
- `R6-F02`: Transcript temp file is removed if the final transcript rename fails.
- `R6-F05`: Hookpoint research now consistently uses the landed `session_replace` module name.
- `R6-F06`: Lock acquire rolls back a published lock if lock-directory fsync fails.
- `R6-F07`: Live-lock scans now propagate lock-directory I/O errors instead of treating them as no live lock.
- `R6-F08`: Malformed lock metadata during acquire is reported as an operational lock-store error rather than normal contention.
- `R6-F09`: Transcript locator execution is now bounded by a timeout and reports timeout separately.

Skipped finding IDs:
- `R6-F01`: Synthetic Codex turn-id fallback is existing unsupported-record handling; adding stderr/log noise is outside the import-replace stdout/stderr contract.
- `R6-F03`: Repeated lease-renewal request; gated-design churn for shared `SessionLock` semantics.
- `R6-F04`: Race-barrier refactor suggestion; current test already asserts the one-winner contract and loser cleanup without adding another test seam.

Watch signals for pass 7:
- Lease renewal is still a repeated design expansion. Stop if it is the only remaining substantive issue.

## Pass 7

- Source: `.tmp/phase7/coderabbit-pass7.md`
- Findings: 6
- Real applied: 2
- Skipped: 4
- Determination: continue to final allowed pass after amend
- Tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`

Applied finding IDs:
- `R7-F03`: Session lock metadata now stores a true SHA-256 digest of the lease token, and drop-time ownership checks compare against the digest.
- `R7-F06`: Supported-surface prose now consistently marks R4-F05 as resolved by Phase 6 orphan canonical recovery.

Skipped finding IDs:
- `R7-F01`: Strict empty-stderr success assertions are intentional for this CLI JSON contract; tests that expect hook stderr use a separate helper.
- `R7-F02`: Compile-gating test hooks would break integration tests that exercise the built binary unless a broader feature/build setup is added; environment-only hooks remain intentionally private and opt-in.
- `R7-F04`: Repeated lease-renewal request; gated-design churn for shared `SessionLock` semantics.
- `R7-F05`: Repeated race-barrier refactor suggestion; current test already asserts the one-winner contract and loser cleanup.

Watch signals for pass 8:
- Stop if only lease renewal, test-hook gating, strict stderr, or race-barrier refactors remain.

## Pass 8

- Source: `.tmp/phase7/coderabbit-pass8.md`
- Findings: 6
- Real applied: 4
- Skipped: 2
- Determination: stop at configured max-passes (8)
- Tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`

Applied finding IDs:
- `R8-F01`: `claude_native_line` fixture timestamps now use two-digit seconds formatting.
- `R8-F02`: Proposal crash-state prose now conditions orphan canonical cleanup on no live `SessionLock` and separates the no-preimage journal branch.
- `R8-F03`: Hookpoints recovery prose now keeps orphan canonical cleanup behind the live-lock check.
- `R8-F06`: `session import-replace` now runs startup recovery before reading/replacing input, with integration coverage through the import-replace CLI path.

Skipped finding IDs:
- `R8-F04`: Repeated lease-renewal request; gated-design churn for shared `SessionLock` semantics.
- `R8-F05`: Repeated canonical-schema/multimodal expansion; outside Rev 4 v1 supported surface, which marks unsupported records rather than extending schema.

Final disposition:
- Max passes reached. Remaining findings are documented design-scope expansions, not accepted implementation bugs for Rev 4 v1.
