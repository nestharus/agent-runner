# Phase 5 Hookpoints — 06-import-replace (`agents session import-replace`)

> **Note (pre-change evidence):** This hookpoint map describes the current
> `06-import-replace` worktree before any Phase 6 implementation. The approved
> action map is `proposals/06-import-replace.md` Rev 4. The audit gate is LOW /
> CLEARED in `risk/06-import-replace-audit.md`; the prompt-referenced
> `risk/06-import-replace-audit-history.md` is absent in this checkout, and the
> current audit file records that absence plus the AIR-R1/AIR-R2/AIR-R3 closure
> state. Rev 4's binding change is the lock-before-journal reorder: canonical
> records first land in an operation-unique staging path, `SessionLock` is
> acquired, and only then are per-session journal artifacts published.

## A. `session import-replace` subcommand hookpoints (proposal §2)

- **Extend:** `Subcommands` currently lives in `src-tauri/src/main.rs:77-166`
  and has no `Session` parent in this local worktree. In the stacked Initiative
  06 branch, 06-locate introduces `Subcommands::Session { command:
  SessionSubcommands }`; import-replace must extend that nested enum rather than
  creating another top-level command group.
- **New child:** add `ImportReplace { session_id: String, #[arg(long =
  "from-file")] from_file: Option<PathBuf>, #[arg(long =
  "preimage-sha256")] preimage_sha256: Option<String> }` beside `Locate`,
  `Export`, `PauseHandshake`, and `ResumeHandshake`.
- **Bare parent behavior:** keep the nested child required. Bare
  `agents session` and bare `agents session import-replace` should remain clap
  usage failures with exit `2`.
- **Dispatch:** top-level dispatch is the `match command` in
  `run(cli)` at `src-tauri/src/main.rs:287-338`. Extend the nested
  `Subcommands::Session` match with `SessionSubcommands::ImportReplace` and
  route to a small `run_session_import_replace` wrapper in `main.rs`.
- **CLI wrapper responsibilities:** parse `<session-id>` as a full UUID before
  DB/config/lock/journal access, validate `--preimage-sha256` as exactly 64 hex
  characters, load bytes from `--from-file` or stdin, call the reusable
  import-replace API, and map typed errors to the shared exit namespace.
- **Input routing:** when `--from-file` is present, ignore stdin. When absent,
  read all stdin bytes. The loaded stream is buffered before any state lookup,
  lock acquisition, journal write, or transcript mutation.
- **Usage errors:** malformed UUID and malformed `--preimage-sha256` are usage
  failures (`2`) rather than domain failures. Structural clap errors remain
  clap-owned text; import-replace semantic errors use compact JSON stderr.
- **Stdout contract:** success emits exactly one compact JSON receipt. Do not
  print progress, warnings, lock details, or recovered-state notices on stdout.
- **No hookpoint — existing prompt readers:** do not reuse `run_resume`'s
  prompt/file/stdin input path at `src-tauri/src/main.rs:1056-1071`; that path
  is answer-payload oriented, trims empty stdin differently, and emits provider
  stdout.
- **README hook:** update the session command synopsis and a short
  import-replace section, but no GUI/Tauri command surface is added.

## B. Reusable `CanonicalToProviderRenderer` API hookpoints (proposal §3 / §6)

- **New module:** create `src-tauri/src/session_replace/render/` for provider
  native rendering. Export it through a parent `session_replace` or through
  `session_import_replace` only if no other module needs it immediately.
- **Public module export:** add new public modules in `src-tauri/src/lib.rs`,
  beside existing exports at `src-tauri/src/lib.rs:1-11`. Expected additions are
  `pub mod session_import_replace;` and either `pub mod session_replace;` or a
  public renderer submodule reachable by tests.
