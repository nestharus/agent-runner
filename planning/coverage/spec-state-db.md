# spec-state-db — SQLite state store, schema, migrations, deployment

## Source files

- `crates/oulipoly-core/src/lib.rs`
- `crates/oulipoly-state/build.rs`
- `crates/oulipoly-state/src/db.rs`
- `crates/oulipoly-state/src/db/session_lifecycle.rs`
- `crates/oulipoly-state/src/db/invocation_records.rs`
- `crates/oulipoly-state/src/db/invocation_schema_indexes.rs`
- `crates/oulipoly-state/src/db/invocation_schema_legacy_migration.rs`
- `crates/oulipoly-state/src/db/invocation_schema_repair.rs`
- `crates/oulipoly-state/src/db/completed_turns.rs`
- `crates/oulipoly-state/src/db/record_timestamps.rs`
- `crates/oulipoly-state/src/db/retention.rs`
- `crates/oulipoly-state/src/db/invocation_timestamp_contract.rs`
- `crates/oulipoly-state/src/db/opening_migrations.rs`
- `crates/oulipoly-state/src/db/opening_write.rs`
- `crates/oulipoly-state/src/db/resume_lookup.rs`
- `crates/oulipoly-state/src/db/resume_resolution.rs`
- `crates/oulipoly-state/src/db/resume_types.rs`
- `crates/oulipoly-state/src/db/owned_turn_event.rs`
- `crates/oulipoly-state/src/lib.rs`
- `crates/oulipoly-state/src/retention.rs`
- `crates/oulipoly-state/src/detached_maintenance.rs`
- `crates/oulipoly-state/src/maintenance.rs`
- `crates/oulipoly-state/src/live_history.rs`
- `crates/oulipoly-state/src/lifecycle_log.rs`
- `crates/oulipoly-state/src/mailbox.rs`
- `crates/oulipoly-state/src/mailbox/retention.rs`
- `crates/oulipoly-state/src/mailbox/schema.rs`
- `crates/oulipoly-state/src/mailbox/migrations/0022_live_history_barrier.sql`
- `crates/oulipoly-state/src/mailbox/migrations/0023_record_timestamp_contract.sql`
- `crates/oulipoly-state/src/mailbox/migrations/0024_completion_mailbox_provenance.sql`
- `crates/oulipoly-state/src/mailbox/migrations/0024_completion_mailbox_provenance_trigger.sql`
- `crates/oulipoly-state/migrations/0012_session_ingress_evidence.sql`
- `crates/oulipoly-state/migrations/0026_live_history_barrier.sql`
- `crates/oulipoly-state/migrations/0027_record_timestamp_contract.sql`
- `crates/oulipoly-state/src/migrations.rs`
- `crates/oulipoly-state/src/repositories/mod.rs`
- `crates/oulipoly-state/src/schema.rs`
- `crates/oulipoly-state/src/schema_probe.rs`
- `crates/oulipoly-state/src/deployment/mod.rs`
- `crates/oulipoly-state/src/deployment/metadata/mod.rs`
- `crates/oulipoly-state/src/deployment/metadata/schema.rs`
- `crates/oulipoly-state/src/deployment/metadata/store/api.rs`
- `crates/oulipoly-state/src/deployment/metadata/store/error.rs`
- `crates/oulipoly-state/src/deployment/metadata/store/filters.rs`
- `crates/oulipoly-state/src/deployment/metadata/store/formatters.rs`
- `crates/oulipoly-state/src/deployment/metadata/store/mod.rs`
- `crates/oulipoly-state/src/deployment/metadata/store/parsers.rs`
- `crates/oulipoly-state/src/deployment/metadata/store/queries.rs`
- `crates/oulipoly-state/src/deployment/metadata/store/rows.rs`
- `crates/oulipoly-state/src/deployment/paths/mod.rs`
- `crates/oulipoly-state/src/deployment/paths/resolver.rs`
- `crates/oulipoly-state/src/deployment/paths/resolver_validators.rs`
- `crates/oulipoly-state/src/deployment/paths/store_backed_routing.rs`
- `crates/oulipoly-state/src/deployment/paths/trigger_cases.rs`
- `crates/oulipoly-state/src/deployment/paths/trigger_decisions.rs`
- `crates/oulipoly-state/src/deployment/paths/triggers.rs`
- `crates/oulipoly-state/src/deployment/paths/types.rs`
- `crates/oulipoly-state/src/deployment/row_version/mod.rs`
- `crates/oulipoly-state/src/deployment/row_version/migrate_v6.rs`
- `crates/oulipoly-state/src/deployment/row_version/registry.rs`
- `crates/oulipoly-state/src/deployment/row_version/checksum/extract.rs`
- `crates/oulipoly-state/src/deployment/row_version/checksum/hash.rs`
- `crates/oulipoly-state/src/deployment/row_version/checksum/mod.rs`
- `crates/oulipoly-state/src/deployment/row_version/compare/decide.rs`
- `crates/oulipoly-state/src/deployment/row_version/compare/mod.rs`
- `crates/oulipoly-state/src/deployment/row_version/compare/predicate.rs`
- `crates/oulipoly-state/src/deployment/row_version/triggers_sql/apply.rs`
- `crates/oulipoly-state/src/deployment/row_version/triggers_sql/generate.rs`
- `crates/oulipoly-state/src/deployment/row_version/triggers_sql/mod.rs`
- `crates/oulipoly-state/src/deployment/routing.rs`
- `src-tauri/src/invocation/stale_reconcile.rs`

