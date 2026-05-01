# 1. Scope statement (Rev 4)

06-import-replace adds one mutating CLI surface:

```bash
agents session import-replace <session-id> [--from-file <path>] [--preimage-sha256 <hash>]
```

It reads canonical transcript JSONL, validates that every line belongs to the
same record family emitted by `agents session export`, and atomically replaces
the resolved provider transcript for the target session. This is the fifth
Initiative 06 feature in technical order because it composes the earlier
surfaces: locate metadata, schema-probe compatibility, export's canonical
reader, and pause-handshake's session lock (`initiatives/06-session-override-contract.md:38-56`,
`initiatives/06-session-override-contract.md:75-89`).

This proposal does not implement code. It defines command shape, input
validation, lock behavior, atomic replacement, state consistency, receipt JSON,
exit mapping, test intent, supported-surface notes, and residual crash states
for the later Phase 6 implementation. It consumes the approved current-state map
at `research/06-import-replace-problem-map.md`; this proposal's §1.1 register
replaces the draft register in that map.

**Rev 2 changes** (in response to Phase 4 Round 1 audit):

- §3 / §6 / §10 / §13: provider-native rendering instead of writing
  canonical bytes to transcript file. New `CanonicalToProviderRenderer`
  per storage type; `other` returns UnsupportedStorage; lossy records
  exit 15 (AIR-R1-F01).
- §4 / §6 / §8 / §9 / §13: durable replace journal at
  `<data-dir>/replace_journal/session-<id>.pending`; on-startup
  recovery scans journal and reconciles transcript-vs-DB state
  deterministically (AIR-R1-F02).
- §13: cite 06-pause-handshake's PR #17 as lock-primitive dependency;
  document that runner writers retrofit observation per their own
  timeline (AIR-R1-F03).
- §6 / §9 / §12: explicit canonical-record field-loss model
  (parent_turn_id, is_sidechain, is_compaction_boundary stored as
  NULL); future canonical-schema extensions preserve them (AIR-R1-F04).

**Rev 3 changes** (Round 2 audit AIR-R2-F01 closure):

- §4 / §6: expanded journal schema with resolved identity
  (chain_id, active_segment_id, provider_name, storage_type)
  + canonical_records_path; recovery rebuilds DB rows from
  journal-attached canonical file, not stale resolver state.
- §4: reordered flow so journal deletion is the LAST step,
  AFTER postimage_sha256 verification AND fresh export round-trip
  verification. Verification failures leave journal in place.
- §4: startup recovery flow specified for both hash-matches-postimage
  (re-apply DB) and hash-matches-preimage (no-op cleanup) and
  ambiguous (quarantine) cases.
- §9: 4 new T-rows for recovery scenarios (rename-only, ambiguous-hash,
  canonical-records-preserved, no-delete-before-verify).
- §8: side-effect contract updated to include canonical_records_path
  write + quarantine directory.

**Rev 4 changes** (Round 3 audit AIR-R3-F01 closure):

- §4 / §6 / §8: reordered acquire flow so canonical records first land
  in a per-process staging path (operation_uuid suffix), then are
  renamed to the per-session canonical-records path AFTER SessionLock
  acquired. Journal entry is written under lock. Eliminates pre-lock
  per-session journal publication race (AIR-R3-F01).
- §6: journal schema gains `operation_uuid` to associate journal with
  canonical-records file across rename.
- §9: T-concurrent-import-replace test row added (subprocess race;
  exactly one wins; loser leaves no per-session journal artifacts).
- §8: side-effect contract updated to include staging directory and
  operation_uuid usage.

What changes:

- Add `session import-replace` under the `session` subcommand group in
  `src-tauri/src/main.rs`.
- Add reusable Rust code under a new module at
  `src-tauri/src/session_replace/`.
- Add provider-native transcript renderers under
  `src-tauri/src/session_replace/`.
- Reuse 06-locate metadata, 06-export canonical parsing, and
  06-pause-handshake locking.
- Add state-DB helper(s) that replace derived `session_turns` rows for one
  provider/session and refresh chain/segment recency after the transcript file
  commit.
- Document the public CLI surface and receipt/error shape in `README.md`.

What does not change:

- `agents resume`, `agents repl --resume`, top-level `--resume`, `trace --json`,
  `migrate-db`, `migrate-config`, and cross-provider migration behavior are not
  changed by this proposal.
- Existing resolver semantics remain authoritative. `import-replace` does not
  invent a second ownership path.
- No provider is spawned, no auto-resume is attempted, no quota is refreshed, no
  account is selected, and no config file is edited.
- No GUI/Tauri command surface is added in v1.
- Provider-native JSONL is not a public input format. The stable input is only
  canonical JSONL from the export record family.
- The replacement transcript file does not store canonical JSONL in v1. It
  stores provider-native bytes rendered from canonical input for the resolved
  storage type.

## 1.1 Assumption register

This is the approved register validated and narrowed from
`research/06-import-replace-problem-map.md` §7. It replaces the draft register
there; do not maintain a competing register.