- **Reader dual:** 06-export's canonical reader currently defines
  `CanonicalRecord`, `ContentChunk`, `RecordSource`, and `SessionStorageType` at
  `/home/nes/projects/agent-runner/worktrees/06-export/src-tauri/src/session_export/mod.rs:8-52`.
  Import-replace should consume those types directly, not copy a second record
  family.
- **Renderer trait shape:** define a small dispatcher API such as
  `CanonicalToProviderRenderer::render(storage_type, records) -> Result<Vec<u8>,
  RenderError>`. The API returns provider-native bytes, never canonical JSONL
  bytes.
- **Storage dispatch:** mirror export's `read_canonical_transcript` dispatcher
  at `06-export/.../session_export/mod.rs:88-99`: `ClaudeCode` routes to a
  Claude renderer, `CodexSession` routes to a Codex renderer, and `Other`
  returns `UnsupportedStorage`.
- **Error model:** `RenderError` needs at least `UnsupportedStorage`,
  `UnsupportedRecordClass { class }`, `SessionProviderMismatch`, and
  `Operational`. Map unsupported storage to exit `12`; map lossy or invalid
  canonical content to exit `15`.
- **Losslessness gate:** the renderer must reject canonical records that cannot
  be represented in the target provider's native JSONL without semantic loss.
  Do this before temp transcript writes. Multi-modal blocks, tool use/results,
  unsupported placeholders, or future canonical chunks fail closed unless the
  renderer has an auditable native representation.
- **Canonical input normalization:** parse and reserialize input with the export
  canonical record serializer before hashing. That normalized canonical byte
  stream is the preimage/postimage hash domain and the journal-attached recovery
  file.
- **State update policy:** DB reconstruction uses canonical records, not
  rendered bytes. Fields absent from canonical records (`parent_turn_id`,
  `is_sidechain`, `is_compaction_boundary`) are written as `NULL` or defaults.

## C. Resolution flow hookpoints, including lock-before-journal staging (proposal §4 Rev 4)

- **Step 1, parse arguments:** parse UUID and preimage hash in the CLI wrapper.
  `StateDb::resolve_resume` also rejects invalid UUIDs at
  `src-tauri/src/state/db.rs:2577-2585`, but import-replace must fail before
  state or journal access.
- **Step 2, load input:** read all bytes from stdin or `--from-file`, decode as
  UTF-8, split as JSONL, reject empty input and blank lines, and deserialize
  every line into the export `CanonicalRecord` family.
- **Step 3, normalize + operation id:** allocate `operation_uuid` before any
  scratch writes. Serialize canonical records through the same compact JSONL
  shape export emits, with optional trailing newline handled consistently by the
  serializer.
- **Step 4, state/config open:** use the same default CLI state root as the
  sibling Initiative 06 commands. The current `StateDb::open_default` points at
  `dirs::data_dir()/oulipoly-agent-runner/state.db`
  (`src-tauri/src/state/db.rs:611-615`), but Phase 6 should prefer the stacked
  schema-probe/default-path helpers where available.
- **Step 5, schema preflight:** call the schema-probe helper before mutation.
  The 06-schema-probe report exposes `safe_for_import_replace` at
  `/home/nes/projects/agent-runner/worktrees/06-schema-probe/src-tauri/src/schema_probe/mod.rs:12-19`
  and computes it from DB compatibility plus feature flags at lines `154-206`.
  Unsafe state maps to exit `14`.
- **Step 6, pre-lock staging write:** write normalized canonical bytes to
  `<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`.
  This is the only allowed pre-lock filesystem artifact from this feature.
- **Step 7, metadata resolution:** call `locate_session_metadata` from
  06-locate (`06-locate/.../session_metadata/mod.rs:81-132`) to get the resolved
  active provider session id, chain id, provider name, storage type, path, and
  mutability.
- **Step 8, resolver semantics:** do not add another ownership path.
  `StateDb::resolve_resume` chooses candidates from `session_chain_segments`
  (`src-tauri/src/state/db.rs:2696-2711`) and selects the latest active segment
  at `src-tauri/src/state/db.rs:2751-2764`; import-replace inherits those
  not-found/ambiguous rules.
