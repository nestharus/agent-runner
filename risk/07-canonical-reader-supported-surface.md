# 07-canonical-reader-unification — Phase 4 Supported-Surface Risk Report

**Verdict: LOW.**

This gate reviews `proposals/07-canonical-reader-unification.md` and the
implementation on branch `07-canonical-reader-rca` (commits `f3dcf9a`,
`56eae86`, `b0d68fc`) against `main` for supported-surface impact. The fix
deletes `session_replace`'s private Claude/Codex parsers and routes both the
`agents session import-replace` preimage/postimage hash path and the
fresh-export verification through `session_export::*`, closing
`research/07-canonical-reader-divergence-rca.md` RC-1..RC-6.

The change is purely an internal-reader unification. The CLI surface
(`agents session import-replace`, `agents session export`,
`agents session locate`, `agents session schema-probe`,
`agents session pause-handshake` / `resume-handshake`) is unchanged in
shape: argument list, exit codes, structured-stderr error JSON, and receipt
JSON fields are all preserved. The receipt's `preimage_sha256` /
`postimage_sha256` *values* shift to the bytes that
`agents session export <id>` actually emits; this is the contract the
receipt was always supposed to satisfy (06-import-replace proposal §6 round-
trip oracle), and the prior values were demonstrably wrong (T1/T2/T4 were
red on `main` and `#[ignore]`'d at merge).

## Evidence

- **All target tests green.** With the fix applied,
  `cargo test --manifest-path src-tauri/Cargo.toml --test initiative_06_import_replace`
  reports **29 passed; 0 failed; 0 ignored** including the previously
  ignored T1, T2, T4. The 11-test `initiative_06_export` suite is also
  green. Full crate test run: every binary's `test result: ok.` (no
  failures, no remaining `ignored`). `cargo clippy --all-targets -- -D
  warnings` clean.
- **Public CLI surface diff is empty.** `git diff main..07-canonical-reader-rca`
  modifies `src-tauri/src/session_export/mod.rs`,
  `src-tauri/src/session_replace/mod.rs`,
  `src-tauri/src/session_replace/internal/mod.rs`, and
  `src-tauri/tests/initiative_06_import_replace.rs`. No file under
  `src-tauri/src/main.rs` (CLI dispatch), `src-tauri/src/state/`,
  `src-tauri/src/session_locate/`, `src-tauri/src/session_schema_probe/`,
  or `src-tauri/src/session_handshake/` is touched.
- **`agents session export` byte-for-byte unchanged.** The
  `session_export` edits are mechanical: `scan_jsonl(path)` is split into
  `scan_jsonl_bytes(bytes, path)` plus a `fs::read` wrapper, and two
  additional `pub fn`s (`read_canonical_transcript_from_bytes`,
  `canonical_jsonl_bytes`) are added for the in-process reuse. Existing
  `parse_claude_code_jsonl` / `parse_codex_rollout_jsonl` retain identical
  behavior; this is confirmed by the unchanged `initiative_06_export`
  suite passing.
- **Receipt JSON shape unchanged.** `ReplaceJournal` and `ReplaceReceipt`
  are not modified. The diff against
  `src-tauri/src/session_replace/mod.rs` adds `session_id` parameters to
  the internal helpers `canonical_records_from_provider_file`,
  `canonical_hash_from_provider_file`, etc., but no public function
  signature exposed to `main.rs` or the integration tests changes.
  `assert_public_session_replace_contract_types_are_reachable` in
  `src-tauri/tests/initiative_06_import_replace.rs:1189` still resolves.
- **Adjacent-path invariants preserved.** `agents resume`,
  `agents repl`, `agents repl --resume`, top-level `--resume`,
  `agents migrate-config`, `agents migrate-db`, `agents trace --json`,
  `agents session locate`, `agents session schema-probe`, `agents session
  pause-handshake`, `agents session resume-handshake`, the GUI / Tauri
  command surface, and direct CLI `claude` / `codex` are all UNCOUPLED
  from this diff. The `agents session export` path is REUSED via
  `session_export::read_canonical_transcript[_from_bytes]` inside the
  unified reader; its CLI shape is unchanged.

## Boundaries respected

