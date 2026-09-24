# Live/history access inventory

This inventory is the AGE-373 source of truth for SQLite structures reached by
launch, startup/recovery, notification delivery, and root-supervisor work. The
machine-readable entries live in
`crates/oulipoly-state/src/live_history.rs::ACCESS_INVENTORY`; a unit test keeps
the identifiers below synchronized.

The classes have operational meaning:

- **live authority** may be accessed synchronously by protected live paths;
  predicates express durable state, never absence or age.
- **bounded cross-boundary** shares a physical table with retained history and
  is allowed only through a typed API with an explicit positive bound and an
  index-backed plan test.
- **historical/diagnostic** requires an explicitly historical repository handle.
  A live handle rejects it before SQLite access and emits an AGE-369 flight
  recorder event. These operations are never implicit post-commit work.

Physical read-only mode does not imply historical authority: generic
`open_read_only*` constructors remain live-scoped for notification and wake
coordination. Diagnostics must select the explicitly named
`open_historical_read_only*` boundary.

## Tables

| Inventory ID | Class | Live contract |
|---|---|---|
| `table.invocations` | bounded cross-boundary | Running seeds use the durable `status='running'` partial index; exact ancestors use primary-key lookup. Terminal rows remain history and are not a zero-work seed. |
| `table.provider_logical_launches` | bounded cross-boundary | Live cancellation recovery selects the explicit `status='cancelling'` projection; terminal status is durable, not inferred. |
| `table.provider_launch_attempts` | bounded cross-boundary | Live access is by exact attempt/logical-launch/invocation identities. |
| `table.provider_launch_transition_replays` | historical/diagnostic | Append-only launch replay evidence. Live retirement uses the normalized current-duty projection, never suffix matching over operation history. |
| `table.provider_launch_native_channel_duties` | live authority | One normalized current row per retained native-channel duty; exact domain checks never scan replay history. |
| `table.completed_turns` | bounded cross-boundary | `recovery_pending` is updated from explicit committed tail dispositions. Missing, old, or corrupt evidence remains pending. |
| `table.completed_turn_recovery_epoch` | live authority | High-water mark and revision flag a newly admitted older invocation behind a recovery cursor. |
| `table.invocation_completion_obligations` | bounded cross-boundary | Live readback is by derived admission key or indexed event identity; the append-only full ledger is diagnostic. |
| `table.invocation_completion_continuity` | bounded cross-boundary | Exact admission join and bounded ordinal suffix are the only live interfaces. |
| `table.completion_authority_continuity` | bounded cross-boundary | Sidecar repair and retirement compare the exact append-only head or read only the bounded suffix selected by State authority. |
| `table.invocation_completion_v2_identity` | bounded cross-boundary | Immutable source identity projection used for exact conflict probes without decoding the admission ledger. |
| `table.mailbox` | bounded cross-boundary | Delivery uses `delivered_at IS NULL`, a monotonic cursor, and a positive page limit. Delivered rows are history. |
| `table.mailbox_delivery_attempts` | bounded cross-boundary | Live delivery/recovery uses exact attempt/session plus unresolved predicates. Resolved rows are terminal history. |
| `table.mailbox_delivery_attempt_items` | bounded cross-boundary | Live settlement joins an exact attempt to its finite submitted sequence set; it is never a global history seed. |
| `table.completion_event` | bounded cross-boundary | Notification/recovery use exact event identity and explicit durable event state. |
| `table.completion_event_listener` | bounded cross-boundary | Idle-close uses the explicit retirement projection; acknowledgement and policy reconciliation use exact event/listener or mailbox identities. |
| `table.runtime_generation` | bounded cross-boundary | Coordination uses exact generation/session/process identity and explicit lifecycle state. |
| `table.session_wake_claim` | live authority | One current claim per session; claim absence is never terminal evidence for another obligation. |
| `table.completion_continuation_owner` | live authority | The partial unique running-owner index is current-root authority. |
| `table.completion_continuation_source` | bounded cross-boundary | Recovery reads only durable `phase='registered'` source obligations in current/inherited authority scope. |
| `table.completion_continuation_attempt` | bounded cross-boundary | Recovery reads only phases outside `drained`/`never_started` in current/inherited authority scope. |
| `table.completion_continuation_notification` | bounded cross-boundary | Presentation policy reconciliation is exact by event/listener; retained policy history is never a live scan seed. |
| `table.completion_continuation_domain` | live authority | Exact current completion-domain identity and lifecycle authority. |
| `table.completion_supervisor_authority` | live authority | Exact current/root supervisor authority rows; historical reachability is expressed only by explicit inheritance edges. |
| `table.completion_supervisor_inheritance` | live authority | Explicit durable authority edges define inherited scope; ancestry is not inferred from old rows. |