## Preconditions

- A target SQLite database path (per-deployment per `AGENTS.md` § State
  DB Schema Migrations) — typically resolved by `deployment/paths/`.
- A schema version expected by the caller (binary semver baked at build
  time by `build.rs`).
- For read paths: an open connection. For write paths: a connection plus
  the row-version triggers in place.

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| Fresh deployment, no DB file. | `db.rs` opens (creating), `migrations.rs` applies the full schema in one transaction, `schema_probe.rs` reports the resulting version. |
| Existing DB at current version. | Open succeeds without migration writes; `schema_probe.rs` confirms version match. |
| Existing DB one or more versions behind. | `migrations.rs` runs forward migrations in order; row-version triggers apply per `row_version/triggers_sql/`. |
| Existing DB at a FUTURE version. | Open fails with `SchemaTooNew` carrying actual and expected versions; do NOT downgrade. |
| Existing DB at a known-incompatible past version (no migration path). | Open fails with `MigrationUnsupported`; advise the operator to reset or restore. |
| Concurrent reader during writer migration. | SQLite WAL + retry handles short waits; long contention surfaces as `DbBusy`. |
| Repository operation on a row whose `row_version` has advanced. | `repositories/mod.rs` returns a typed conflict error; caller decides retry/replace. |
| A resident owner accepts one immutable mailbox row. | One transaction verifies the exact supervisor lease, persists ingress, records `accepted_pending`, and advances only that session's cursor. |
| PTY transport or manual acknowledgement evidence arrives. | Store its explicit evidence kind under the exact delivery/session/generation fence without advancing provider `submitted` or `confirmed`. |
| A caller reports that one materialized completion mailbox row was consumed in-band. | When the durable completion owner matches the exact listener owner, acknowledge only that mailbox row and listener once as `consumed_in_call`, resolve its delivery attempt, and leave sibling event listeners and unrelated pending rows active. |
| A v2 completion listener is materialized, or a v23 sidecar is upgraded with pending completion rows. | Materialization stamps durable v2 provenance in the mailbox transaction. Upgrade backfills relational v2 evidence and verifies retained payloads for remaining pending rows. Ambiguous rows retain wake debt and reject generic activation; a later exact-claim retry can classify restored evidence. A verified legacy payload may enter the existing generic lane. |
| A caller lists direct logical invocation children. | `list_invocation_children` returns only direct children in deterministic chronological `created_at, id` order; consumer-specific projections may reorder their already-loaded copy without changing this history contract. |
| A selected retained lifecycle reaches terminal state. | Its terminal/closed time and explicit eligibility projection are written atomically with the state transition. Backward wall time is preserved as `clock_anomaly` and is not eligible. |
| A legacy row lacks strong timestamp evidence. | Migration preserves known bytes, records `legacy_unknown`, and leaves `retention_eligible_at` null rather than inventing migration/epoch/file time. |
| A current-stamped invocation schema is initialized, drift-repaired, or rebuilt from the sanctioned pre-UUID shape. | The shared idempotent timestamp installer supplies safe projections, retention index, immutable creation/terminal guards, terminal-reopen rejection, and append-only audited repair authorization before the route completes. |
| A classifier-accepted versionless full runner has the exact seven-column pre-UUID invocation table. | One transaction establishes the shape-independent v5 baseline and version marker; migrations continue through v26, rebuild rows with stable generated UUIDs and unknown terminal age, then install v27. Reopen preserves the result exactly. |
| A triggered completion listener is reactivated from retired to pending. | The same transition clears listener eligibility, blocks/clears the parent event, preserves first close, and keeps unknown/anomaly evidence sticky. Later retirement uses its new occurrence and updates the parent from the exact child plus the indexed pending projection. |
| An operator supplies an evidenced terminal-time correction. | Only an explicit historical State handle may append the immutable audit row and repair terminal/eligibility fields in one transaction; ordinary/live writers are rejected. |
| A detached caller requests one retention slice. | Before opening or mutating the authority database, the job durably checkpoints one fresh fixed `as_of` and empty cursor. The complete per-family policy fixes that inclusive cutoff; indexed selection returns at most `limit + 1`, each selected authority root is revalidated in a short zero-wait transaction, and the outcome returns counts, typed preservation reasons, gaps, and an exact resumable cursor. Resume after a post-batch/pre-checkpoint crash reuses the original `as_of`. |
| Detached retention or payload compaction opens State or mailbox history. | The historical handle disables connection-open process observation, including compaction, while ordinary opens retain observation. Maintenance progress is recorded only in its job/evidence records. |
| A terminal row is active, unresolved, unacknowledged, inherited, recovery-authoritative, legacy-unknown, clock-anomalous, or younger than the horizon. | Retention preserves it with the corresponding reason; age or count pressure cannot override missing authority evidence. |
| A writer changes authority or creates a related record after retention candidate selection. | Exact status/timestamp/dependency predicates preserve the stale candidate; the new record is never captured by the old snapshot. |
| Independent retention observation cannot be recorded. | The already-completed bounded operation is not rolled back and returns an explicit observation-delivery gap. |