The diff stays inside the proposal's declared scope:

- `internal::CanonicalRecord` and `internal::ContentChunk` are deleted
  (`src-tauri/src/session_replace/internal/mod.rs`); the public
  `session_replace::CanonicalRecord` re-export now points at
  `session_export::CanonicalRecord`. Matches D1.
- `internal::SessionMetadata` gains a `to_export_metadata()` helper, plus
  a free function `export_metadata_for(...)` in
  `src-tauri/src/session_replace/mod.rs` builds an `ExportSessionMetadata`
  from per-call args. Matches D2; D3 is satisfied by deleting
  `parse_claude_native`, `parse_codex_native`,
  `extract_claude_content`, `extract_codex_content`,
  `extract_text_items`, `source_value`, and `jsonl_data_lines`.
- `canonical_jsonl_bytes` becomes a thin wrapper over
  `session_export::canonical_jsonl_bytes`. Matches D4.
- `ClaudeCodeRenderer::render` and `CodexSessionRenderer::render` are
  preserved as the provider-native renderer path; only their
  `ContentChunk` consumption is updated to the new struct shape (text
  chunk via `chunk.text.as_deref().unwrap_or("")`). Matches D5.
- `From<ExportError> for ReplaceError` is implemented as a per-callsite
  `map_export_error` in `src-tauri/src/session_replace/mod.rs:1093`,
  collapsing each variant to the documented `ReplaceError` (covering
  `15 invalid-input-transcript`, `12 unsupported-storage`,
  `10/11/12` resolver codes, and operational fallbacks). Matches D6.
- `internal::SessionLock`, `internal::SessionMetadata` (the type
  itself), the journal lifecycle, and the recovery scan flow are
  unchanged in identity and ordering. Matches §3 out-of-scope.

## Findings

All findings are LOW and non-terminal under the supported-surface lens.
None blocks Phase 5/Phase 6.

### AIR-SUPPORTED-SURFACE-F01 — receipt hash values change for the same session

**Severity: LOW, non-terminal.**

After this fix, `receipt.preimage_sha256` and `receipt.postimage_sha256`
no longer equal the values the same `agents session import-replace
<session>` would have emitted on `main` HEAD `941e6e8` for transcripts
that exercise RC-1..RC-6 (Claude `summary`/`compaction-summary` records,
non-text Claude content, Codex turns whose `payload.id` is empty, Codex
transcripts whose pre-summary records are present, transcripts with
out-of-order timestamps, and Codex transcripts missing
`session_meta`). The new values equal `sha256(stdout of agents session
export <id>)`, which is the contract every prior round of the
06-import-replace risk track promised but did not enforce.

**Cohort impact:** an automated caller (cohort A `agent-harness`,
cohort B local automation) that snapshotted a prior `postimage_sha256`
and feeds it as `--preimage-sha256` into a follow-up
`agents session import-replace` will now observe exit `15`
`preimage-mismatch` instead of accepting the stored hash. Recovery is
trivial: re-derive via `agents session export <id> | sha256sum`, which
is the documented derivation path.

**Evidence:** RCA reproduction at `research/07-canonical-reader-divergence-rca.md`
captures the pre-fix receipt hash for T1/T2/T4 and the post-fix
agreement.

**Why bounded:** pre-fix hashes were demonstrably wrong (T1/T2/T4 were
`#[ignore]`'d at merge precisely because `cargo test` could prove they
disagreed with `agents session export`). No published contract documented
the pre-fix value as authoritative; 06-import-replace proposal §6 always
named "round-trip with `agents session export`" as the oracle. Cohort A
has not stored a hash from a green production run, because there has
never been one.

**What would close it:** nothing further. Documenting the receipt-hash
realignment in the next 07 PR description (or a CHANGELOG entry once
that surface exists) is sufficient at the supported-surface lens.

### AIR-SUPPORTED-SURFACE-F02 — new strict rejections from RC-5 / RC-6 on the import path

**Severity: LOW, non-terminal.**