## Indexes

| Inventory ID | Class | Protected use |
|---|---|---|
| `index.idx_invocations_running_parent` | live authority | Seeds the current live invocation forest without terminal-history scan. |
| `index.idx_invocations_parent_running_created` | bounded cross-boundary | Bounded direct-child projection with live rows first. |
| `index.provider_launch_nonterminal` | historical/diagnostic | Legacy all-status index; live paths must not use it as a current projection. |
| `index.provider_launch_cancelling` | live authority | Exact current cancellation set used by supervisor retirement revalidation. |
| `index.provider_launch_native_channel_duty_domain` | live authority | Exact current native-channel duty by completion domain. |
| `index.completed_turns_recovery_pending` | live authority | Partial completed-turn recovery working set; terminal rows are absent. |
| `index.completed_turns_recovery_target` | live authority | Partial provider/session target seek for pending recovery and chain refusal. |
| `index.completed_turns_recovery_session` | live authority | Partial session seek for legacy manual-resume refusal. |
| `index.idx_invocation_completion_obligations_legacy` | live authority | Startup detects explicit legacy NULL bindings without reading v2 history. |
| `index.idx_invocation_completion_obligations_event` | bounded cross-boundary | Exact event conflict/readback for immutable admissions. |
| `index.idx_invocation_completion_continuity_head` | live authority | State continuity-head lookup is a one-row descending index read. |
| `index.idx_completion_authority_continuity_head` | live authority | Sidecar continuity-head lookup is a one-row descending index read. |
| `index.idx_invocation_completion_v2_registration` | bounded cross-boundary | Exact registration-incarnation conflict probe. |
| `index.idx_invocation_completion_v2_source` | bounded cross-boundary | Exact domain/source conflict probe. |
| `index.idx_invocation_completion_v2_handle` | bounded cross-boundary | Exact domain/handle conflict probe. |
| `index.idx_mailbox_pending` | historical/diagnostic | Legacy non-partial index retained for compatibility; not a live projection. |
| `index.idx_mailbox_pending_target` | historical/diagnostic | Legacy non-partial target index retained for compatibility; not a live projection. |
| `index.idx_mailbox_pending_session_live` | historical/diagnostic | Undelivered-row diagnostic cursor; it may include explicitly retired error rows and is not used by protected delivery/supervisor scans. |
| `index.idx_mailbox_pending_target_live` | historical/diagnostic | Undelivered target diagnostic cursor; it may include explicitly retired error rows and is not used by protected delivery/supervisor scans. |
| `index.idx_mailbox_deliverable_session_live` | live authority | Session cursor excluding delivered rows and all explicit terminal error dispositions. |
| `index.idx_mailbox_deliverable_target_live` | live authority | Session/chain target cursor excluding delivered rows and all explicit terminal error dispositions. |
| `index.idx_mailbox_deliverable_global` | live authority | Global deliverable sequence used by capped session discovery and idle close; explicit terminal errors are absent. |
| `index.idx_mailbox_delivery_attempt_unresolved` | live authority | Unresolved attempt settlement and recovery by session. |
| `index.idx_completion_event_listener_session_live` | live authority | Exact-session listener activation contains only explicit retirement obligations. |
| `index.idx_completion_event_listener_retirement_pending` | live authority | Explicit pending-retirement listener projection; response-only/no-notification rows, acknowledged rows, and explicitly terminal mailbox-error rows are absent. |
| `index.idx_runtime_generation_live_session` | live authority | Explicit non-exited runtime projection by session. |
| `index.completion_continuation_owner_running` | live authority | Current completion owner. |
| `index.completion_continuation_attempt_unresolved` | live authority | Current/inherited unresolved supervisor attempts. |
| `index.completion_continuation_attempt_native_runtime` | bounded cross-boundary | Exact native runtime/invocation identity probe for cancellation, drain, and supervisor retirement without attempt-history scans. |
| `index.completion_continuation_source_unaccepted` | live authority | Current/inherited registered sources. |
| `index.idx_mailbox_terminal_retention` | historical/diagnostic | Explicit terminal-mailbox retention only. |
| `index.idx_mailbox_delivery_attempt_terminal_retention` | historical/diagnostic | Explicit resolved-attempt retention only. |