- **Step 9, freeze identity:** freeze `session_id`, `chain_id`,
  `active_segment_id`, `provider_name`, `storage_type`, and `jsonl_path` for the
  operation before acquiring the lock. Recovery must use the frozen identity,
  not a later resolver result.
- **Step 10, validate against metadata:** after resolution, reject canonical
  records whose `session_id` or `provider_name` disagree with the frozen
  identity. Reject `storage_type: other`, unavailable/missing/non-UTF-8 paths,
  and non-mutable metadata as exit `12`.
- **Step 11, renderer preflight:** run a dry render or full render-support
  validation against the frozen storage type before acquiring the lock when
  possible. The actual provider-native bytes may be produced under lock, but no
  known lossy input should reach the mutation window.
- **Step 12, acquire lock:** create `SessionLock` at the same data-dir lock
  root as pause-handshake and call `acquire` for the resolved active provider
  session id, not the raw user input. Busy maps to `13`.
- **Step 13, busy cleanup:** if acquire returns busy, unlink only
  `staging/<operation_uuid>.canonical.jsonl`, fsync the staging directory where
  supported, and exit `13`. Do not create, rewrite, or delete any
  `session-<id>.pending` or `session-<id>.canonical.jsonl` path.
- **Step 14, publish under lock:** after acquire succeeds, atomically rename
  staging to `<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl`
  and fsync `replace_journal/`.
- **Step 15, journal under lock:** write
  `<state-data-dir>/replace_journal/session-<session_id>.pending` only after the
  staging-to-session rename lands. The lock holder owns this per-session
  recovery signal.
- **Step 16, protected preimage:** read the current provider transcript through
  06-export's canonical reader, compute the canonical export hash, write it into
  the journal by atomic rewrite/fsync, then compare to `--preimage-sha256` if
  supplied.
- **Step 17, mutation:** render provider-native bytes, write the same-directory
  transcript temp file, fsync the temp file, rename over `jsonl_path`, and fsync
  the parent directory.
- **Step 18, DB + verify:** open a SQLite transaction, replace derived state,
  verify postimage hash and fresh export equality, commit, delete journal files,
  fsync the journal directory, release lock, and emit the receipt.

## D. Replace journal hookpoints: layout, schema, write/read/cleanup

- **Root:** use `<state-data-dir>/replace_journal/` under the same default data
  root as `state.db` and `locks`. Do not place journals beside provider JSONL
  files.
- **Staging directory:** create `<state-data-dir>/replace_journal/staging/` for
  operation-unique canonical input scratch files named
  `<operation_uuid>.canonical.jsonl`.
- **Pending file:** use
  `<state-data-dir>/replace_journal/session-<session_id>.pending` for the
  versioned JSON journal.
- **Canonical side file:** use
  `<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl` for
  the normalized canonical records attached to the pending operation.
- **Quarantine directory:** use
  `<state-data-dir>/replace_journal/quarantine/` for ambiguous pending journals.
  Preserve the canonical side file in place for manual inspection.
- **Journal schema:** persist `schema_version`, `operation`,
  `started_at`, `session_id`, `chain_id`, `active_segment_id`,
  `provider_name`, `storage_type`, `jsonl_path`, `operation_uuid`,
  `preimage_sha256`, `postimage_sha256`, `canonical_records_path`,
  `db_state_pending`, and `expected_turn_count`.
- **Schema versioning:** v1 reads only `schema_version: 1` and
  `operation: "import-replace"`. Unsupported versions are operational during a
  live command and should be quarantined or ignored with a warning during
  startup recovery, depending on how much can be safely parsed.
