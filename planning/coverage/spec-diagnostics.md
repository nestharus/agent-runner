# spec-diagnostics — Runtime diagnostics, trace, services, ports

## Source files

- `crates/oulipoly-state/src/diagnostic_recorder.rs`
- `crates/oulipoly-state/src/diagnostic_producer.rs`
- `crates/oulipoly-state/src/event_store/mod.rs`
- `crates/oulipoly-state/src/event_store/envelope.rs`
- `crates/oulipoly-state/src/event_store/schema.rs`
- `crates/oulipoly-state/src/event_store/generation.rs`
- `crates/oulipoly-state/src/event_store/writer.rs`
- `crates/oulipoly-state/src/event_store/reader.rs`
- `crates/oulipoly-state/src/event_store/union_reader.rs`
- `crates/oulipoly-state/src/event_store/importer.rs`
- `crates/oulipoly-state/src/event_store/maintenance.rs`
- `crates/oulipoly-state/src/sqlite_observability.rs`
- `crates/oulipoly-state/src/db/invocation_lifecycle_finalize.rs`
- `crates/oulipoly-state/src/db/opening_read_only.rs`
- `crates/oulipoly-state/src/db/opening_write.rs`
- `crates/oulipoly-state/src/db/ownership_authority.rs`
- `crates/oulipoly-state/src/db/provider_launch_lifecycle.rs`
- `crates/oulipoly-state/src/mailbox.rs`
- `crates/oulipoly-state/src/mailbox/completion_continuation/attempts.rs`
- `crates/oulipoly-state/src/mailbox/completion_continuation/mod.rs`
- `crates/oulipoly-state/src/mailbox/completion_continuation/notification.rs`
- `crates/oulipoly-state/src/pid_identity.rs`
- `crates/oulipoly-state/src/lifecycle_log.rs`
- `crates/oulipoly-runtime/src/lib.rs`
- `crates/oulipoly-runtime/src/diagnostics/mod.rs`
- `crates/oulipoly-runtime/src/executor/cli/pty_broker/mod.rs`
- `crates/oulipoly-runtime/src/executor/cli/runtime_exit_journal.rs`
- `crates/oulipoly-runtime/src/observability/invocation.rs`
- `crates/oulipoly-runtime/src/trace/mod.rs`
- `crates/oulipoly-runtime/src/services/adapters.rs`
- `crates/oulipoly-runtime/src/services/dtos.rs`
- `crates/oulipoly-runtime/src/services/error.rs`
- `crates/oulipoly-runtime/src/services/lock.rs`
- `crates/oulipoly-runtime/src/services/marker.rs`
- `crates/oulipoly-runtime/src/services/migration.rs`
- `crates/oulipoly-runtime/src/services/mod.rs`
- `crates/oulipoly-runtime/src/services/ports.rs`
- `crates/oulipoly-runtime/src/services/resume.rs`
- `crates/oulipoly-runtime/src/services/session_lifecycle.rs`
- `crates/oulipoly-runtime/src/services/session_warning.rs`
- `crates/oulipoly-runtime/src/services/session_window.rs`
- `crates/oulipoly-runtime/src/services/trace_failure.rs`
- `crates/oulipoly-runtime/src/ports/mod.rs`
- `src-tauri/src/commands/diagnostics/formatter.rs`
- `src-tauri/src/commands/diagnostics/mapper.rs`
- `src-tauri/src/commands/diagnostics/orchestration.rs`
- `src-tauri/src/commands/notify.rs`
- `src-tauri/src/commands/offline_diagnostics.rs`
- `src-tauri/src/dispatch.rs`
- `src-tauri/src/mailbox_delivery.rs`
- `src-tauri/src/completion_owner/root_supervisor.rs`
- `src-tauri/src/completion_owner/original_work.rs`
- `src-tauri/src/completion_owner/control.rs`
- `src-tauri/src/completion_owner/linux.rs`
- `src-tauri/src/completion_owner/j01_private_trace.rs`
- `src-tauri/src/completion_owner/driver.rs`
- `src-tauri/src/wake_coordinator/constants.rs`
- `runtime-caps.json`
- `src-tauri/src/main.rs`
- `src-tauri/src/usage/cli.rs`
- `src-tauri/src/wake_coordinator/sweep/mod.rs`
- `src-tauri/src/wake_coordinator/sweep/candidate.rs`