## Statements and bounded interfaces

| Inventory ID | Class | Bound and plan evidence |
|---|---|---|
| `statement.invocations.live_subtree_children` | bounded cross-boundary | Positive caller limit; production query constant; tests require the running partial index plus primary-key ancestor lookup and compare small/large terminal history. |
| `statement.mailbox.pending_delivery` | bounded cross-boundary | Positive page limit and monotonic `seq` cursor; tests require both pending indexes and zero-work behavior after terminal-history saturation. |
| `statement.mailbox.pending_sessions` | bounded cross-boundary | Finite AGE-370 wake scan cap; oldest/newest selection uses only the global pending partial index. |
| `statement.mailbox.pending_exact` | live authority | Primary-key read for an explicitly submitted row; never searches a backlog. |
| `statement.mailbox.pending_count` | live authority | Exact count over the three mutually exclusive pending projections; preserves delivery diagnostics without materializing or hashing the backlog. |
| `statement.mailbox.full_listing` | historical/diagnostic | Requires a historical mailbox handle; live-handle attempts are rejected and attributed through AGE-369 diagnostics before SQLite access. |
| `statement.mailbox.resolve_unresolved_delivery_attempts` | live authority | Short writer mutation seeded only by the unresolved partial index. |
| `statement.completed_turns.recovery_pending` | live authority | Explicit recovery disposition and a partial pending index plus primary-key invocation join keep terminal rows outside the working set. |
| `statement.completed_turns.recovery_target` | live authority | Indexed provider/session or exact session seek finds one conflict; chain walks use the target index per segment. |
| `statement.completed_turns.recovery_page` | live authority | A 101-row pending-index seek returns at most 100 identities, a continuation cursor, and a restart signal for older arrivals. |
| `statement.completion_continuation.exact_admission` | live authority | Derived admission primary key with one continuity join; immutable registration/source/handle conflicts use exact indexed identity projections. |
| `statement.completion_continuation.continuity_suffix` | bounded cross-boundary | Positive caller limit after the durable sidecar continuity ordinal; the obligation join is by admission primary key. |
| `statement.completion_continuation.legacy_probe` | live authority | Partial NULL-binding existence probe; it does not infer terminality. |
| `statement.provider_launch.native_channel_duty_for_domain` | live authority | Normalized exact-domain `EXISTS`, replacing the replay suffix scan. |
| `statement.supervisor.close_idle_live_obligations` | live authority | Idle close checks registered sources, explicit listener-retirement obligations, pending mailbox, and unresolved attempts through terminal-excluding projections. |
| `statement.completion_continuation.pending_for_supervisor` | bounded cross-boundary | Positive page limit and lexical cursor over explicit current/inherited authority; tests require unresolved/source partial indexes. |
| `statement.completion_continuation.native_runtime_identity` | bounded cross-boundary | Exact `(runtime_generation_uuid, spawn_invocation_uuid[, domain_id])` lookup through the activation-only identity index. |
| `statement.completion_continuation.full_admission_ledger` | historical/diagnostic | Full immutable ledger enumeration remains available only for audit/test tooling, never exact live readback. |

