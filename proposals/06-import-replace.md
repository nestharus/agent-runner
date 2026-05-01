# 1. Scope statement (Rev 1)

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

What changes:

- Add `session import-replace` under the `session` subcommand group in
  `src-tauri/src/main.rs`.
- Add reusable Rust code under a new module at
  `src-tauri/src/session_import_replace/`.
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

## 1.1 Assumption register

This is the approved register validated and narrowed from
`research/06-import-replace-problem-map.md` §7. It replaces the draft register
there; do not maintain a competing register.

| ID | Assumption | Evidence | Invalidator | Used by |
| --- | --- | --- | --- | --- |
| A1 | Earlier Initiative 06 surfaces land before import-replace: `SessionMetadata`, schema-probe feature flags/read-only checks, export canonical reader, and pause-handshake locks. | Initiative sequence places import-replace last and says it composes locate + canonical reader + lock (`initiatives/06-session-override-contract.md:38-56`, `initiatives/06-session-override-contract.md:75-89`). | Import-replace is rebased directly onto this local worktree without those APIs. | §2, §4, §6, §8, §13. |
| A2 | `StateDb::resolve_resume` remains the sole ownership resolver. | Initiative 06 requires reuse of `StateDb::resolve_resume`; current resolver owns chain/segment selection (`src-tauri/src/state/db.rs:2577-2670`). | A preceding feature changes public session ownership away from chain/segment resume semantics. | §4 resolution flow; §5 exits `10`/`11`; §13 checklist. |
| A3 | Canonical input is the 06-export `CanonicalRecord` JSONL family, not provider-native JSONL and not `session_turns`. | Harness anti-scope excludes provider-native JSONL; export defines `CanonicalRecord` fields and parser selection (`03-session-import-replace.md`, `06-export/src-tauri/src/session_export/mod.rs:8-99`). | Export contract changes before import-replace starts. | §3 validation; §6 replacement renderer; §9 tests. |
| A4 | Preimage/postimage hashes are over the canonical transcript byte stream emitted by the export serializer, not over summary DB rows. | `session_turns` lacks content/source hashes, while harness requests current canonical transcript hash (`src-tauri/src/state/db.rs:559-572`, `03-session-import-replace.md`). | Harness explicitly pins preimage to raw provider JSONL bytes instead of canonical export bytes. | D2 in §4; §6 receipt; §9 preimage tests. |
| A5 | Storage type remains the discriminator for supported replace behavior: `claude_code` and `codex_session` are supported; `other` is refused in v1. | Export parser supports Claude/Codex and rejects `other`; locate maps config storage to public types (`06-export/src-tauri/src/session_export/mod.rs:36-52`, `06-locate/src-tauri/src/session_metadata/mod.rs:23-39`). | A later storage API supplies a renderer for `other` before this feature lands. | §4 step 5; §5 exit `12`; §6 renderer. |
| A6 | Current `running` invocation rows are not sufficient as replace locks. | Running rows may lack session id before provider spawn, can survive hard process death, and are not tokenized leases (`research/06-import-replace-problem-map.md` §5 #10-13). | A preceding feature turns invocation lifecycle into durable active-writer leases. | D1 in §4; §8 lock API; §12 residuals. |
| A7 | State consistency must replace `session_turns` and refresh existing chain/segment rows after file commit. | Resolver, trace, export metadata, and migration all read session tables; `session_turns` uniqueness is `(provider_name, session_id, turn_id)` (`src-tauri/src/state/db.rs:559-597`, `src-tauri/src/state/db.rs:2577-2764`). | A preceding feature introduces a new canonical transcript-state table that supersedes these rows. | D4 in §4; §7 state update; §9 tests. |
| A8 | Crash recovery cannot make filesystem rename and SQLite update one physical transaction without a journal or larger storage redesign. | Current migration renames before DB segment updates and has no pending-op table (`src-tauri/src/migration/mod.rs:206-231`). | A prior feature lands a durable transcript-replace journal used by import-replace. | D5 in §4; §8 recovery; §12 residuals. |
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
mutates the canonical transcript and derived session tables. The value remains
positive because the mutation is bounded by resolver ownership, schema
compatibility checks, a session lock, canonical input validation, same-directory
atomic rename, and one receipt that records before/after hashes. Rollback is also
clear: avoid invoking the new subcommand or revert the additive CLI/module work.

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

# 3. Input validation

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

Line numbering in errors is internal detail. The public stderr JSON should carry
`invalid-input-transcript`, a short message, and optionally `line` when the
failure belongs to one input line.

The validator returns `Vec<CanonicalRecord>` for state-row reconstruction plus a
canonical JSONL byte stream serialized by the same canonical serializer export
uses. This normalized byte stream, not the caller's original formatting, is what
import-replace writes to supported provider transcript files.

Normalizing through the export serializer keeps `agents session export <id>`
after replace byte-for-byte comparable to the import stream that import-replace
committed. If Phase 5 hookpoints prove provider-native renderers cannot consume
that canonical byte stream directly, stop and revise this proposal rather than
silently writing provider-native JSONL from a different schema.

# 4. Resolution, lock, and atomic replace flow

D1 decision: import-replace acquires its own session lock. It does not merely
check for locks created by pause-handshake. The lock target is the resolved
active provider session id, not the raw user input. If `SessionLock::acquire`
returns busy, import-replace exits `13 session-busy`.

D2 decision: use a two-phase replace with same-directory temp file, fsync, and
atomic rename. The authoritative commit window is protected by `SessionLock`.

Flow:

1. Parse `<session-id>` and `--preimage-sha256`.
2. Load input bytes from stdin or `--from-file`.
3. Validate canonical JSONL shape and normalize input bytes.
4. Open state/config using the same CLI default state root used by the earlier
   Initiative 06 commands.
5. Run schema compatibility preflight. If schema-probe says
   `safe_for_import_replace: false`, exit `14 schema-incompatible` before any
   transcript mutation.
6. Resolve session metadata through `SessionMetadata`. Map resolver not-found to
   `10`, ambiguity to `11`, and unsupported/non-mutable storage to `12`.
7. Reject `storage_type: "other"` with exit `12 unsupported-storage`.
8. Validate the input canonical JSONL against the resolved `session_id`,
   `provider_name`, and storage renderer support. Invalid input exits `15`.
9. Clean stale import-replace temp files in the target transcript directory whose
   names match this feature's temp-file convention and are not currently locked
   by another live replace operation.
10. Compute a preflight `preimage_sha256` over the current canonical export stream
   for the resolved session. If `--preimage-sha256` is present and mismatches,
   exit `15` before lock acquisition.
11. Acquire `SessionLock` for the resolved active provider session id with owner
    `"import-replace"`. Busy exits `13`.
12. Re-read and re-hash the current canonical export stream while holding the
    lock. This second hash is the receipt `preimage_sha256`. If the optional
    caller-provided preimage now mismatches, release the lock and exit `15`.
13. Render the validated canonical records into the bytes to be written to
    `jsonl_path`. For v1, this is the canonical JSONL byte stream produced by
    the export serializer.
14. Write bytes to `<jsonl_path>.tmp-import-replace-<uuid>` in the same
    directory.
15. Fsync the temp file.
16. Atomically rename the temp file to `jsonl_path`.
17. Fsync the parent directory.
18. Update DB rows for state consistency in one SQLite transaction.
19. Compute `postimage_sha256` by reading the newly committed transcript through
    the same canonical export path used for preimage hashing.
20. Emit one receipt JSON on stdout, release the lock, and exit `0`.

The second preimage check after lock acquisition is intentional. It preserves the
harness preimage behavior while closing the time-of-check/time-of-use gap between
early hashing and the protected commit window.

# 5. Exit codes

D7 decision: use the harness exit namespace exactly.

| Exit | Error code | Producing condition | Mutation guarantee |
| --- | --- | --- | --- |
| `0` | none | Transcript file replaced, state consistency transaction committed, receipt emitted. | Transcript and DB update both completed. |
| `1` | `operational-error` or specific internal code | Unexpected I/O, DB, serialization, fsync, rename, lock-store operational error, or post-commit verification error not covered below. | Depends on phase; stderr must state whether rename happened when known. |
| `2` | `invalid-session-id` or clap usage | Invalid UUID, invalid flag value, missing args, duplicate/unknown args. | No mutation. |
| `10` | `session-not-found` | Resolver cannot find a chain/active session for the input. | No mutation. |
| `11` | `ambiguous-session` | Resolver returns ambiguous owner. | No mutation. |
| `12` | `unsupported-storage` | No supported storage, missing/unavailable transcript path, `other` storage, non-UTF-8 path, unsupported renderer, or metadata says the session is not file-backed/mutable. | No mutation. |
| `13` | `session-busy` | `SessionLock::acquire` reports an unexpired lock for the resolved session, or exclusive ownership cannot be established. | No mutation by this process. |
| `14` | `schema-incompatible` | Schema-probe/read-only compatibility says import-replace is unsafe for this state DB or required feature flags are absent. | No mutation. |
| `15` | `invalid-input-transcript` or `preimage-mismatch` | Input canonical JSONL is malformed, schema-invalid, session/provider mismatched, not reconstructable for v1 state, or `--preimage-sha256` does not match the current canonical transcript. | No mutation when detected before rename; no DB mutation if detected after lock before write. |

Errors must not print partial receipts. Success must not print non-JSON human
text on stdout.

# 6. Receipt JSON schema

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
2. Insert one summary row per canonical turn that maps cleanly to
   `SessionTurnIngest` fields.
3. Preserve role, turn id, timestamp, parent/sidechain/compaction flags where
   represented by canonical records.
4. Set `source_file` to the replaced `jsonl_path` when the current schema/helper
   supports it; otherwise keep existing ingest helper behavior if the column is
   not meaningful in this branch.
5. Update `session_chains.last_used_at` for the resolved chain to the newest
   imported turn timestamp, or commit time if no usable turn timestamp exists.
6. Update the active `session_chain_segments` row for the resolved
   `(chain_id, provider_name, session_id)` so its `last_turn_id` reflects the
   latest imported turn id.
7. Do not close or reopen chain segments. Replace is content mutation of the
   existing active segment, not a migration to a new segment.
8. Do not create new chains. If no resolver-visible active segment exists, the
   command already exited `10`.

Unsupported canonical records may be retained in transcript bytes, but they do
not produce `session_turns` rows unless the shared parser can map them into a
known turn summary. All-unsupported input exits `15` before mutation.

If the SQLite transaction fails after the rename, the command exits `1` and
reports that the transcript file may already have been replaced. It must not
attempt to rename the old file back unless a durable rollback protocol exists.

# 8. Locking and crash recovery

Lock behavior:

- Import-replace creates a `SessionLock` rooted at the same lock directory used
  by 06-pause-handshake.
- It calls `SessionLock::acquire` for the resolved active provider session id.
- Busy maps to exit `13`.
- Operational lock-store failures map to exit `1`.
- The lock is released on normal success and on handled pre-rename failures.
- If the process dies, the lock lease expires according to `SessionLock`
  semantics; import-replace does not invent a second lock format.

Temp-file convention:

```text
<jsonl_path>.tmp-import-replace-<uuid>
```

Crash states:

1. Crash before temp write: no durable mutation.
2. Crash after temp write before rename: temp file lingers; future
   import-replace startup may unlink stale temp files with this feature prefix.
3. Crash after fsync before rename: same as #2.
4. Crash after rename before DB update: transcript is replaced and DB rows may
   be stale. The next ingestion scan, future `migrate-db`, or a repeated
   import-replace with the correct preimage can reconcile state.
5. Crash during DB transaction: SQLite rolls the transaction back or commits it
   according to SQLite durability; transcript file remains replaced.
6. Crash after DB commit before receipt: transcript and DB are updated, but the
   caller did not receive a receipt. Re-running with the old preimage should
   fail `15`; exporting and hashing gives the postimage.

D5 decision: do not add a durable pending-operation table in v1. Stale temp
files are cleaned opportunistically by prefix/age, and the post-rename/pre-DB
gap is documented as residual. A future journal can tighten this if Phase 4
risk gates require it.

Directory fsync:

- Fsync the temp file after writing.
- Fsync the parent directory after rename.
- Treat fsync failures as operational errors.
- On platforms where directory fsync is unavailable, use the strongest local
  equivalent and document the platform caveat in code comments and tests.

# 9. Test-intent track

## 9.1 Test-intent track

| Change risk or verification risk | Intended behavior / acceptance condition | Level | Fixture source / application point | Assumption link | Expected observable signal | Residual risk |
| --- | --- | --- | --- | --- | --- | --- |
| Valid stdin replace | Canonical JSONL from stdin replaces an idle supported Claude fixture. | end-to-end | Temp state DB, providers/sessions config, located JSONL, canonical fixture from export. | A1, A3, A5 | Exit `0`; receipt hashes present; path bytes replaced; export after replace matches import. | Does not cover Codex renderer. |
| Valid `--from-file` replace | `--from-file packed.jsonl` behaves identically to stdin. | end-to-end | Same fixture as stdin with file input. | A9 | Exit `0`; stdout receipt shape identical except hashes/timestamps. | None beyond file I/O platform behavior. |
| Preimage mismatch | Wrong `--preimage-sha256` exits `15` before lock/write. | component + e2e | Fixture with known existing export hash. | A4 | Exit `15`; stderr `preimage-mismatch`; transcript mtime/hash unchanged. | Concurrent external writer outside locks remains outside supported surface. |
| Lock busy | Existing pause-handshake lock causes exit `13`. | integration | Use 06-pause-handshake `SessionLock` to acquire same session id before command. | A6 | Exit `13`; no temp file; no transcript or DB mutation. | Does not prove every non-cooperating provider process is detected. |
| Malformed JSONL | Invalid JSON, blank line, non-UTF-8, or missing canonical fields exit `15`. | unit + e2e | Validator unit cases plus CLI stdin cases. | A3 | Exit `15`; stderr line when available; no mutation. | Exact error messages not stable. |
| Session/provider mismatch | Valid canonical records for another session/provider exit `15`. | component | Canonical fixture with mismatched ids. | A2, A3 | Exit `15`; no mutation. | None. |
| Unsupported storage | `other` storage or missing renderer exits `12`. | integration | Metadata fixture from locate with no supported storage. | A5 | Exit `12`; no lock/write. | Future storage support may alter expected result. |
| Schema incompatible | Probe says unsafe for import-replace exits `14`. | component | Temp DB missing required tables/indexes or feature flag fixture. | A1 | Exit `14`; no input write. | Depends on schema-probe helper shape after merge. |
| Atomic temp/rename | Failure injected before temp, after temp, after fsync, before rename, and after rename produces deterministic state. | component | Replace primitive with injectable failure points. | A8 | Pre-rename failures leave target unchanged; post-rename failures report possible file commit. | Real OS crash cannot be perfectly simulated. |
| DB row replacement | Existing `session_turns` for the session are deleted/reinserted; unrelated sessions remain unchanged. | component | Seed two sessions under same provider and replace one. | A7 | Row counts and latest turn ids match imported canonical records. | Summary mapping from unsupported records remains intentionally limited. |
| Chain/segment refresh | Existing active segment remains same row; `last_turn_id` and chain `last_used_at` refresh. | component | Seed active chain/segment with old turns. | A7 | Same chain/segment identity; refreshed fields only. | Does not prove future resolver changes. |
| Postimage round-trip | `agents session export <id>` after success emits the imported canonical transcript. | end-to-end | Claude and Codex fixtures where available. | A3, A4, A5 | Export hash equals receipt `postimage_sha256`. | If Codex renderer deferred, Codex test becomes explicit unsupported-storage test. |

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
- Show stdin and `--from-file` examples.
- Document the receipt JSON fields exactly as §6.
- Document exit codes `0`, `1`, `2`, `10`, `11`, `12`, `13`, `14`, and `15`.
- Document that import-replace acquires a session lock and returns
  `session-busy` for existing locks.
- Document that preimage hashes are canonical export hashes.
- Clarify that a crash after file rename and before DB update can leave state
  stale until the next reconciliation path.

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

Observability: the receipt JSON is the durable caller-facing signal. The command
does not create invocation rows, run provider commands, refresh quotas, or emit
GUI events. Stderr JSON is the failure signal.

# 12. Implementation residuals

Known residuals Phase 4 should evaluate rather than treat as accidental
omissions:

- The file rename and DB update are not one physical transaction. A crash after
  rename and before DB commit can leave transcript and derived rows out of sync.
- There is no durable pending-operation table in v1. Stale same-directory temp
  files are cleaned opportunistically by naming convention.
- Running invocation rows are not treated as authoritative busy locks. The
  supported cross-process signal is `SessionLock`; non-cooperating external
  provider processes remain outside this contract.
- `other` storage is not replaceable in v1 even if locate can identify a path.
  Without a renderer/parser contract, writing would be guesswork.
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
| Ownership resolution reuses `StateDb::resolve_resume`; no second ownership path. | Yes | A2 and §4 steps 4-6; partial segmentless DBs remain not-found. |
| Lock observation for import-replace once pause-handshake lands. | Yes | D1 in §4: import-replace acquires `SessionLock`; busy exits `13`. |
| Import-replace refuses if the session is in-flight or not exclusively owned by the replace operation. | Yes within cooperative lock surface | Existing lock or failed acquire exits `13`; non-cooperating external writers are residual in §12. |
| Read-only `StateDb` open / schema compatibility supplied by schema-probe. | Yes | §4 step 5 exits `14` when probe says unsafe. |
| Export canonical reader defines import input family and round-trip oracle. | Yes | A3, §3 validation, §9 postimage round-trip. |
| Two-phase atomic file replacement uses same-directory temp, fsync, rename. | Yes | D2 in §4 and fsync details in §8. |
| State consistency covers required rows. | Yes | D4a in §7 replaces `session_turns` and refreshes chain/segment metadata. |
| No auto-resume. | Yes | §1, §11, and residuals state no provider launch. |
| No provider spawn. | Yes | §1 and §11. |
| No quota refresh. | Yes | §1 and §11. |
| No config edits. | Yes | §1 and §11. |
| No coupling to `migrate-config`. | Yes | §1 and §11. |
| No provider-native JSONL as stable public input. | Yes | §1, §3, §10. |