## Preconditions

- For the legacy runtime diagnostics sink, a configured `StateDb` connection.
- The runtime caller has registered the relevant service adapters at
  startup (via `wiring.rs`); ports + adapters compose the runtime's
  outward-facing service interface.
- For trace operations: an in-flight or completed invocation whose
  inputs, outputs, and timings should be recorded.
- For offline flight-recorder inspection: a configured application data root;
  primary State, PID-mailbox SQLite, provider configuration, and runtime services
  are not preconditions.
- Partitioned SQLite cutover additionally requires the exact durable
  `PRESERVE-LEGACY-V1` marker and preservation manifest. Without that explicit
  offline activation, the legacy JSONL producer remains the normal sink.

## AGE-319 flight-recorder coverage

The version-1 flight recorder is a deliberately incomplete, DB-independent
control-plane witness. The following table is the exact producer map for this
slice; a row is included only where the named source constructs a `SpanStart`.

| Included producer | Recorded operation/resource | Scope represented |
|-------------------|-----------------------------|-------------------|
| Completion registration in `db/ownership_authority.rs` | `completion_registration` / `state_sqlite`, with a parented `completion_authority_registration` / `pid_mailbox_sqlite` child | The State obligation transaction and the PID-mailbox authority/open/fence/registration participant. The shared diagnostic ID does not turn either resource commit into cross-store settlement. |
| Invocation finalization in `db/invocation_lifecycle_finalize.rs` | `invocation_terminal_finalize` / `state_sqlite`, with a parented `invocation_terminal_finalize_completion_authority` / `pid_mailbox_sqlite` child when finalization enters that participant | The selected State terminal transaction and its in-scope completion-sidecar participation, not process exit or delivery. |
| Provider launch lifecycle in `db/provider_launch_lifecycle.rs` | `provider_launch_begin` and `provider_launch_{operation}` / `state_sqlite`, with a parented `provider_launch_transition_completion_authority` / `pid_mailbox_sqlite` child when a transition enters sidecar publication | Begin plus exactly these current transition operation forms: `activate`, `endpoint`, `promotion/{observation_id}`, `transfer`, `certify`, `native-recovery`, `native-custody`, `native-recovered-custody`, `native-runtime-cancellation`, `native-channel-duty-owner`, `complete-native`, `complete`, `settle_cancel`, and `reconcile/{disposition}`. |
| Session admission in `mailbox.rs` | `session_admission_enqueue` and `session_admission_try_admit` / `pid_mailbox_sqlite` | The selected queue/admit `BEGIN IMMEDIATE` transactions. |
| Wake claim in `mailbox.rs` | `wake_claim_acquire` / `pid_mailbox_sqlite` | The selected wake-claim acquisition transaction. |
| Wake recovery in `wake_coordinator/sweep/mod.rs` | `wake_recovery_sweep` / `wake_recovery_orchestration` | The bounded composite sweep outcome; it is not a synthetic SQLite commit. |
| Runner-visible terminal handoff in `mailbox_delivery.rs` | `pty_terminal_handoff` / `runner_visible_handoff`, with observation beginning before mailbox open | The runner-side handoff attempt and bounded outcome; it is not an ACK, process-exit, or full delivery-settlement claim. |
| Root-owned original work in `completion_owner/original_work.rs` | `root_original_work_submit`, `root_original_work_grant`, `root_original_work_cancel`, and `root_original_work_terminal` / `process_tree` | DB-independent initiation/acceptance classification, exact worker grant identity, durable cancellation acceptance, and exact terminal/session-drain integration. Completion listener ACKs remain separate. |

Explicit omissions are every other State or PID-mailbox transaction not named
above, including generic invocation/artifact/session writes, schema/migration and
quota work, mailbox delivery/receipt/ACK writes, wake-claim release, and the
remaining admission mutations. Agent-bash-local pre-runner registration,
pre-spawn, provisional-result, and publication phases outside the paired v1
path are also omitted, as are lifecycle repair, universal SQLite
instrumentation, and Tauri IPC diagnostics. Nested work is covered only when it
has its own named parented span; merely executing inside a composite span does
not make an omitted transaction a recorded participant.