## Edge cases

- DB file exists but is empty / zero bytes — treat as fresh; recreate
  schema.
- DB file is locked by another process (lock not WAL-aware) —
  `db.rs` waits up to the configured busy_timeout, then returns `DbBusy`.
- Migration partially succeeds then fails — the migration transaction
  rolls back atomically; schema_probe still reports the pre-migration
  version.
- Row-version checksum mismatch on read — `row_version/checksum/`
  modules surface a typed mismatch; caller decides repair path.
- Path resolution finds a multi-deployment ambiguity — `paths/triggers`
  emits a structured ambiguity error rather than guessing.
- Repeating an exact-row consumed acknowledgement is idempotent; it does not
  increase `delivery_attempts` or acknowledge another listener for the event.
- Only the exact listener matching an event's durable completion owner may
  claim its late-consumption marker; repeat and sibling-listener claims leave
  their mailbox rows unchanged.
- A sibling row presented with another listener's durable owner identity remains
  pending even when both rows reference the same completion marker.
- Equal terminal and creation time is valid for newly authoritative writes, but
  historical equality known to originate from the legacy rebuild is not treated
  as closure evidence. Established terminal time and terminal state cannot be
  rewritten/reopened by ordinary replay.
- Existing mailbox prune/reclaim paths require explicit eligibility. A terminal
  phase with unknown age, unresolved custody, or a clock anomaly is retained.
- The default horizon is 30 days with an inclusive boundary: equal and older
  authoritative times may proceed, while a value one microsecond younger is
  preserved. Resume cursors cannot cross policy/family/cutoff snapshots.
- Busy live writers return a typed nonblocking outcome without advancing past
  the blocked candidate. Restarting the same slice is idempotent.
- Unrecognized nonempty versionless pre-UUID invocation shapes are rejected
  before journal-mode, schema, or version mutation; no partially normalized
  identity is left for the next open.

## Error conditions