- **Atomic journal write:** implement journal writes with a unique temp file in
  `replace_journal/`, file fsync, same-directory rename, and directory fsync.
  The lock module's `atomic_write_json` pattern at
  `06-pause-handshake/.../session_lock/mod.rs:263-293` is the closest local
  precedent.
- **Journal rewrites:** preimage insertion and any `db_state_pending` updates
  should rewrite the whole JSON file atomically. Do not append partial JSON or
  rely on in-place mutation.
- **Read path:** parse journals as structured JSON, then read
  `canonical_records_path` as canonical JSONL for DB recovery. Recovery must
  not reconstruct DB rows from the provider-native postimage.
- **Cleanup success:** delete the pending journal and canonical side file only
  after postimage hash verification, fresh export verification, and SQLite
  commit all succeed. This deletion is the last durable cleanup step before the
  receipt.
- **Cleanup failure:** handled failures after lock acquisition leave the pending
  journal and canonical side file in place unless the failure is explicitly
  pre-rename/no-op cleanup during startup recovery.
- **Transcript temp cleanup:** stale `<jsonl_path>.tmp-import-replace-<uuid>`
  files are inert unless renamed. Opportunistic cleanup is by exact prefix/age
  in the target transcript directory and must avoid touching provider-owned
  temp files.

## E. Crash-recovery startup scan hookpoints (proposal §6 / §8)

- **Startup hook:** run `session_import_replace::recover_pending_replaces()` on
  CLI startup before commands that rely on session-derived state, or in the
  state-open/session-command setup path before locate/export/resume/migration
  resolution. Avoid running recovery from GUI-only paths unless the default CLI
  state root is the same.
- **Scan pattern:** enumerate `replace_journal/session-*.pending`. Ignore
  non-matching files, staging files, and quarantine entries.
- **Parse journal:** extract frozen identity and hashes from the journal:
  `chain_id`, `active_segment_id`, `provider_name`, `storage_type`,
  `jsonl_path`, `operation_uuid`, and `canonical_records_path`.
- **No-preimage journal:** if the journal lacks `preimage_sha256`, the process
  died before reading the old transcript. Treat it as pre-rename no-op: delete
  the pending journal and canonical side file, fsync `replace_journal/`, and do
  not mutate DB state.
- **Compute current hash:** read `jsonl_path` through the storage-specific
  export parser and canonical serializer. Hash canonical export bytes, not raw
  provider-native bytes.
- **Postimage match:** if current canonical hash equals `postimage_sha256`, the
  transcript rename landed. Re-apply DB updates idempotently from
  `canonical_records_path`, refresh the frozen segment/chain, delete recovery
  files, and fsync.
- **Preimage match:** if current canonical hash equals `preimage_sha256`, the
  transcript rename did not land or was rolled back. Delete the journal and
  canonical side file; leave transcript and DB untouched.
- **Ambiguous hash:** if current hash matches neither hash, or if export parsing
  fails, move the journal to `replace_journal/quarantine/`, preserve
  `canonical_records_path`, log the required manual-recovery warning, and leave
  transcript/DB untouched.
- **Idempotence:** DB recovery must be safe after a SQLite commit already
  happened. Delete and reinsert the same `(provider_name, session_id)` turns,
  then update the same frozen active segment and chain.
- **No manual CLI:** do not add `agents session import-replace --recover` or
  `agents migrate-db --recover` in v1.

## F. Two-phase atomic mutation hookpoints (tempfile + rename + DB transaction)

- **Existing precedent gap:** migration currently writes a fixed
  `jsonl.tmp` and renames it before DB updates
  (`src-tauri/src/migration/mod.rs:206-231`). Import-replace must not reuse that
  primitive as-is; it lacks unique temp names, fsyncs, and a recovery journal.
- **Temp path:** write provider-native bytes to
  `<jsonl_path>.tmp-import-replace-<operation_uuid>` in the same directory as
  `jsonl_path`. Use create-new semantics where possible.