This recorder runs in parallel with three older diagnostic practices and does
not ingest, replace, or reconcile any of them:

- The State `LifecycleEventSink` records and forwards invocation lifecycle
  records through the State-backed lifecycle path. It still requires State and
  has different event semantics.
- The native runtime-exit journal retains provider-generation exit intent,
  predecessor, and result artifacts for its custody protocol. Those artifacts,
  not recorder events, remain authoritative for that protocol.
- Opt-in legacy `notify-trace.log` is a shared textual notify/PTY/wake trace with
  its existing shared rotation behavior. Its claim-token field is now only the
  constant presence marker `redacted` or `none`, but this slice does not redesign
  that file's writer/rotation authority and the offline reader does not read it.
  Cross-producer rotation/deletion of that legacy evidence remains an explicit
  residual authority and retention risk outside this slice.

## AGE-369 SQLite observability extension

SQLite events retain stable repository-owned operation and query-family labels,
one of the typed `state` / `pid_mailbox` / `pid_identity` database roles, and a
non-path classification (`managed_file`, `read_only_snapshot`, `memory`, or
`external_file`). Raw paths, SQL text, expanded SQL, and bound parameters are
not event fields. SQLite error messages are reduced to a constant safe summary;
primary and extended result codes retain the actionable classification.

Existing instrumented `BEGIN IMMEDIATE` boundaries separately measure time in
the begin call (writer-authority/busy wait), transaction statement work, commit,
and post-commit work before release. Missing measurements use typed gap reasons
instead of zeroes or estimates. Autocommit write APIs report one combined
statement duration and explicitly classify writer wait versus VM execution as
not separable. Changed/returned row counts are recorded only from direct API
results; SQLite's unavailable rows-examined count stays absent with an explicit
gap.

New connection-open and root-supervisor continuation statement observations use
a late-emitting policy: failures, contention, and operations at or above the
100-ms default threshold are retained; ordinary fast successes are dropped by
default. `OULIPOLY_SQLITE_OBSERVABILITY=off` disables this layer, `all` retains
all normal events, `OULIPOLY_SQLITE_SLOW_MILLIS` changes the threshold (bounded
to 60 seconds), and `OULIPOLY_SQLITE_SAMPLE_EVERY=N` retains one ordinary fast
success in every bounded N. Late retained records are handed off synchronously
only after the observed database call has returned; database-held transaction
phases continue to use the bounded deferred queue. Queue/retention gaps keep the
AGE-319 fail-open policy.

`OULIPOLY_SQLITE_QUERY_PLANS=true` opts retained root-supervisor reconciliation
read observations into bounded `EXPLAIN QUERY PLAN` shape collection. Evidence
contains at most 64 nodes and only operator categories/counts; SQLite detail
strings, table/index names, SQL templates, and bound values are discarded. Plan
collection is off by default, is skipped for filtered normal events, and never
changes query results or adds a retry/deadline.

## AGE-371 timestamp and retention semantics

Flight-recorder schema v2 keeps UTC wall-clock occurrence in `recorded_at` and
copies that exact value to `retention_eligible_at`; monotonic
`elapsed_micros` remains the only latency measure. Schema-v1 records decode as
explicitly `legacy_unknown`. JSONL shard mtime is never event age authority.