- `DbOpenFailed` — file IO, permission, or corruption.
- `DbBusy` — busy_timeout exceeded.
- `SchemaTooNew` — DB ahead of binary.
- `MigrationUnsupported` — past version with no forward path.
- `MigrationFailed` — forward migration threw; transaction rolled back.
- `RowVersionMismatch` — repository update saw a row whose version
  changed since read.
- `DeploymentRoutingAmbiguous` — multi-deployment selection has more
  than one candidate.

## Boundaries

- State DB does NOT decide which session/provider/account to write —
  callers supply identities.
- State DB does NOT classify terminal signals — that is the recognizer.
- State DB does NOT execute provider processes — that is the executor.
- State DB does NOT apply observability visibility or live-candidate
  prioritization policy; it preserves chronological invocation history.
- `oulipoly-core` re-exports a thin type surface used by state +
  runtime; it has no behavior to spec independently. Source-anchored
  here so PRs touching core resolve to this spec rather than NO_SPEC.

## Declared test patterns

Per `~/ai/conventions/testing.md`: schema-probe round-trip, migration
forward/upgrade tests, row-version conflict tests, deployment routing
table tests, repositories contract.

- `crates/oulipoly-state/tests/age122_lifecycle_schema_round2.rs`
- `crates/oulipoly-state/tests/age122_sqlite_schema_round2.rs`
- `crates/oulipoly-state/tests/age_123_resume_provider_identity.rs`
- `crates/oulipoly-state/tests/age_149_migration_error_characterization.rs`
- `crates/oulipoly-state/tests/age_149_schema_classifier_characterization.rs`
- `crates/oulipoly-state/tests/age_32_connection_boundary.rs`
- `crates/oulipoly-state/tests/age_32_migration_boundary.rs`
- `crates/oulipoly-state/tests/age_54_row_preservation.rs`
- `crates/oulipoly-state/tests/age_61_row_version_compare.rs`
- `crates/oulipoly-state/tests/age_61_row_version_migration_idempotent.rs`
- `crates/oulipoly-state/tests/age_61_row_version_migration_pragma.rs`
- `crates/oulipoly-state/tests/age_61_row_version_triggers_old_writer.rs`
- `crates/oulipoly-state/tests/age_62_metadata_store.rs`
- `crates/oulipoly-state/tests/age_62_opener_contract.rs`
- `crates/oulipoly-state/tests/age_62_readonly_schema_probe.rs`
- `crates/oulipoly-state/tests/age_62_resolver_routing.rs`
- `crates/oulipoly-state/tests/age371_record_timestamps.rs`
- `crates/oulipoly-state/src/retention.rs`
  (policy boundary, fail-closed record/generation facts, cursor and observation contracts)
- `crates/oulipoly-state/src/db/retention.rs`
  (bounded restart, stale snapshots, exact boundary, writer contention)
- `crates/oulipoly-state/src/detached_maintenance.rs`
  (pre-mutation retention snapshot, post-batch crash resume,
  observation-free historical opens, phase/outcome handling)
- `crates/oulipoly-state/src/mailbox/retention.rs`
  (authority preservation, payload fencing, bounded restart, writer contention)
- `crates/oulipoly-state/tests/repositories_contract.rs`
- `crates/oulipoly-state/src/db/tests/resume_resolution_tests_1.rs`
  (exact-chain precedence, provider-scoped native candidate preservation,
  one-lineage deduplication, and multi-lineage ambiguity)
- `crates/oulipoly-state/src/mailbox.rs`
  (`late_consumed_completion_acknowledges_materialized_mailbox_row_once`)
- `src-tauri/tests/age_32_state_db_migrations.rs`
- `src-tauri/tests/age149_owned_turn_event_schema.rs`
- `src-tauri/tests/empty_bodies_ref_rca/rc1_schema_contract.rs`
- `src-tauri/tests/empty_bodies_ref_rca/rc3_export_db_source.rs`

## Cross-references

- `planning/coverage/spec-session-lifecycle.md` — writes session
  metadata through this surface.
- `planning/coverage/spec-diagnostics.md` — diagnostics sink.
- `planning/coverage/spec-config.md` — deployment routing reads from
  config.
- `AGENTS.md` § "State DB Schema Migrations".