| ID | Assumption | Evidence | Invalidator | Used by |
| --- | --- | --- | --- | --- |
| A1 | Earlier Initiative 06 surfaces land before import-replace: `SessionMetadata`, schema-probe feature flags/read-only checks, export canonical reader, and pause-handshake locks. | Initiative sequence places import-replace last and says it composes locate + canonical reader + lock (`initiatives/06-session-override-contract.md:38-56`, `initiatives/06-session-override-contract.md:75-89`). | Import-replace is rebased directly onto this local worktree without those APIs. | §2, §4, §6, §8, §13. |
| A2 | `StateDb::resolve_resume` remains the sole ownership resolver. | Initiative 06 requires reuse of `StateDb::resolve_resume`; current resolver owns chain/segment selection (`src-tauri/src/state/db.rs:2577-2670`). | A preceding feature changes public session ownership away from chain/segment resume semantics. | §4 resolution flow; §5 exits `10`/`11`; §13 checklist. |
| A3 | Canonical input is the 06-export `CanonicalRecord` JSONL family, not provider-native JSONL and not `session_turns`. The on-disk replacement bytes are provider-native renderings of that canonical input. | Harness anti-scope excludes provider-native JSONL as input; export defines `CanonicalRecord` fields and parser selection (`03-session-import-replace.md`, `06-export/src-tauri/src/session_export/mod.rs:8-99`). | Export contract changes before import-replace starts, or a provider adds a native renderer with incompatible losslessness rules. | §3 validation/rendering; §6 replacement renderer; §9 tests. |
| A4 | Preimage/postimage hashes are over the canonical transcript byte stream emitted by the export serializer, not over summary DB rows. | `session_turns` lacks content/source hashes, while harness requests current canonical transcript hash (`src-tauri/src/state/db.rs:559-572`, `03-session-import-replace.md`). | Harness explicitly pins preimage to raw provider JSONL bytes instead of canonical export bytes. | D2 in §4; §6 receipt; §9 preimage tests. |
| A5 | Storage type remains the discriminator for supported replace behavior: `claude_code` and `codex_session` are supported; `other` is refused in v1. | Export parser supports Claude/Codex and rejects `other`; locate maps config storage to public types (`06-export/src-tauri/src/session_export/mod.rs:36-52`, `06-locate/src-tauri/src/session_metadata/mod.rs:23-39`). | A later storage API supplies a renderer for `other` before this feature lands. | §4 pre-mutation setup; §5 exit `12`; §6 renderer. |
| A6 | Current `running` invocation rows are not sufficient as replace locks. | Running rows may lack session id before provider spawn, can survive hard process death, and are not tokenized leases (`research/06-import-replace-problem-map.md` §5 #10-13). | A preceding feature turns invocation lifecycle into durable active-writer leases. | D1 in §4; §8 lock API; §12 residuals. |
| A7 | State consistency must replace `session_turns` and refresh existing chain/segment rows after file commit. | Resolver, trace, export metadata, and migration all read session tables; `session_turns` uniqueness is `(provider_name, session_id, turn_id)` (`src-tauri/src/state/db.rs:559-597`, `src-tauri/src/state/db.rs:2577-2764`). | A preceding feature introduces a new canonical transcript-state table that supersedes these rows. | D4 in §7; §7 state update; §9 tests. |
| A8 | Crash recovery cannot make filesystem rename and SQLite update one physical transaction, so import-replace v1 must use a durable pending-operation journal to make startup recovery deterministic. | Current migration renames before DB segment updates and has no pending-op table (`src-tauri/src/migration/mod.rs:206-231`); Phase 4 audit requires deterministic recovery for injected failures. | A prior feature lands an equivalent durable transcript-replace journal used by import-replace. | D5 in §8; §6 recovery API; §8 recovery; §9 tests. |
| A9 | `--from-file` and stdin are equivalent after bytes are loaded. | Harness specifies stdin unless `--from-file`; existing code has no transcript-oriented reader (`03-session-import-replace.md`, `research/06-import-replace-problem-map.md` §1 #26). | Harness adds format-specific behavior for file input. | §2 CLI; §3 input validation; §9 tests. |
| A10 | Postimage hash and mutation receipt are new observability fields, not reuse of an existing receipt. | Existing receipts carry ids, providers, paths, or lock tokens, not transcript hashes (`research/06-import-replace-problem-map.md` §6 #12). | Export/schema-probe gains a stable transcript hash field first. | §6 JSON receipt; §11 observability. |

## 1.2 Net-value statement

Yes: this reduces a concrete current-state risk on the supported CLI surface.
Today, the only way for `agent-harness` to replace session transcript material is
direct private mutation of provider JSONL and SQLite state. The harness request
is explicitly for a stable CLI replacement of that v1 adapter, with atomic file
replacement, preimage protection, and state-row consistency
(`03-session-import-replace.md`).

The blast radius is high enough to justify a narrow public contract: this command
mutates the provider transcript through canonical input and updates derived
session tables. The value remains positive because the mutation is bounded by
resolver ownership, schema compatibility checks, a session lock, canonical input
validation, same-directory atomic rename, and one receipt that records
before/after hashes. Rollback is also clear: avoid invoking the new subcommand or
revert the additive CLI/module work.

# 2. Subcommand surface

Add a child command under the `session` command group:

```text
session import-replace <session-id> [--from-file <path>] [--preimage-sha256 <hash>]
```

Clap shape:

```rust
enum SessionSubcommands {
    ImportReplace {
        session_id: String,
        #[arg(long = "from-file")]
        from_file: Option<PathBuf>,
        #[arg(long = "preimage-sha256")]
        preimage_sha256: Option<String>,
    },
}
```

Input source rules:

- If `--from-file <path>` is present, read all bytes from that path.
- If `--from-file` is absent, read all bytes from stdin.
- Empty input is invalid transcript input and exits `15`.
- Non-UTF-8 input is invalid transcript input and exits `15`.
- The loaded byte stream must be buffered before any transcript mutation.

CLI usage rules:

- `<session-id>` must parse as a full UUID before state mutation. Invalid parse
  exits `2 invalid-session-id`.
- `--preimage-sha256` must be exactly a lowercase or uppercase 64-character hex
  digest. A malformed digest exits `2` as usage error because it is an invalid
  option value, not a failed compare.
- `--from-file` and stdin are mutually exclusive by behavior only: when
  `--from-file` is present, stdin is ignored.
- Success stdout is one compact JSON receipt. Error stderr is one JSON object for
  all import-replace domain failures except clap's own structural usage text.

# 3. Input validation and provider-native rendering

D3 decision: validate canonical JSONL shape before state lookup, lock
acquisition, or transcript mutation. Validate session/provider consistency after
resolution. Invalid input exits `15`.

Validation rules:

1. Decode the loaded bytes as UTF-8.
2. Split as JSONL. A final trailing newline is optional.
3. Reject empty files and blank lines.
4. Parse each line as JSON. Any invalid JSON exits `15`.
5. Deserialize each line into the 06-export `CanonicalRecord` family:
   `session_id`, `provider_name`, `turn_id`, `role`, `timestamp`, `content`,
   `source`, and `unsupported_record` (`06-export/src-tauri/src/session_export/mod.rs:8-34`).
6. Reject records whose required fields are absent, wrong-typed, or violate the
   export canonical schema.
7. After resolution, reject any record whose `session_id` does not equal the resolved active
   provider session id.
8. After resolution, reject any record whose `provider_name` does not equal the resolved active
   provider name.
9. Validate timestamp order with the same export-side canonical check when it is
   applicable (`06-export/src-tauri/src/session_export/mod.rs:333-342`).
10. Reject a canonical transcript that consists only of `unsupported_record`
    lines. Such input is schema-compatible JSON but not a replaceable transcript
    for v1 state reconstruction.
11. After the storage type is known, pass every canonical record through the
    storage-specific `CanonicalToProviderRenderer`. If a record class cannot be
    represented losslessly in the target provider's native JSONL shape, exit
    `15 invalid-input-transcript` with a specific error code naming the
    unsupported record class.

Line numbering in errors is internal detail. The public stderr JSON should carry
`invalid-input-transcript`, a short message, and optionally `line` when the
failure belongs to one input line.

The validator returns `Vec<CanonicalRecord>` for state-row reconstruction plus a
canonical JSONL byte stream serialized by the same canonical serializer export
uses. That normalized canonical byte stream is used for input hashing,
preimage/postimage comparison, and DB reconstruction. It is not written directly
to provider transcript paths in v1.

The transcript file write contract is narrower: v1 writes provider-native bytes,
not canonical bytes. Import-replace renders canonical input into the resolved
storage type through a new renderer module:

```text
src-tauri/src/session_replace/
```

Renderer contract:

- `CanonicalToProviderRenderer::render(storage_type, records)` returns the exact
  provider-native byte stream to write to `jsonl_path`.
- `claude_code` maps `CanonicalRecord` into Claude JSONL records, including
  native fields such as `sessionId`, `type`, `uuid`, `message`, and compatible
  message payload structure.
- `codex_session` maps `CanonicalRecord` into Codex rollout records, including
  native structures such as `response_item.payload` where applicable.
- `other` returns `UnsupportedStorage`; it never guesses a native layout.
- Rendering is the dual of 06-export's provider parser. Every supported rendered
  record must round-trip through export back to the canonical input.
- Anti-scope for v1: lossy re-encoding. Multi-modal blocks, tool-use records, or
  any provider-specific record class that lacks a clean native representation for
  the target storage type fails before mutation with
  `15 invalid-input-transcript` and an error code such as
  `unsupported-record-class:tool-use`.

Normalizing through the export serializer keeps `agents session export <id>`
after replace byte-for-byte comparable to the import stream that import-replace
accepted, while provider CLIs continue to see their native transcript format on
disk.

# 4. Resolution, lock, and atomic replace flow

D1 decision: import-replace acquires its own session lock. It does not merely
check for locks created by pause-handshake. The lock target is the resolved
active provider/session pair, not the raw user input. If `SessionLock::acquire`
returns busy, import-replace exits `13 session-busy`.

D2 decision: use a two-phase replace with same-directory temp file, fsync,
atomic rename, and a durable replace journal. The authoritative commit window is
protected by `SessionLock`.

Pre-mutation setup, expanded from success-flow steps 1-2:

1. Parse `<session-id>` and `--preimage-sha256`.
2. Load input bytes from stdin or `--from-file`.
3. Validate canonical JSONL shape and normalize input bytes. Allocate an
   `operation_uuid` for all scratch paths in this process.
4. Open state/config using the same CLI default state root used by the earlier
   Initiative 06 commands.
5. Run schema compatibility preflight. If schema-probe says
   `safe_for_import_replace: false`, exit `14 schema-incompatible` before any
   transcript mutation.
6. Write normalized canonical records to the temporary operation-unique staging
   path:
   `<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`.
   This is not a per-session journal artifact.
7. Resolve session metadata through `SessionMetadata`. Map resolver not-found to
   `10`, ambiguity to `11`, and unsupported/non-mutable storage to `12`.
8. Reject `storage_type: "other"` with exit `12 unsupported-storage`.
9. Freeze the resolved identity for the operation: `session_id`, `chain_id`,
   `active_segment_id`, `provider_name`, `storage_type`, and `jsonl_path`.
10. Validate the input canonical JSONL against the resolved `session_id`,
   `provider_name`, and storage renderer support. Invalid input or known lossy
   render cases exit `15`; provider-native byte rendering happens in the
   protected success flow after the journal is published under lock.
11. Clean stale import-replace temp files for the resolved transcript path only:
    `<jsonl_path>.tmp-import-replace-*`. Do not sweep unrelated session temp
    files in the same directory, and skip files owned by another live replace
    operation.
12. Compute the expected `postimage_sha256` over the normalized canonical input
    stream. This is the canonical export hash expected after the provider-native
    file is committed and parsed back through export.

Handled failures after staging creation but before `SessionLock` acquisition
unlink the operation-unique staging file before exit. They never publish a
per-session journal or canonical-records path.

Success flow:

1. Validate input and write normalized canonical records to the temporary
   operation-unique staging path:
   `<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`.
   This is per-process scratch state and never collides with another contender.
2. Resolve session ownership through `SessionMetadata` and freeze the resolved
   identity for the operation.
3. Acquire `SessionLock` for the resolved active provider/session pair with
   owner `"import-replace"`. Busy maps to exit `13`; before returning, unlink
   the staging file. No per-session journal artifact has been published.
4. Now under lock, atomically rename the staging canonical file to its
   journal-attached per-session path:
   `<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl`.
   Fsync the `replace_journal` directory after the rename. Only the lock owner
   can land canonical records at this path.
5. Write the durable pending journal entry under lock at
   `<state-data-dir>/replace_journal/session-<session_id>.pending`. The journal
   includes the frozen resolved identity, `operation_uuid`,
   `canonical_records_path`, `postimage_sha256`, and `expected_turn_count`.
6. Read the existing provider transcript, parse it through the canonical reader,
   compute `preimage_sha256`, record that hash in the journal with an atomic
   rewrite/fsync, and verify it against `--preimage-sha256` when given. A
   preimage mismatch exits `15` before transcript mutation and leaves the
   journal plus canonical records file in place for inspection.
7. Render canonical records to provider-native bytes, write them to
   `<jsonl_path>.tmp-import-replace-<operation_uuid>` in the same directory, and
   fsync the temp file.
8. Atomically rename `<jsonl_path>.tmp-import-replace-<operation_uuid>` to
   `jsonl_path` and fsync the parent directory.
9. Begin a SQLite transaction. Replace `session_turns` rows for this
   provider/session from `canonical_records_path` and refresh segment/chain
   recency, but do not commit yet.
10. Compute `postimage_sha256` by reading the newly written transcript file
    through the canonical reader. Verify it matches the journal's recorded
    `postimage_sha256`. If it mismatches, roll back the SQLite transaction, exit
    `1 operational-error`, and leave the journal plus canonical records file in
    place for operator inspection.
11. Run fresh export verification: parse the new transcript through the canonical
    reader and compare the resulting canonical bytes to
    `canonical_records_path`. If it mismatches, roll back the SQLite transaction,
    exit `1 operational-error` with a specific fresh-export verification error,
    and leave the journal plus canonical records file in place.
12. Only after step 11 succeeds, commit the SQLite transaction.
13. Delete the journal entry and canonical records file with idempotent unlink,
    then fsync the `replace_journal` directory. This is the last durable cleanup
    step.
14. Release `SessionLock`.
15. Emit one receipt JSON on stdout and exit `0`.

The under-lock preimage check is intentional. It preserves the harness preimage
behavior while closing the time-of-check/time-of-use gap between early
validation and the protected commit window. Any failure in success-flow steps
6-12 leaves the journal plus canonical records file in place; that journal is
the recovery signal. A lock-busy contender exits after deleting only its own
staging file, because it never publishes a per-session journal path.

Journal format:

```json
{
  "schema_version": 1,
  "operation": "import-replace",
  "started_at": "2026-04-30T12:34:56Z",
  "session_id": "9e69e8cc-616d-4640-bf1d-96f5391b1a2e",
  "chain_id": "33d5e6ec-f8a5-4cf8-9f65-9fe7ec3f6a0a",
  "active_segment_id": 42,
  "provider_name": "claude2",
  "storage_type": "claude_code",
  "jsonl_path": "/home/me/.claude2/projects/.../9e69e8cc.jsonl",
  "operation_uuid": "0b67fdde-92c1-45d1-832c-4b1fbf5c8306",
  "preimage_sha256": "...",
  "postimage_sha256": "...",
  "canonical_records_path": "/home/me/.local/share/agent-runner/replace_journal/session-9e69e8cc-616d-4640-bf1d-96f5391b1a2e.canonical.jsonl",
  "db_state_pending": true,
  "expected_turn_count": 18
}
```

The journal is private implementation state, not a public receipt log.
Its hashes are canonical export hashes for recovery comparison, not raw
provider-native file-byte hashes.
`chain_id`, `active_segment_id`, `provider_name`, and `storage_type` are
resolved before transcript mutation and frozen in the journal. The
`operation_uuid` identifies the staging source that was atomically renamed into
`canonical_records_path` under lock and is required so crash recovery can
associate the journal with the canonical-records file. The
`canonical_records_path` file is published before the transcript rename and is
the recovery source of truth for rebuilding `session_turns`; recovery must not
re-read the postimage transcript and infer DB rows from provider-rendered bytes.
`storage_type` is limited to `claude_code` or `codex_session` in v1 because
`other` is rejected before journal creation.

DB finalization transaction contents:

1. Replace `session_turns` rows for the resolved provider/session with rows
   reconstructed from `canonical_records_path`.
2. Refresh `session_chain_segments.last_used_at` and `last_turn_id` for the
   active segment.
3. Refresh `session_chains.last_used_at` for the owning chain.

The filesystem journal entry and canonical records file are deleted only after
postimage verification, fresh export verification, and SQLite commit all
succeed. The journal directory is fsynced before the success receipt.

# 5. Exit codes

D7 decision: use the harness exit namespace exactly.

| Exit | Error code | Producing condition | Mutation guarantee |
| --- | --- | --- | --- |
| `0` | none | Transcript file replaced, state consistency transaction committed, receipt emitted. | Transcript and DB update both completed. |
| `1` | `operational-error` or specific internal code | Unexpected I/O, DB, serialization, fsync, rename, lock-store operational error, or post-rename verification error not covered below. | Depends on phase; stderr must state whether rename happened when known. |
| `2` | `invalid-session-id` or clap usage | Invalid UUID, invalid flag value, missing args, duplicate/unknown args. | No mutation. |
| `10` | `session-not-found` | Resolver cannot find a chain/active session for the input. | No mutation. |
| `11` | `ambiguous-session` | Resolver returns ambiguous owner. | No mutation. |
| `12` | `unsupported-storage` | No supported storage, missing/unavailable transcript path, `other` storage, non-UTF-8 path, unsupported renderer, or metadata says the session is not file-backed/mutable. | No mutation. |
| `13` | `session-busy` | `SessionLock::acquire` reports an unexpired lock for the resolved provider/session pair, or exclusive ownership cannot be established. | No mutation by this process. |
| `14` | `schema-incompatible` | Schema-probe/read-only compatibility says import-replace is unsafe for this state DB or required feature flags are absent. | No mutation. |
| `15` | `invalid-input-transcript` or `preimage-mismatch` | Input canonical JSONL is malformed, schema-invalid, session/provider mismatched, not reconstructable for v1 state, contains a record class that cannot be rendered losslessly to the target provider format, or `--preimage-sha256` does not match the current canonical transcript. | No mutation when detected before rename; no DB mutation if detected after lock before write. |

Errors must not print partial receipts. Success must not print non-JSON human
text on stdout.

# 6. Reusable API and receipt JSON schema

Reusable implementation modules:

- `src-tauri/src/session_replace/` owns the CLI command handler,
  validation orchestration, preimage checks, journal lifecycle, file replace
  primitive, DB update call, and receipt/error mapping.
- `src-tauri/src/session_replace/` owns
  `CanonicalToProviderRenderer`, with implementations for `claude_code` and
  `codex_session`. `other` returns `UnsupportedStorage`.
- The renderer API accepts canonical records and returns provider-native bytes
  plus structured unsupported-record-class errors. It does not accept
  provider-native input and does not expose canonical bytes as the write target.
- The DB update API accepts the resolved provider/session identity, the
  replaced `jsonl_path`, and the canonical records. It writes only fields present
  in the canonical record schema: `provider`, `session_id`, `turn_id`,
  `timestamp`, and `role`. Fields not present in `CanonicalRecord`
  (`parent_turn_id`, `is_sidechain`, `is_compaction_boundary`) are intentionally
  written as `NULL` or schema defaults in `session_turns`.
- The journal API first writes normalized canonical records to
  `<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`.
  After `SessionLock` is acquired, it atomically renames that staging file to
  `<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl` and
  writes `<state-data-dir>/replace_journal/session-<session_id>.pending` under
  the lock. The pending entry contains the resolved `chain_id`,
  `active_segment_id`, `provider_name`, `storage_type`, `jsonl_path`,
  `operation_uuid`, canonical hashes, and `canonical_records_path`.
- The recovery API scans `<state-data-dir>/replace_journal/` on startup and
  reconciles pending replace operations before normal session resolution work
  relies on derived rows.

Startup recovery contract:

1. Scan `<state-data-dir>/replace_journal/` for files named
   `session-<session_id>.pending`.
   Also scan for `session-<session_id>.canonical.jsonl` files without a
   matching pending journal. These are orphan side files from a crash between
   the under-lock canonical rename and pending-journal write; if no live
   `SessionLock` exists for the session, delete them, fsync `replace_journal/`,
   and do not mutate transcript or DB state. If a live lock exists, leave the
   side file for the active owner.
2. Read journal JSON; extract resolved identity (`chain_id`,
   `active_segment_id`, `provider_name`, `storage_type`, `jsonl_path`), hashes,
   `operation_uuid`, and `canonical_records_path`. Ignore files whose
   `operation` is not `"import-replace"` or whose `schema_version` is
   unsupported. If a pending journal lacks a completed `preimage_sha256` because
   the process died before reading the original transcript, treat it as a
   pre-rename no-op: delete the journal and canonical records file, fsync the
   journal directory, and do not mutate DB state.
3. Read the transcript at `jsonl_path` through the storage parser and canonical
   serializer, then compare that canonical export SHA-256 to the journal hashes.
   The journal hashes remain canonical hashes even though the file bytes are
   provider-native.
4. If the transcript hash equals `postimage_sha256`, the rename landed.
   Re-apply DB updates idempotently from `canonical_records_path`: replace
   `session_turns` rows for `(provider_name, session_id)` and update the frozen
   active segment's `last_used_at` / `last_turn_id` plus the owning chain's
   `last_used_at`. Then delete the journal entry and canonical records file and
   fsync the journal directory.
5. If the transcript hash equals `preimage_sha256`, the rename never landed or
   was rolled back. Delete the journal entry and canonical records file and
   fsync the journal directory. Do not mutate DB state.
6. If the transcript hash matches neither hash, or the transcript cannot be
   parsed into canonical export bytes, the state is ambiguous. Move the journal
   to `<state-data-dir>/replace_journal/quarantine/`, leave the canonical
   records file in place for manual inspection, and log
   `"import-replace recovery: ambiguous transcript state for session X; manual recovery needed"`.
   Leave the transcript and DB untouched.

The future `agents migrate-db --recover` flag, or a separate
`agents session import-replace --recover`, is anti-scope for v1. Rev 4 only
requires on-startup auto-recovery.

D6 decision: stdout on success is exactly one JSON object with the harness
fields plus stable operation vocabulary:

```json
{
  "session_id": "9e69e8cc-616d-4640-bf1d-96f5391b1a2e",
  "provider_name": "claude2",
  "storage_type": "claude_code",
  "operation": "import-replace",
  "preimage_sha256": "...",
  "postimage_sha256": "...",
  "jsonl_path": "/home/me/.claude2/projects/.../9e69e8cc.jsonl",
  "state_updated": true,
  "committed_at": "2026-04-30T12:34:56Z"
}
```

Required success fields:

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `session_id` | string UUID | yes | Resolved active provider session id. |
| `provider_name` | string | yes | Resolved active provider/account name. |
| `storage_type` | string enum | yes | `claude_code` or `codex_session` in v1. `other` is an error for replace. |
| `operation` | string | yes | Literal `import-replace`. |
| `preimage_sha256` | string | yes | SHA-256 hex of canonical export stream immediately before replace under lock. |
| `postimage_sha256` | string | yes | SHA-256 hex of canonical export stream after replace and DB update. |
| `jsonl_path` | string path | yes | Canonical absolute UTF-8 path that was replaced. |
| `state_updated` | boolean | yes | `true` only after the DB consistency transaction commits. |
| `committed_at` | string timestamp | yes | UTC ISO-8601/RFC3339 timestamp taken after DB commit. |

Hash details:

- Hash bytes are the canonical JSONL bytes that `agents session export <id>`
  would emit for the same resolved session at that point in time.
- Hashing never uses `session_turns` summaries.
- Hashing never uses caller-provided input bytes before normalization.
- `postimage_sha256` must match a fresh export after replace.

# 7. DB consistency update

D4 decision: choose D4a. Import-replace replaces all `session_turns` rows for
the resolved `(provider_name, session_id)` and refreshes existing chain/segment
metadata. It does not rely on a later ingestion scan as the primary consistency
mechanism.

State update transaction after the filesystem rename:

1. Delete `session_turns` rows for `provider_name = resolved.provider_name` and
   `session_id = resolved.session_id`.
2. Insert one summary row per canonical turn that maps cleanly to the v1
   canonical-record fields.
3. Store canonical-record fields only: provider/provider_name, `session_id`,
   `turn_id`, `timestamp`, and `role`.
4. Intentionally drop fields not present in `CanonicalRecord`:
   `parent_turn_id`, `is_sidechain`, and `is_compaction_boundary` are written as
   `NULL` or schema default values in `session_turns`. This is documented data
   loss in v1; downstream features such as resume and trace should not rely on
   these fields after a replace.
5. Set `source_file` to the replaced `jsonl_path` when the current schema/helper
   supports it; otherwise keep existing ingest helper behavior if the column is
   not meaningful in this branch.
6. Update `session_chains.last_used_at` for the resolved chain to the newest
   imported turn timestamp, or commit time if no usable turn timestamp exists.
7. Update the active `session_chain_segments` row for the resolved
   `(chain_id, provider_name, session_id)` so its `last_turn_id` reflects the
   latest imported turn id and its `last_used_at` reflects the newest imported
   turn timestamp, or commit time if no usable turn timestamp exists.
8. Do not close or reopen chain segments. Replace is content mutation of the
   existing active segment, not a migration to a new segment.
9. Do not create new chains. If no resolver-visible active segment exists, the
   command already exited `10`.

Unsupported canonical records may be retained in provider-native transcript
bytes only when the renderer has a lossless native encoding for that record
class. They do not produce `session_turns` rows unless the shared parser can map
them into a known turn summary. All-unsupported input exits `15` before
mutation.

If the SQLite transaction fails after the rename, the command exits `1`, leaves
the durable journal entry in place, and reports that startup recovery will
reconcile the pending DB update when the transcript hash is unambiguous. It must
not attempt to rename the old file back.

Future canonical-record schema extensions can preserve
`parent_turn_id`, `is_sidechain`, and `is_compaction_boundary`; v1 does not infer
them from provider-native payloads during import-replace.

# 8. Locking and crash recovery

Lock behavior:

- Import-replace creates a `SessionLock` rooted at the same lock directory used
  by 06-pause-handshake.
- It calls `SessionLock::acquire` for the resolved active provider/session pair.
- Busy maps to exit `13`.
- Operational lock-store failures map to exit `1`.
- The lock is released on normal success and on handled failures when the
  process remains alive.
- If the process dies, the lock lease expires according to `SessionLock`
  semantics; import-replace does not invent a second lock format.

Temp-file convention:

```text
<jsonl_path>.tmp-import-replace-<operation_uuid>
```

Durable journal side effects:

- Before acquiring the session lock and before writing any per-session journal
  artifact, import-replace writes normalized canonical records only to
  `<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`.
  This staging path is operation-unique scratch state.
- If `SessionLock::acquire` returns busy, import-replace unlinks only its
  staging file and exits `13`; it must not create or modify
  `<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl` or
  `<state-data-dir>/replace_journal/session-<session_id>.pending`.
- Other handled failures after staging creation but before lock acquisition also
  unlink only the staging file before exit.
- After acquiring the session lock, import-replace atomically renames the
  staging file to
  `<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl`.
- It then writes the pending journal entry under the same lock at
  `<state-data-dir>/replace_journal/session-<session_id>.pending`.
- The journal records `schema_version`, `operation`, `started_at`,
  `session_id`, `chain_id`, `active_segment_id`, `provider_name`,
  `storage_type`, `jsonl_path`, `operation_uuid`, `preimage_sha256`,
  `postimage_sha256`, `canonical_records_path`, `db_state_pending`, and
  `expected_turn_count`.
- The canonical records file, journal file, and `replace_journal` directory are
  fsynced after the under-lock staging rename and before transcript mutation.
- Failures in the protected flow after lock acquisition and before SQLite
  commit, including under-lock preimage mismatch, postimage hash mismatch, and
  fresh export verification mismatch, leave the journal entry and canonical
  records file in place.
- After postimage verification, fresh export verification, and SQLite commit all
  succeed, import-replace deletes the journal entry and canonical records file
  and fsyncs the `replace_journal` directory before emitting the receipt.
- Recovery quarantine lives under
  `<state-data-dir>/replace_journal/quarantine/`.
- If the process exits after rename and before journal deletion, the journal and
  canonical records file are the durable recovery signal.

Crash states:

1. Crash before staging rename: no per-session journal artifact exists. A stale
   operation-unique file may remain under `replace_journal/staging/`; future
   startup or import-replace runs may unlink stale staging files by age and
   operation UUID.
2. Crash after staging rename before pending journal write: startup recovery
   sees an orphan `session-<session_id>.canonical.jsonl`. It deletes the side
   file only when no live `SessionLock` exists for that session, fsyncs
   `replace_journal/`, and does not mutate transcript or DB state. If a live
   lock exists, recovery leaves the side file for the active owner.
3. Crash after staging rename and pending journal write, but before
   `preimage_sha256` is recorded or transcript temp write begins: startup
   recovery treats the no-preimage pending journal as a pre-rename no-op,
   deletes the journal and canonical records file, fsyncs `replace_journal/`,
   and does not mutate DB state.
4. Crash after transcript temp write before transcript rename: temp file lingers; future
   import-replace startup may unlink stale temp files with this feature prefix.
5. Crash after transcript temp fsync before transcript rename: same as #4.
6. Crash after transcript rename before DB update: startup recovery sees `jsonl_path` whose
   canonical export hash matches `postimage_sha256`, re-applies the DB update
   idempotently from `canonical_records_path`, refreshes segment recency,
   deletes the journal and canonical records file, and leaves a deterministic
   committed state.
7. Crash during DB transaction: SQLite rolls the transaction back or commits it
   according to SQLite durability. Startup recovery sees the postimage and
   re-applies the DB update idempotently from `canonical_records_path` if
   needed, then deletes the journal and canonical records file.
8. Crash after DB commit before journal deletion or receipt: startup recovery
   sees the postimage, re-applies the same DB update idempotently from
   `canonical_records_path`, deletes the journal and canonical records file, and
   the caller can export/hash to discover the committed postimage.
9. Startup recovery sees the transcript's canonical export hash matching
   `preimage_sha256`: rename did not happen or was rolled back; recovery deletes
   the journal and canonical records file only.
10. Startup recovery sees a canonical export hash matching neither preimage nor
   postimage, or cannot parse the provider-native transcript at all: recovery
   logs the required manual-recovery warning, moves the journal entry to
   `replace_journal/quarantine/`, preserves the canonical records file for
   inspection, and leaves state untouched for explicit operator cleanup. The
   recovery CLI flag is not part of v1.

D5 decision: add the durable pending-operation journal in v1. Stale temp files
are still cleaned opportunistically by prefix/age, but the post-rename/pre-DB
gap is closed by startup recovery.

Directory fsync:

- Fsync the operation-unique staging canonical file after writing.
- Fsync `replace_journal/staging/` after deleting a staging file on lock busy
  when the platform supports directory fsync.
- Fsync `replace_journal/` after renaming staging canonical records to the
  per-session canonical records path and after writing or rewriting the pending
  journal.
- Fsync the temp file after writing.
- Fsync the parent directory after rename.
- Treat fsync failures as operational errors.
- On platforms where directory fsync is unavailable, use the strongest local
  equivalent and document the platform caveat in code comments and tests.

# 9. Test-intent track

## 9.1 Test-intent track

| Change risk or verification risk | Intended behavior / acceptance condition | Level | Fixture source / application point | Assumption link | Expected observable signal | Residual risk |
| --- | --- | --- | --- | --- | --- | --- |
| Valid stdin replace | Canonical JSONL from stdin renders to provider-native bytes and replaces an idle supported Claude fixture. | end-to-end | Temp state DB, providers/sessions config, located JSONL, canonical fixture from export. | A1, A3, A5 | Exit `0`; receipt hashes present; provider transcript path contains Claude-native JSONL; export after replace matches import. | Does not cover Codex renderer. |
| Valid `--from-file` replace | `--from-file packed.jsonl` behaves identically to stdin. | end-to-end | Same fixture as stdin with file input. | A9 | Exit `0`; stdout receipt shape identical except hashes/timestamps. | None beyond file I/O platform behavior. |
| Preimage mismatch | Wrong `--preimage-sha256` exits `15` after lock acquisition but before transcript write. | component + e2e | Fixture with known existing export hash. | A4 | Exit `15`; stderr `preimage-mismatch`; transcript mtime/hash unchanged; journal and `canonical_records_path` remain for deterministic preimage cleanup. | Concurrent external writer outside locks remains outside supported surface. |
| Lock busy | Existing pause-handshake lock causes exit `13`. | integration | Use 06-pause-handshake `SessionLock` to acquire same session id before command. | A6 | Exit `13`; staging file unlinked; no per-session journal/canonical file, transcript, or DB mutation by this process. | Does not prove every non-cooperating provider process is detected. |
| Malformed JSONL | Invalid JSON, blank line, non-UTF-8, or missing canonical fields exit `15`. | unit + e2e | Validator unit cases plus CLI stdin cases. | A3 | Exit `15`; stderr line when available; no mutation. | Exact error messages not stable. |
| Session/provider mismatch | Valid canonical records for another session/provider exit `15`. | component | Canonical fixture with mismatched ids. | A2, A3 | Exit `15`; no mutation. | None. |
| Unsupported storage | `other` storage or missing renderer exits `12`. | integration | Metadata fixture from locate with no supported storage. | A5 | Exit `12`; no lock/write. | Future storage support may alter expected result. |
| Unsupported record class | Canonical records that cannot be represented losslessly in the target provider format exit `15`. | component | Canonical multi-modal/tool-use fixture without clean Claude or Codex native rendering. | A3, A5 | Exit `15`; stderr names unsupported record class; no journal, temp file, transcript, or DB mutation. | Exact unsupported class taxonomy may grow. |
| Schema incompatible | Probe says unsafe for import-replace exits `14`. | component | Temp DB missing required tables/indexes or feature flag fixture. | A1 | Exit `14`; no input write. | Depends on schema-probe helper shape after merge. |
| T-concurrent-import-replace | Spawn two subprocesses calling import-replace on the same `session_id`; exactly one process wins the lock and the other exits busy. | integration | Shared temp state DB and transcript fixture; two valid but distinguishable canonical inputs launched concurrently. | A6, A8 | Exactly one returns `0` with valid receipt and final transcript/export matching the winner. The loser returns `13 session-busy`, unlinks its staging file, leaves no per-session journal/canonical files in `<session>.canonical.jsonl` or `<session>.pending`, and performs no transcript mutation. | Scheduler timing can be nondeterministic; test needs a barrier or lock-acquire hook to make the race observable. |
| Atomic temp/rename | Failure injected before temp, after temp, after fsync, before rename, and after rename produces deterministic state. | component | Replace primitive with injectable failure points and journal inspection. | A8 | Pre-rename failures leave target unchanged; recovery deletes preimage-matching journals and canonical records files; post-rename failures recover DB from postimage and delete recovery artifacts. | Real OS crash cannot be perfectly simulated. |
| Journal post-rename recovery | Crash after rename and before DB update is reconciled on next startup. | integration | Inject failure immediately after rename/fsync with seeded `session_turns`. | A8 | Startup scan finds pending journal, transcript canonical export hash matches postimage, DB rows are replaced idempotently from `canonical_records_path`, journal and canonical records file are deleted. | Requires startup hook to run in test harness. |
| T-recovery-rename-only | Kill process between rename and DB commit; restart recovers derived state from the journal-attached canonical records file. | integration | Injectable kill point after rename/fsync and before SQLite commit, with stale `session_turns`. | A8 | Startup scan finds postimage hash, replaces `session_turns` from `canonical_records_path`, refreshes frozen segment, deletes journal and canonical records file. | Requires deterministic crash injection around transaction boundary. |
| Journal pre-rename recovery | Crash before rename does not mutate DB and clears stale journal. | integration | Inject failure after journal write but before temp rename. | A8 | Startup scan sees transcript canonical export hash matching preimage, leaves transcript/DB unchanged, deletes journal and canonical records file. | External mutation between crash and startup becomes ambiguous case. |
| Journal ambiguous recovery | Transcript canonical export hash matching neither preimage nor postimage is quarantined. | component | Mutate transcript after pending journal creation. | A8 | Startup scan logs warning, moves journal to quarantine, preserves canonical records file, does not rewrite transcript or DB. | Operator recovery command is anti-scope. |
| T-recovery-ambiguous-hash | Kill process with pending journal, manually corrupt transcript, restart. | integration | Pending journal plus transcript bytes edited so canonical export hash matches neither preimage nor postimage, or parser rejects it. | A8 | Startup scan moves journal to `replace_journal/quarantine/`, logs manual-recovery warning, leaves transcript and DB untouched. | Manual repair remains outside v1. |
| T-recovery-canonical-records-preserved | Canonical records file survives crash and remains byte-for-byte equal to normalized input. | component + integration | Inject crash after staging-to-session canonical rename and after transcript-rename-before-commit. | A8 | `canonical_records_path` exists after crash; content equals normalized canonical JSONL and is used for DB recovery. | Does not validate future canonical schema extensions. |
| T-no-deletion-before-verify | Inject postimage hash mismatch after rename; command exits operationally without deleting recovery artifacts. | component | Replace primitive test hook mutates rendered transcript or expected hash before postimage verification. | A8 | Exit `1`; stderr names postimage verification failure; journal and `canonical_records_path` still exist; SQLite transaction is not committed. | Requires a targeted fault injection hook. |
| DB row replacement | Existing `session_turns` for the session are deleted/reinserted; unrelated sessions remain unchanged. | component | Seed two sessions under same provider and replace one. | A7 | Row counts and latest turn ids match imported canonical records. | Summary mapping from unsupported records remains intentionally limited. |
| DB metadata loss is explicit | Imported rows intentionally lose `parent_turn_id`, `is_sidechain`, and `is_compaction_boundary`. | component | Seed existing rows with parent/sidechain/compaction metadata, then import canonical records. | A7 | Reinserted rows have canonical fields populated and absent fields set to `NULL` or defaults. | Future canonical schema may change this expectation. |
| Chain/segment refresh | Existing active segment remains same row; `last_turn_id` and chain `last_used_at` refresh. | component | Seed active chain/segment with old turns. | A7 | Same chain/segment identity; refreshed fields only. | Does not prove future resolver changes. |
| Postimage round-trip | `agents session export <id>` after success emits the imported canonical transcript. | end-to-end | Claude and Codex fixtures where available. | A3, A4, A5 | Export hash equals receipt `postimage_sha256` even though on-disk bytes are provider-native. | If Codex renderer deferred, Codex test becomes explicit unsupported-storage test. |

New fixture infrastructure is expected for CLI-level Rust integration tests:
temp state DB seeding, config-root builder, locator scripts, canonical export
fixtures, and injectable failure hooks in the replace primitive. Phase 6b should
index these helpers explicitly.

# 10. README updates

Update `README.md` in the same style as the current CLI synopsis and session
inspection sections:

- Add `session import-replace <session-id> [--from-file <path>] [--preimage-sha256 <hash>]`
  under Subcommands near the sibling `session` commands.
- Add a short "Replacing a Session Transcript" section after export.
- State that input is canonical JSONL emitted by `agents session export`; provider
  native JSONL is not accepted.
- State that the transcript file is written in provider-native format rendered
  from canonical input, so provider CLIs continue to recognize their own
  transcript files.
- Show stdin and `--from-file` examples.
- Document the receipt JSON fields exactly as §6.
- Document exit codes `0`, `1`, `2`, `10`, `11`, `12`, `13`, `14`, and `15`.
- Document that import-replace acquires a session lock and returns
  `session-busy` for existing locks.
- Document that preimage hashes are canonical export hashes.
- Document that lossless rendering is required; unsupported canonical record
  classes fail with `15 invalid-input-transcript`.
- Document that startup recovery scans the durable replace journal and
  deterministically reconciles post-rename/pre-DB-commit crashes.
- Clarify that manual recovery commands such as `agents migrate-db --recover`
  are anti-scope for v1.

Example:

```bash
agents session export 9e69e8cc-616d-4640-bf1d-96f5391b1a2e > before.jsonl
HASH="$(sha256sum before.jsonl | awk '{print $1}')"
agents session import-replace 9e69e8cc-616d-4640-bf1d-96f5391b1a2e \
  --from-file packed.jsonl \
  --preimage-sha256 "$HASH"
```

# 11. Supported-surface track

## 11.1 Supported-surface track

Deployment mode: local CLI binary only. No GUI command, no Tauri frontend
surface, no daemon, and no server.

Customer cohort: `agent-harness` is the primary consumer, replacing its v1
direct mutation of `state.db` and provider JSONL. Secondary consumers are local
automation scripts that already use `agents session export` and need a stable
write-back primitive.

Adjacent public/user-reachable paths and blast-radius notes:

- `agents session locate` remains read-only metadata and supplies the reusable
  ownership/path API.
- `agents session schema-probe` remains the read-only compatibility gate and
  advertises whether import-replace is safe.
- `agents session export` remains the canonical reader and round-trip oracle.
- `agents session pause-handshake` / `resume-handshake` remain lock surfaces;
  import-replace observes and acquires the same lock primitive.
- `agents resume`, `agents repl --resume`, and top-level `--resume` keep using
  resolver semantics; they are not launched by import-replace.
- `trace --json` remains invocation-tree scoped and does not read receipts.
- `migrate-db` remains an explicit DB maintenance command; import-replace does
  not call it automatically.
- Cross-provider migration keeps using `migration::migrate_chain_segment` unless
  a later proposal deliberately refactors it onto the replace primitive.

Migration path: no user state one-shot is required before using this command
when schema-probe reports compatibility. Existing partial DBs that cannot be
resolved by `StateDb::resolve_resume` remain not-found.

Rollback path: avoid invoking the new subcommand or revert the additive
CLI/module work. Successful replacements are real transcript mutations; rollback
of a particular session requires importing a prior exported transcript with the
current postimage as preimage.

Observability: the receipt JSON is the durable caller-facing success signal, and
the private replace journal plus its `canonical_records_path` file are the
crash-recovery signal. The command does not create invocation rows, run provider
commands, refresh quotas, or emit GUI events. Stderr JSON is the failure signal.

# 12. Implementation residuals

Known residuals Phase 4 should evaluate rather than treat as accidental
omissions:

- The file rename and DB update are not one physical transaction. The durable
  journal closes the deterministic recovery requirement, but there is still no
  single underlying atomic transaction spanning filesystem and SQLite.
- Manual recovery commands such as `agents migrate-db --recover` or
  `agents session import-replace --recover` are not implemented in v1. Only
  on-startup auto-recovery is in scope.
- Running invocation rows are not treated as authoritative busy locks. The
  supported cross-process signal is `SessionLock`; non-cooperating external
  provider processes remain outside this contract.
- `other` storage is not replaceable in v1 even if locate can identify a path.
  Without a renderer/parser contract, writing would be guesswork.
- Imported sessions lose parent/sidechain/compaction metadata until the
  canonical schema extends. `parent_turn_id`, `is_sidechain`, and
  `is_compaction_boundary` are written as `NULL` or defaults after replace.
- If directory fsync is unavailable on a platform, implementation must document
  the weaker durability boundary and test the fallback.
- If a receipt is lost after commit, callers must use export/hash to determine
  the committed postimage. There is no receipt log in v1.
- GUI state DB divergence remains out of scope; the CLI uses the same default
  state root as the sibling Initiative 06 commands.

# 13. Cross-feature constraint compliance checklist

| Constraint | Compliance | Citation / note |
| --- | --- | --- |
| Shared error-code namespace uses `10` session-not-found, `11` ambiguous-session, `12` unsupported-storage, `13` session-busy, `14` schema-incompatible, `15` invalid input/preimage mismatch. | Yes | Namespace in `initiatives/06-session-override-contract.md:106-111`; mapping in §5. |
| Ownership resolution reuses `StateDb::resolve_resume`; no second ownership path. | Yes | A2 and §4 pre-mutation setup; partial segmentless DBs remain not-found. |
| Lock observation for import-replace once pause-handshake lands. | Yes | D1 in §4: import-replace acquires `SessionLock`; busy exits `13`. 06-pause-handshake PR #17 supplies the lock primitive dependency. |
| Import-replace refuses if the session is in-flight or not exclusively owned by the replace operation. | Yes within cooperative lock surface | Existing lock or failed acquire exits `13`; non-cooperating external writers are residual in §12. Lock observation by writer paths (`run_repl`, `run_resume`, balanced one-shot, `migrate_chain_segment`) is a sibling-PR concern per 06-pause-handshake's PR #17 narrowed harness acceptance. v1 import-replace observes locks; concurrent runner writers observe per their own retrofit timeline. The harness consumer of v1 should treat `session-busy` as advisory until full retrofit lands. |
| Read-only `StateDb` open / schema compatibility supplied by schema-probe. | Yes | §4 pre-mutation setup exits `14` when probe says unsafe. |
| Export canonical reader defines import input family and round-trip oracle. | Yes | A3, §3 validation, §9 postimage round-trip. |
| Provider transcript file receives provider-native bytes, not canonical bytes. | Yes | §3 and §6 define `CanonicalToProviderRenderer`; `claude_code` and `codex_session` render native records; `other` returns `UnsupportedStorage`. |
| Lossy canonical-to-provider re-encoding is refused. | Yes | §3 renderer contract and §9 unsupported-record-class test exit `15 invalid-input-transcript`. |
| Two-phase atomic file replacement uses same-directory temp, fsync, rename. | Yes | D2 in §4 and fsync details in §8. |
| Durable journal closes post-rename/pre-DB crash recovery. | Yes | §4 journal flow, §6 startup recovery API, §8 side effects/crash states, §9 recovery tests. The journal carries resolved identity and `canonical_records_path`, and deletion happens only after verification plus DB commit. |
| State consistency covers required rows. | Yes, with documented canonical-field loss | D4a in §7 replaces `session_turns` and refreshes chain/segment metadata. `parent_turn_id`, `is_sidechain`, and `is_compaction_boundary` are set to `NULL`/defaults because `CanonicalRecord` does not carry them. |
| No auto-resume. | Yes | §1, §11, and residuals state no provider launch. |
| No provider spawn. | Yes | §1 and §11. |
| No quota refresh. | Yes | §1 and §11. |
| No config edits. | Yes | §1 and §11. |
| No coupling to `migrate-config`. | Yes | §1 and §11. |
| No provider-native JSONL as stable public input. | Yes | §1, §3, §10. |
| No manual recovery CLI in v1. | Yes | §6 and §12 mark `agents migrate-db --recover` / `agents session import-replace --recover` as anti-scope; startup auto-recovery is implemented. |