Cleanup status generations now carry `started_at`, `completed_at`, and explicit
eligibility. An inactive shard whose mtime cannot be read is retained as
uncertain, and aggregate file-count pressure cannot retire a shard below the
stale-age threshold. Rotated 30-day diagnostic storage remains later work.
Lifecycle sink records likewise carry UTC `recorded_at` / identical
`retention_eligible_at` while preserving monotonic `latency_us`. Runtime trace
reports are derived views rather than an independent retained trace family; no
general durable metrics family is selected. Native exit journals and opt-in
notify/overlay/TUI-profile files remain retention-ineligible unless a later
family-specific contract supplies authoritative clocks.

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| Runtime emits a per-attempt diagnostics record. | `diagnostics/mod.rs` formats a `DiagnosticsEnvelope` carrying provider/account/signal/reason; persisted via the trace surface. |
| Service caller invokes a port through the adapter layer. | Adapter translates the DTO, calls the underlying repository/service, returns a typed result. |
| A typed service error is raised. | `services/error.rs` defines the variant; downstream caller pattern-matches. |
| Session warning is emitted (e.g. window approaching exhaustion). | `services/session_warning.rs` records a structured warning that does not abort the invocation. |
| Trace failure occurs (e.g. trace sink is unreachable). | `services/trace_failure.rs` emits a typed `TraceFailure` carrying the cause without surfacing through the user-facing return path. |
| Marker handling (OULIPOLY marker in stream). | `services/marker.rs` parses + buffers; downstream consumers (recognizer) read parsed markers, never raw bytes. |
| Heuristic category fallback sees generic auth wording. | `diagnostics/mod.rs` classifies only specific auth-expired sentinels such as `unauthorized` and `token expired`; generic `auth` text is left for more-specific classifiers or falls through rather than becoming `AuthExpired`. |
| An external diagnostics model executes through `RuntimeDiagnosticsService`. | The service reuses its populated provider registry for the external model while built-in diagnostics remain on the registry-free executor path. |
| Primary provider execution fails and fallback diagnostics also fails. | The primary exit, terminal reason, stderr, invocation, and provider-session identity remain authoritative; the secondary failure emits one `OULIPOLY_DIAGNOSTIC_FAILURE` marker with its operation and settles a typed non-null error category. |
| A live-only observability snapshot reads chronological logical children after terminal history fills the invocation cap. | The invocation projection stably prioritizes durable `Running` candidates before terminal candidates, then applies the existing finite traversal cap and exact PID/boot/start-time liveness validation. Terminal-inclusive snapshots retain the chronological child order. |
| `diagnostics recent --limit N [--json]` runs while State and PID-mailbox SQLite are unavailable or held in valid live write transactions. | The process routes before completion-owner bootstrap, recovery, tracing initialization, or runtime-service construction; one bounded inspection generation of the default flight-recorder root produces both the raw recent failures and their coalesced groups, with the same retained coverage and reader issues. It does not mutate the held stores. |
| `diagnostics trace <diagnostic-id> [--json]` runs while State and PID-mailbox SQLite are unavailable. | The process routes through the same offline boundary and reports only retained source records for the diagnostic ID plus coverage/issues; an empty retained trace is not represented as database or delivery success. |
| A selected State or PID-mailbox control-plane transaction is attempted. | Its top-level resource-scoped span synchronously appends `Requested` before the real `BEGIN IMMEDIATE`, then submits only phases actually observed: `Acquired`, `CommitStarted`, `Committed`, contention/failure, and `Released`. A nested sidecar child submits its `Requested` observation nonblockingly before sidecar authority/open/fence work because the parent database writer is already held. SQLite begin/commit failures retain primary and extended codes plus configured busy timeout and observed wait; recorder failure never changes the transaction result. |
| Preservation has been durably activated for a producer process. | Eligible diagnostic and lifecycle observations receive one normalized event ID/digest before fan-out. The process-local SQLite/WAL writer is the normal sink; shadow mode or SQLite failure/backpressure writes that same envelope to the process's uniquely named exact-current JSONL shard without changing State/PID-mailbox results. |
| Preservation is absent or its exact marker is invalid. | Absence keeps the legacy JSONL path active. An invalid marker fails safe against historical cleanup and does not authorize SQLite cutover. Neither case causes root-process history enumeration or import. |
| Invocation lifecycle telemetry is emitted. | A typed allowlist parses invocation identity, hashes raw session identity, converts artifact paths to role/fingerprint evidence, normalizes/redacts bounded scalar values, validates the AGE-371 record timestamp/eligibility projection, and constructs the event envelope before SQLite/JSONL sink selection. The envelope occurrence is the record's authoritative `recorded_at`; record-level retention fields are not duplicated into its payload, and the pre-existing lifecycle callback retains its input shape and authority behavior. |
| A typed SQLite observation is retained. | The event includes a stable query family, database role/path class, transaction mode and phase, observed operation elapsed time, only honestly separable sub-durations/counts, explicit measurement gaps, typed SQLite result codes on failure, and existing diagnostic/invocation correlations. It never contains SQL, expanded SQL, bound values, credentials, or raw database paths. |
| Root-supervisor continuation acceptance, custody attachment, phase advance, or bounded reconciliation scan meets a retention condition. | The PID-mailbox statement is correlated by hashed attempt/owner/supervisor identity. Autocommit writes report exact rows changed and classify writer wait/execution as inseparable; the read reports execution time and exact rows returned while leaving rows examined unavailable. |
| Completion registration crosses State and PID-mailbox authority. | State and sidecar use distinct parent/child spans under one diagnostic ID. State `Released` occurs after its commit and before sidecar commit; neither resource commit is represented as delivery, ACK, process exit, or complete cross-store settlement. |
| Launch admission, provider launch, wake claim, terminal finalization, or wake recovery runs. | The producer attaches a bounded lifecycle phase while retaining the exact resource/effect scope. Wake-recovery sweep and runner-visible terminal handoff are composite orchestration spans and do not synthesize SQLite `Committed` phases. |
| Legacy completion wake tracing observes a claim token. | The trace preserves a constant `redacted`/`none` presence marker and never formats, hashes, or writes the capability value. |