`session_replace`'s preimage read step now invokes
`session_export::read_canonical_transcript`, which applies
`validate_timestamp_order` (RC-5) and the
`saw_matching_session_meta` check (RC-6). A session whose existing
on-disk transcript has out-of-order timestamps, or a Codex rollout
without a `session_meta` line whose `payload.id` matches the resolved
session, now exits `15 invalid-input-transcript` from
`agents session import-replace` at the preimage step instead of
proceeding with malformed canonical records.

**Cohort impact:** any cohort attempting to replace a session whose
existing transcript is malformed in those specific ways now fails
loudly. Such sessions already failed `agents session export` on `main`,
so the import path is now consistent with the export path; an
end-to-end `export | edit | import-replace` workflow has identical
acceptance criteria on both sides.

**Why bounded:** the strictness was always a property of the export
oracle. The renderer's output is RFC3339-ordered by construction
(records are emitted in input order; the input is already ordered); the
fresh-export verification step only fails if the renderer or the on-disk
state diverged from a valid provider transcript, in which case exit `1`
correctly indicates a corrupt mutation rather than user error. T1/T2/T4
exercise the success path; T11 (`t11_no_deletion_before_verify_…`)
covers the verification-mismatch behavior.

**What would close it:** nothing — alignment is the intended behavior
under the unified-reader contract.

### AIR-SUPPORTED-SURFACE-F03 — canonical-JSONL `source` field is now structurally typed and required

**Severity: LOW, non-terminal.**

`parse_canonical_jsonl` (`src-tauri/src/session_replace/mod.rs:671`)
deserializes user-supplied input as `session_export::CanonicalRecord`,
whose `source: RecordSource` field has no `#[serde(default)]`. The
prior `internal::CanonicalRecord` declared
`#[serde(default)] pub source: Value`, allowing any shape (including
absence). Post-fix, an `agents session import-replace` invocation
whose stdin/`--from-file` canonical-JSONL omits `source` or supplies a
non-`RecordSource`-compatible value exits `15 invalid-input-transcript`.