- **File fsync:** after writing provider-native bytes, call file sync before
  rename. Treat fsync failure as exit `1` and leave the journal in place if
  already published.
- **Atomic rename:** use same-directory rename from temp to `jsonl_path`.
  Rename failure is operational; when rename has not happened, target transcript
  should remain unchanged.
- **Directory fsync:** fsync the transcript parent directory after rename. On
  platforms where directory fsync is unavailable, use the strongest local
  equivalent and document the caveat in code.
- **SQLite transaction:** begin the DB transaction only after transcript rename
  and parent directory fsync. The transaction replaces derived state but does
  not make the filesystem rename atomic with SQLite; the journal bridges that
  gap.
- **DB replacement:** delete `session_turns` for the frozen
  `(provider_name, session_id)`, insert rows from canonical records, update
  `session_chain_segments.last_turn_id` / recency for the frozen
  `active_segment_id`, and update `session_chains.last_used_at` for the frozen
  `chain_id`.
- **No segment reopen:** do not call `open_chain_segment` or
  `close_active_segment_returning`; those helpers are migration semantics at
  `src-tauri/src/state/db.rs:2365-2498`, not content replacement semantics.
- **Postimage verification:** before committing, read the newly written
  provider-native transcript through export and compare its canonical hash to
  the journal's `postimage_sha256`.
- **Fresh export verification:** also compare fresh canonical export bytes to
  `canonical_records_path`. Verification failure rolls back SQLite, exits `1`,
  and leaves recovery artifacts.
- **Commit then cleanup:** commit SQLite only after both verification gates.
  Delete journal artifacts only after commit. If the process dies before
  cleanup, startup recovery re-applies the DB update and cleans.
- **Receipt timestamp:** `state_updated` is true only after DB commit; the
  receipt timestamp is taken in the committed success path.

## G. Provider-native rendering hookpoints (Claude vs Codex)

- **Claude reader evidence:** export parses Claude by top-level `sessionId`,
  `type`, `uuid`, `timestamp`, message/content fields, and
  `isCompactSummary` at
  `06-export/.../session_export/mod.rs:101-164`.
- **Claude renderer target:** emit one JSONL object per mappable canonical turn
  with native fields compatible with the reader: `sessionId`, `type`, `uuid`,
  `timestamp`, and a message/content payload that `extract_claude_content` can
  read back.
- **Claude roles:** canonical role `user` and `assistant` map directly to
  native `type`. Unknown roles fail with `unsupported-record-class:<role>` or a
  role-specific invalid input error.
- **Claude content:** canonical text chunks should render into the known Claude
  message/content shape. Non-text chunks fail unless the renderer implements a
  lossless native form and export can read it back to the same canonical chunk.
- **Codex reader evidence:** export requires a matching `session_meta` line and
  maps `response_item` records whose `payload.type == "message"` and role is
  `user` or `assistant` at
  `06-export/.../session_export/mod.rs:166-236`.
- **Codex renderer target:** emit a native `session_meta` record for the
  resolved session if required for parser round-trip, followed by `response_item`
  message records with `payload.type: "message"`, `payload.role`, timestamp,
  payload content, and stable payload ids where canonical `turn_id` can be used
  safely.
- **Codex synthesized ids:** export currently synthesizes turn ids from
  `<path>:<line>` when payload id is absent
  (`06-export/.../session_export/mod.rs:201-207`). Import-rendered Codex records
  should prefer explicit native ids to avoid path/line-dependent postimage drift.
- **Codex session_meta:** because export fails without matching `session_meta`
  (`06-export/.../session_export/mod.rs:223-232`), the renderer must produce or
  preserve a matching metadata line when replacing the whole file.
- **Provider mismatch:** a canonical record for one provider cannot be rendered
  into another provider's transcript path after resolution. Detect this before
  mutation and exit `15`.
- **Round-trip oracle:** for every supported renderer fixture, `render ->
  read_canonical_transcript -> canonical serialize` must equal normalized input
  byte-for-byte.