## Edge cases

- Diagnostics write fails (state DB locked) — caller should not panic;
  the diagnostics module returns an error variant and the runtime
  continues without diagnostic side effects.
- Trace serialization encounters a non-UTF-8 byte sequence — fallback to
  base64 envelope per the typed schema.
- Service window query overlaps the boundary of a session migration —
  `services/session_window.rs` returns the union of pre- and
  post-migration windows.
- Unknown or ambiguous failure text that contains the substring `auth`
  but lacks a specific expired/unauthorized sentinel must not be upgraded
  to `AuthExpired`; this preserves unknown observability and avoids hiding
  provider failures behind an overbroad auth bucket.
- A secondary external-provider protocol failure such as `registry_lookup`
  must not replace the primary provider exit or synthesize an agent result;
  its structured diagnostics marker records the secondary operation.
- Durable `Running` status only changes candidate order in a live-only
  projection. Missing, dead, or mismatched process identity still fails
  closed, and unrelated invocations remain outside the logical subtree.
- A truncated or unsupported flight-recorder record does not make offline
  inspection fail as a whole. Output preserves the readable source records and
  reports the ignored/truncated source coordinates and retained coverage.
- `retention-status.json` is a separately bounded input. The reader opens it
  directly and reads at most `MAX_RETENTION_STATUS_READ_BYTES` (64 KiB), without
  allocating or deserializing the rest of an oversized file. It reports
  `oversized_retention_status`, `truncated_retention_status` (missing the
  completed-record newline), or `invalid_retention_status` distinctly, and
  exposes `retention_status_bytes_read`, `retention_status_bytes_skipped`, and
  `retention_status_limit_reached` independently of shard byte coverage.
- Missing/unreadable recorder roots, a shard that disappears or whose pathname
  resolves to a replacement file identity after discovery, growth beyond the
  observed byte budget, work omitted by the discovery/shard/byte bounds, and an
  unsatisfied aggregate retention target are explicit coverage issues. On the
  Linux/WSL evidence path, a discovery-time device/inode identity must match the
  descriptor opened for reading; a mismatch is skipped as
  `concurrent_replacement`. A bounded query is one generation of best-effort
  retained evidence, not a filesystem snapshot.
- A missing or unreadable shard mtime is unknown evidence, never an epoch-age
  substitute. A wall-clock regression yields no eligible age, and aggregate
  shard pressure does not override this conservative result.
- Raw and coalesced recent failures are derived from the same in-memory
  inspection. Unique record event IDs are deduplicated; a redacted cause/failure
  signature prevents unrelated generic failures from being merged solely by
  operation/resource/phase.
- State or sidecar `BEGIN IMMEDIATE` contention preserves the operation's
  original lock error. The synchronously appended top-level `Requested` survives
  independently of the database result; queued Contention/Released evidence
  retains typed SQLite codes when the writer drains those later phases.