**Cohort impact:** the documented producer of canonical JSONL is
`agents session export`, which already emits the
`{storage_type, jsonl_path, line, byte_start, byte_end, sha256}` shape
on `main` (since #16). Hand-built canonical-JSONL inputs that previously
relied on `source` defaulting to `null` no longer parse; the fixture
helper `tests/fixtures/initiative_06_import_replace.rs:906`
(`source_json`) already emits the structurally correct shape, so test
coverage is intact.

**Why bounded:** the on-disk private journal at
`<state-data-dir>/replace_journal/session-<id>.canonical.jsonl` was
already written with the same six-field shape by the pre-fix code (via
the old `source_value` helper), so a pre-fix orphan canonical file
deserializes cleanly under the new struct typing. Cohort A's documented
input pipeline is `export | … | import-replace`, which never omits
`source`.

**What would close it:** nothing required at the supported-surface
lens. A one-paragraph note in `research/06-import-replace-contract.md`
or the import-replace help text explicitly listing `source` as a
required canonical-JSONL field would tighten the contract for
hand-builders, but the current bytes-from-export workflow is unaffected.

### AIR-SUPPORTED-SURFACE-F04 — `chain_id` synthesis in `export_metadata_for` is a forward-compat hazard

**Severity: LOW, non-terminal.**

`export_metadata_for` at `src-tauri/src/session_replace/mod.rs:1074`
synthesizes `chain_id: String::new()` rather than threading the
caller's `SessionMetadata::chain_id` through. Proposal §2 D2 asserts
"`chain_id` on the export side is informational only when the parsers
themselves don't read it; verify by code-reading," and code-reading at
`src-tauri/src/session_export/mod.rs` confirms this for the current
parsers (`chain_id` is on the metadata struct but unused inside
`parse_claude_code_jsonl_bytes` / `parse_codex_rollout_jsonl_bytes`).
A future change to either parser that consumed `chain_id` would
silently produce wrong canonical bytes through the
`session_replace`-mediated path while leaving the
`agents session export` path correct, re-introducing the
RC-class divergence with a different proximate cause.

**Cohort impact:** none today. The hazard is forward-compat only.

**Why bounded:** the four call sites that build
`ExportSessionMetadata` via `export_metadata_for` (preimage hash,
postimage hash check, recovery hash check, and fresh-export verify) all
pass through the same helper, so a future fix is one helper away.
`internal::SessionMetadata::to_export_metadata()` (added in
`src-tauri/src/session_replace/internal/mod.rs:46`) already threads
`chain_id` correctly and could replace the current free function, but
that is a Phase-6 ergonomics improvement rather than a supported-surface
break.

**What would close it (if elevated):** thread
`metadata.chain_id` through `export_metadata_for` (or have the four
call sites use `metadata.to_export_metadata()` instead), and add a unit
test that asserts a non-empty `chain_id` does not affect the canonical
bytes for either parser. Not required at LOW.

### AIR-SUPPORTED-SURFACE-F05 — pre-fix in-flight pending journals quarantine on first post-fix start

**Severity: LOW, non-terminal.**

Startup recovery (`recover_pending_replaces`,
`src-tauri/src/session_replace/mod.rs:534`) computes
`current_hash` via the new unified reader and compares it against the
journal's `postimage_sha256`. A pending journal written by pre-fix code
contains a `postimage_sha256` produced by the old parser; for any
session that exercises RC-1..RC-6, that value will not equal the
post-fix hash of the same on-disk transcript. The recovery match table
at lines 572-613 then routes the entry to the `_ =>
move_to_quarantine` arm, leaving the canonical-records file intact (the
orphan-cleanup pass at line 619 does not delete it because the
quarantined `.pending` exists).

**Cohort impact:** if a user crashed mid-replace on pre-fix `main` and
started the post-fix binary, the pending journal is quarantined rather
than re-applied. For crashes after DB commit but before journal
deletion, the DB rows are already correct; quarantining is harmless. For
crashes before DB commit (between rename and SQLite begin), the pending
replace is silently abandoned (DB unchanged, transcript reflects the
post-rename bytes), which differs from the pre-fix expectation that the
next start would re-apply the DB write.

**Why bounded:** import-replace shipped to `main` only days ago (#18 at
`941e6e8`); no production caller has a pre-fix in-flight journal on
disk. Cohort A (`agent-harness`) has not run a green import-replace
because T1/T2/T4 were red on `main`. The quarantine sink is the
documented recovery-ambiguity destination; cohort A is expected to
inspect quarantine on upgrade per `research/06-import-replace-contract.md`.

**What would close it (if elevated):** add a one-shot upgrade hook
that, on first post-fix start, scans `replace_journal/` for pending
journals whose `schema_version: 1` predates the canonical-reader
unification, and either (a) re-derives `postimage_sha256` from the
canonical-records file plus the on-disk transcript before deciding
recovery action, or (b) quarantines proactively with a structured-stderr
notice so the operator sees the upgrade-time transition. Not required at
LOW because no pre-fix production journals exist and the failure mode is
quarantine, not data loss.

## Verdict rationale

**Supported-surface delta is bounded.** The CLI surface (commands, args,
exit codes, structured-stderr shape, receipt JSON shape) is unchanged.
The single semantic change — receipt hash *values* — aligns with the
documented round-trip oracle (`agents session export`) instead of the
prior demonstrably-wrong values. Adjacent paths are UNCOUPLED or REUSED
without modification. The four cohorts (A `agent-harness`, B local
scripts, C `repl`/`resume` users, D GUI/Tauri, E direct CLI
`claude`/`codex`) are non-regressed; cohort A and B are *strengthened*
because the receipt hash now matches the export bytes they would diff
against in CI.

**Termination signals do not fire.** All five findings above are LOW
and non-terminal: none breaks a documented contract, none requires a
schema migration, none introduces an unbounded blast-radius. Pre-fix
in-flight state quarantines safely (F05); receipt hash realignment is a
contract correction (F01); two newly-strict rejections inherit the
export oracle's existing strictness (F02); the canonical-JSONL `source`
field tightening matches the export-producer shape that is already on
`main` (F03); the `chain_id` synthesis is a forward-compat note that
the proposal anticipates (F04).

**Recommendation.** Phase 4 closes at LOW. Phase 5 (hookpoints) and
Phase 6 (implementation finalization, including any of the
non-terminal closures above the team chooses to fold in) may proceed.