## H. Lock observation hookpoints (`SessionLock` acquire from 06-pause-handshake)

- **Dependency:** 06-pause-handshake PR #17 supplies `session_lock`. Its current
  API shape exposes `SessionLock`, `Lease`, and `LockError` at
  `/home/nes/projects/agent-runner/worktrees/06-pause-handshake/src-tauri/src/session_lock/mod.rs:14-43`.
- **Constructor:** `SessionLock::new(lock_dir)` creates/canonicalizes the lock
  directory, sets Unix `0700`, opens `sentinel.lock`, and stores the sentinel fd
  (`06-pause-handshake/.../session_lock/mod.rs:81-98`).
- **Acquire:** call `SessionLock::acquire(session_id, provider_name, ttl)` after
  metadata resolution and before per-session journal publication. The current
  implementation returns `Busy { expires_at }` for non-expired existing locks
  (`06-pause-handshake/.../session_lock/mod.rs:100-157`).
- **Lock key:** use the resolved active provider session id, not the raw
  command argument. This matches pause-handshake's design and handles chain-id
  inputs correctly.
- **TTL:** import-replace needs a TTL long enough for render/write/DB
  verification. Reuse pause-handshake constants if public; otherwise define an
  import-replace internal default with tests that avoid sleeps.
- **Release:** call `release` on normal success and handled failures after
  acquire. If release itself fails after the mutation completed, report
  operationally only if no success receipt has been emitted; never roll back a
  committed transcript.
- **Busy mapping:** `LockError::Busy` maps to exit `13 session-busy`; malformed
  lock metadata, fsync, rename, permission, randomness, or sentinel failures map
  to exit `1`.
- **Sibling writers:** import-replace observes locks in v1. Full observation by
  `run_repl`, `run_resume`, balanced one-shot, and migration is a sibling
  retrofit tracked by 06-pause-handshake; do not claim non-cooperating external
  writers are blocked.

## I. Test-intent track hookpoints (proposal §9.1)

- **Test homes:** unit/component tests belong in `src-tauri/src/session_import_replace/`
  and `src-tauri/src/session_replace/render/`. CLI integration tests belong in
  a new `src-tauri/tests/initiative_06_import_replace.rs`, following existing
  binary-test patterns that use `env!("CARGO_BIN_EXE_oulipoly-agent-runner")`.
- **Fixture builder:** add a temp state/config/transcript builder that seeds
  `session_chains`, `session_chain_segments`, `session_turns`, providers config,
  sessions config, and locator scripts. Reuse 06-locate/06-export fixture
  shapes where they exist.
- **Canonical fixtures:** derive valid canonical JSONL fixtures from the export
  reader. Store normalized input bytes and expected provider-native render bytes
  for Claude and Codex separately.
- **CLI success stdin:** valid canonical stdin replaces an idle supported Claude
  fixture; assert exit `0`, receipt fields, provider-native file bytes, DB rows,
  and export-after-replace equals input.
- **CLI success file:** `--from-file` uses the same code path after byte load;
  assert identical outcome class to stdin.
- **Preimage mismatch:** wrong `--preimage-sha256` exits `15`, leaves transcript
  unchanged, and leaves journal/canonical records for startup preimage cleanup.
- **Lock busy:** acquire `SessionLock` first, run import-replace, assert exit
  `13`, staging file removed, and no per-session journal artifacts.
- **Concurrent replace:** spawn two subprocesses for the same session with
  distinguishable inputs; exactly one exits `0`, the other exits `13`, final
  export matches the winner, and the loser leaves no per-session journal files.
- **Malformed input:** invalid JSON, blank lines, non-UTF-8, missing fields,
  bad timestamps, all-unsupported input, and provider/session mismatch exit
  `15` before lock/journal/transcript mutation.