- An acquired transaction that exits through an early error still emits a
  bounded static failure category and `Released`; it does not manufacture a
  commit, rollback success, or broader lifecycle outcome.
- Completion capabilities, wake/claim/lease tokens, cursors, credentials,
  prompts, transcripts, payloads, environment bodies, and arbitrary paths are
  excluded from producer correlations and cause text.
- Recorder initialization, queueing, append, or rotation failure remains
  fail-open for the protected operation and never changes its result. The process
  may retry initialization and attempts at most one bounded, non-secret
  diagnostic-gap notification per failed stage; a gap notification is not
  operation-failure or operation-success evidence. Nonblocking enqueue failure
  uses the exact bounded stages `deferred_queue_full` and
  `deferred_queue_disconnected`;
  synchronous writer-channel loss uses `writer_disconnected`. Before resolving
  the process-recorder root, initialization makes one best-effort attempt to
  start the process-wide reporter; `FlightRecorder::open` makes the same
  idempotent attempt for directly constructed recorders. At most one reporter
  worker owns stderr and its bounded `GAP_REPORTER_QUEUE_CAPACITY` (16) channel.
  Every operation and recorder-writer path performs only `try_send` against an
  already observed sender and never writes stderr itself. A missing, full, or
  disconnected reporter, reporter-start failure, or stderr write failure drops
  diagnostic visibility without blocking, panicking, retrying indefinitely, or
  changing protected work. A deferred/database-held producer only atomically
  marks the stage pending and immediately returns its exact status; pending
  stages receive their one nonblocking reporter-handoff attempt when the recorder
  writer next makes progress or at the next top-level pre-database `Requested`
  boundary. Once-per-stage therefore means at most one handoff attempt, not proof
  that stderr accepted or retained the notification.
- Event-store append is evidence-only. A top-level pre-transaction observation
  may wait after bounded queue admission for its SQLite commit; observations
  emitted while a State/PID-mailbox writer may be held use nonblocking enqueue.
  Accepted best-effort work hands its pending reply and exact envelope to the
  bounded recorder worker without waiting. Worker-side unknown writes,
  identity/append failures, and reply loss route that envelope to emergency
  JSONL; successful normal-mode commits do not shadow-write. Completion-handoff
  saturation/disconnection and fallback failure produce explicit diagnostic
  gaps and never alter the protected operation.
- Under the exact preservation marker, legacy cleanup performs no directory
  scan or deletion. Rotation syncs and closes only the exact active shard and
  opens a create-once, uniquely named successor; the four-shard, seven-day, and
  aggregate cleanup policies are suspended until AGE-372/AGE-377 retirement.
  Startup marker selection plus active-shard creation/lease and activation
  inventory are serialized by the retention lock. A legacy writer also
  rechecks the marker under that lock before every destructive rotation and
  switches to create-once rotation when marker state is present or unreadable.
- The default recorder has a bounded deferred queue of
  `DEFAULT_DEFERRED_QUEUE_CAPACITY` (1024), configurable as
  `RecorderConfig::deferred_queue_capacity`. A top-level `Requested` waits for the
  writer and returns `RecordStatus::Appended` only after append/flush; all
  non-Requested events and nested sidecar-child `Requested` events use
  nonblocking queue submission and return `RecordStatus::Queued` when accepted.
  The writer thread alone performs their regular-file append/rotation work, so an
  already-held State or PID-mailbox writer performs no recorder file I/O. Queued
  means accepted for deferred writing, not appended or crash-durable: abrupt
  process exit may lose a later phase that was accepted but not drained, while
  the earlier synchronous top-level `Requested` remains the retained boundary.
- Each process keeps an advisory lease on its active shard for the writer
  lifetime and transfers that lease on rotation. Aggregate cleanup first holds
  the nonblocking cleanup lease, then obtains a candidate shard lease
  nonblockingly before deleting only stale, confidently inactive shards.
  Live, locked, or identity-uncertain shards remain; coverage/retention status
  reports when those shards prevent the aggregate target from being met. Startup
  cleanup examines at most `MAX_CLEANUP_DIRECTORY_ENTRIES` (1024) raw directory
  entries and retains at most `MAX_CLEANUP_ISSUES` (128) issue strings of at most
  `MAX_CLEANUP_ISSUE_BYTES` (64) bytes each. Status serialization is checked
  against the same 64-KiB reader bound before the temporary file is written.
  `directory_entries_examined`, `directory_entries_unreadable`,
  `directory_limit_reached`, `issue_limit_reached`, and `issues_omitted` expose
  that work. Reaching the directory cap or encountering an unreadable entry
  or omitting issue detail makes `aggregate_limit_satisfied` false because
  incomplete coverage cannot prove the aggregate target, while discovered
  held/live/uncertain shards remain fail-closed against deletion.