Exact-key notification reads and writes are live authority. Diagnostic queries
that intentionally return terminal rows (mailbox `--all`, notification history,
trace/history views) are historical/diagnostic and must not be introduced into a
protected live call graph.

## Non-SQLite original-work artifacts

`original-work-v1` adds no table, query, history scan, or maintenance job. Its
intent, exclusive acceptance, durable cancellation, terminal/drain result, and
JSONL diagnostics live inside the exact private agent-bash handle directory.
The diagnostic JSONL has a handle-local lock, a 1 MiB active generation, one
rotated generation, and a 128 KiB record bound. The directory is the retention
unit. The bounded agent-bash startup reaper evaluates terminal state, delivery
obligation, exact process custody, age, and its existing per-pass directory cap.
A current-v2 source additionally needs the separate, digest-bound
`source-retention-release-v1.json` witness; a nested child remains retained
while its exact parent handle exists. For an accepted root handle, the private
`root-work-result-v1.json` must also be durable before reaping can safely remove
the handle; that root result follows its accepted child results. This gate
belongs to the Bash reaper and requires exact-pair
verification with the runner's result integration. The guardian does not scan handle history,
while startup can read bounded per-handle source/reaper evidence. See
[`root-original-work-v1.md`](root-original-work-v1.md).

## Maintenance

| Inventory ID | Class | Transaction/external-work rule |
|---|---|---|
| `maintenance.terminal_history.retention_stats` | historical/diagnostic | Read-only explicit command; never runs after a live commit. |
| `maintenance.terminal_history.payload_compaction` | historical/diagnostic | Candidate scan and payload filesystem publication/verification precede each short revalidation/write transaction. |
| `maintenance.terminal_history.payload_compaction_stats` | historical/diagnostic | Explicit read-only sizing operation on a historical handle. |
| `maintenance.terminal_history.prune` | historical/diagnostic | Indexed age/eligibility candidate scans precede one short zero-wait revalidation transaction per row; payload reclamation uses its independent content-addressed fence outside writer ownership. |
| `maintenance.terminal_history.vacuum` | historical/diagnostic | Explicit operation only; requires historical scope and never runs from startup, delivery, recovery, or supervisor coordination. |
| `maintenance.record_timestamps.state_terminal_repair` | historical/diagnostic | Explicit audited repair only. A live handle is rejected before SQLite; the historical transaction appends the immutable audit row and changes the terminal/eligibility projection atomically. |
| `maintenance.record_timestamps.sidecar_terminal_repair` | historical/diagnostic | Explicit audited repair only. A live sidecar handle is rejected before SQLite; the historical transaction appends the immutable audit row and changes the terminal/eligibility projection atomically. |

Schema upgrade/repair, manual migration/backfill, full mailbox listing, and
offline diagnostics remain historical/diagnostic operations outside live
traces. AGE-372 owns implemented retention policy/bounded coordination
operations; AGE-376 provides atomic event heads, eligibility metadata, leases,
and receipt formats; and AGE-377 owns detached scheduling, singleton leases,
and destructive event-generation execution.
AGE-373 only removes implicit maintenance and establishes the access boundary
they must use.

AGE-375 selected independent producer-partitioned SQLite/WAL generations for
non-authoritative diagnostic events. That decision does not reclassify or move
any table in this inventory: all current State/PID-mailbox live authority and
bounded cross-boundary records remain in their existing transactional stores.
See [`event-storage-topology.md`](event-storage-topology.md) for the exact
move/stay list and the rule that event presence or absence grants no authority.

AGE-371 eligibility indexes are historical candidate projections and are not
live-path scan authority. Their semantics and selected/excluded record families
are defined in `docs/architecture/record-timestamp-contract.md`.