- **Unsupported storage:** `other`, no storage, missing transcript path, or
  renderer unsupported exits `12` before lock or journal.
- **Unsupported record class:** canonical records with chunks/classes that the
  target renderer cannot round-trip exit `15` before mutation and name the
  unsupported class.
- **Schema incompatible:** a temp DB or mocked probe reporting unsafe exits
  `14` before staging or mutation.
- **Fault injection:** add replace primitive injection points after staging
  write, after journal write, after temp write, after temp fsync, after rename,
  before DB commit, after DB commit, and before cleanup.
- **Recovery rename-only:** crash after transcript rename and before DB commit;
  startup scan sees postimage, rebuilds DB from `canonical_records_path`, and
  removes journal files.
- **Recovery pre-rename:** crash after journal write but before transcript
  rename; startup scan sees preimage, leaves DB/transcript unchanged, and
  deletes recovery artifacts.
- **Recovery ambiguous:** corrupt transcript while pending journal exists;
  startup scan quarantines journal, preserves canonical records, logs warning,
  and leaves DB/transcript untouched.
- **No deletion before verify:** inject postimage or fresh-export mismatch after
  rename; command exits `1`, rolls back SQLite, and leaves journal artifacts.
- **DB replacement:** seed unrelated sessions under the same provider and assert
  only the frozen `(provider_name, session_id)` rows are deleted/reinserted.
- **Metadata loss explicit:** seed parent/sidechain/compaction metadata, replace
  from canonical records, and assert those fields are `NULL`/defaults.
- **Chain/segment refresh:** active segment row identity remains stable while
  `last_turn_id` and chain/segment recency refresh from canonical records.
- **Renderer round-trip:** provider renderer component tests assert
  canonical -> native -> export canonical equality for Claude and Codex.

## J. Implementation surface summary

- **CLI:** extend the stacked `SessionSubcommands` with `import-replace` and add
  `run_session_import_replace` in `src-tauri/src/main.rs`.
- **Core module:** add `src-tauri/src/session_import_replace/` for command
  orchestration, canonical input validation, normalized serialization, hash
  computation, journal lifecycle, recovery scan, error mapping, receipt structs,
  and fault-injection test seams.
- **Renderer module:** add `src-tauri/src/session_replace/render/` for
  `CanonicalToProviderRenderer`, Claude native rendering, Codex native
  rendering, and unsupported-storage/lossy-class errors.
- **Export dependency:** reuse 06-export's canonical types and
  `read_canonical_transcript`; do not copy parser logic into import-replace.
- **Metadata dependency:** reuse 06-locate's `SessionMetadata` /
  `locate_session_metadata`; do not query `session_turns` or locator scripts
  directly for ownership.
- **Schema dependency:** call 06-schema-probe compatibility helpers and exit
  `14` when `safe_for_import_replace` is false.
- **Lock dependency:** use 06-pause-handshake's `SessionLock` for the resolved
  active provider session id. Busy exits before per-session journal artifacts
  are created.
- **DB API:** add a focused `StateDb` helper such as
  `replace_session_turns_from_canonical(...)` that owns one transaction for
  deleting/reinserting `session_turns` and refreshing the frozen chain/segment.
- **Filesystem API:** add private helpers for atomic write JSON, atomic write
  bytes, fsync file, fsync directory, same-directory rename, staging cleanup,
  and transcript temp cleanup. Avoid relying on migration's fixed temp path.
- **Startup recovery hook:** wire `recover_pending_replaces` into CLI startup or
  state/session command initialization before session resolution work reads
  derived rows.
- **Docs:** update `README.md` with synopsis, input format, receipt fields,
  exit codes, canonical hash meaning, lock behavior, renderer losslessness, and
  startup recovery behavior.
- **Out of scope:** no provider spawn, no auto-resume, no quota refresh, no
  config edits, no provider-native input format, no GUI command, no manual
  recovery CLI, and no cross-provider migration refactor in this feature.