- Default writer retention is at most four one-MiB shards per process (one
  active plus rotations), with a best-effort aggregate target of 64 shards and
  a seven-day stale-age threshold. Reader work has four separate bounds. It
  reads at most 64 KiB of retention status, examines at most
  `MAX_INSPECTION_DIRECTORY_ENTRIES` (1024) raw directory entries before
  metadata sorting, then selects at most 256 matching shards and reads at most
  64 MiB total shard content; it rejects any individual record over one MiB.
  `directory_entries_examined` and `directory_limit_reached` describe discovery;
  `inspection_directory_limit` means unexamined entries may be omitted and does
  not fabricate `files_skipped` or `bytes_skipped` for names/metadata it did not
  read. Existing file/byte skipped fields describe only discovered content
  excluded by the selected-shard or byte caps.

## Error conditions

- `DiagnosticsWriteFailed` — DB write into the diagnostics log failed.
- `ServiceAdapterError` — adapter could not translate DTO ↔ domain type.
- `TraceFailure` (typed) — trace emission failed; non-fatal.
- `PortNotRegistered` — caller invoked a port whose adapter is not
  registered (programmer error; should never ship).

## Boundaries

- Diagnostics does NOT decide whether to retry — it records; the
  balancer decides.
- Diagnostics does NOT create new user-facing categories for AGE-175
  unknown observability; `unknown` remains the settled category when no
  narrower classifier applies.
- Trace does NOT mutate session state — it is a side-channel sink.
- Services / ports layer is the contract surface for the runtime; it
  does NOT bypass `oulipoly-state` for persistence.
- Service adapters do NOT call provider executables — that is the
  executor's domain.
- Observability does NOT infer authority or liveness from PPID ancestry,
  bare PID, recency, cwd, or filenames.
- Offline flight-recorder inspection does NOT open, reconcile, migrate, or infer
  success from primary State or PID-mailbox SQLite; it does not bootstrap a
  completion owner, run wake recovery, construct provider/runtime services, or
  treat recorder append/read success as database, notification, receipt, ACK,
  process-terminal, or settlement success.
- The flight recorder observes existing authority boundaries; it does NOT move
  completion, launch, wake, terminal, root-supervisor, or repair authority.
- A transaction `Committed` phase belongs only to its named SQLite resource.
  Composite wake recovery and runner-visible handoff use terminal/released
  outcomes without claiming a database commit.
- Top-level `Requested` persistence may synchronously wait for the recorder writer
  only before the protected database attempt. Once State or PID-mailbox writer
  authority is held, later and nested-child observations are queue submissions;
  their producer path performs no recorder regular-file append, flush, or
  rotation and their queued status does not assert durability.
- Recorder files are private same-UID artifacts, not a security boundary against
  a malicious same-UID peer. A record ID, diagnostic ID, span relationship, or
  coalesced group grants no completion, launch, wake, repair, replay, delivery,
  ACK, reap, or settlement authority.
- The implementation uses portable filesystem primitives where practical, but
  the candidate validation and emergency operational-evidence claim are bounded
  to Linux/WSL. This specification makes no macOS/Windows operational-evidence
  claim from the Linux/WSL witnesses.
- Reverting the source does not erase
  `diagnostics/flight-recorder-v1`. Retained, private, schema-versioned artifacts
  are inert when reverted code does not read them; this slice performs no
  rollback deletion. A later deletion request requires a separately identified,
  bounded cleanup command with exact ownership rather than an implicit source
  revert or broad directory removal.
- This diagnostic slice does not repair inherited completion-endpoint succession.
  A live managed descendant can still retain an older root's endpoint and fail to
  join it without becoming an in-tree successor; a fresh independent entry may
  recover later. Flight-recorder evidence observes that pre-existing lifecycle
  boundary but does not transfer or repair its authority.

## Declared test patterns

Per `~/ai/conventions/testing.md`: parity tests per service, contract
tests on the ports surface, fixture tests on the trace envelope schema.

- `crates/oulipoly-runtime/tests/age144_trace_output_schema_doc.rs`
- `crates/oulipoly-runtime/tests/age34_runtime_diagnostics_service_routing.rs`
- `crates/oulipoly-runtime/tests/age_149_typed_trace_failure_characterization.rs`
- `crates/oulipoly-runtime/tests/age37_trace_service_parity.rs`
- `crates/oulipoly-runtime/tests/ports_contract.rs`
- `crates/oulipoly-runtime/tests/observability_snapshot.rs`
- `crates/oulipoly-runtime/tests/service_traits_compile.rs`
- `crates/oulipoly-runtime/tests/resume_service_parity.rs`
- `src-tauri/tests/age27_diagnostics_effective_provider.rs`
- `src-tauri/tests/age289_registry_diagnostic_failure.rs`
- `src-tauri/tests/age_54_trace_row_preservation.rs`
- `src-tauri/tests/pr_b_trace_integration.rs`
- `src-tauri/tests/pipeline_status_propagation_rca/age158_characterization.rs`
- `src-tauri/tests/pipeline_status_propagation_rca/age158_rc1_characterization.rs`
- `src-tauri/tests/age175_failure_response_identity.rs`
- `src-tauri/tests/pipeline_status_propagation_rca/rc1_abnormal_termination_under_tail_pipeline.rs`
- `src-tauri/tests/initiative_09_internal_unification.rs`
- `src-tauri/tests/age319_offline_diagnostics_cli.rs`
  - unavailable leaf paths prove that the early route is independent of
    available primary stores, not that no SQLite open syscall was attempted;
  - valid State and PID-mailbox SQLite files held in real write transactions
    prove that the compiled early CLI succeeds and leaves their database/journal
    bytes and live uncommitted rows unchanged.
- `crates/oulipoly-state/src/db/provider_launch_lifecycle.rs` — actual State
  contention proves Requested precedes typed Contention without changing the
  original error.
- `crates/oulipoly-state/src/mailbox.rs` — actual PID-mailbox contention proves
  the same ordering and code preservation at the admission producer.
- `crates/oulipoly-state/src/diagnostic_recorder.rs` — deferred-writer tests
  distinguish `Appended` from `Queued`, use an explicit drain/flush witness for
  later phases, and exercise nonblocking queue saturation plus nested-child
  submission without entering reporter handoff while the writer is blocked.
  Full and disconnected reporter fixtures prove top-level closures still run
  with unchanged results, and a failing sink fixture proves reporter write
  errors are ignored; process-recorder initialization failure remains handed off
  because reporter initialization precedes recorder-root resolution. The
  reader tests separately exercise the hard status/directory-entry caps and
  discovery/open file-identity replacement, while cleanup tests reach both its
  discovery and persisted-issue bounds. The independent abrupt-exit witness
  establishes the retained top-level Requested/crash-tail boundary, not survival
  of undrained queued phases.
- `crates/oulipoly-state/tests/provider_launch_lifecycle.rs`
- `src-tauri/src/commands/notify.rs` — sentinel token-redaction formatting test.
- `src-tauri/src/wake_coordinator/sweep/mod.rs`

## Cross-references

- `planning/coverage/spec-state-db.md` — diagnostics sink + service
  repository layer.
- `planning/coverage/spec-session-lifecycle.md` — session_lifecycle
  service is a top consumer.
- `planning/coverage/spec-executor.md` — executor emits diagnostics
  inputs.
- `planning/coverage/spec-result-envelope.md` — consumes settled
  `unknown` diagnostics for the AGE-175 structured stderr marker.
- `planning/coverage/spec-event-storage.md` — accepted successor sink for
  non-authoritative diagnostic, trace, metric, and log records; the current
  JSONL recorder remains the emergency fallback during cutover.
- `AGENTS.md` § Rust Workspace Structure.
