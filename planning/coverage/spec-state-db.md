# spec-state-db — SQLite state store, schema, migrations, deployment

## Source files

- `crates/oulipoly-core/src/lib.rs`
- `crates/oulipoly-state/build.rs`
- `crates/oulipoly-state/src/db.rs`
- `crates/oulipoly-state/src/db/invocation_records.rs`
- `crates/oulipoly-state/src/db/owned_turn_event.rs`
- `crates/oulipoly-state/src/lib.rs`
- `crates/oulipoly-state/src/lifecycle_log.rs`
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
| A caller lists direct logical invocation children. | `list_invocation_children` returns only direct children in deterministic chronological `created_at, id` order; consumer-specific projections may reorder their already-loaded copy without changing this history contract. |

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
- `crates/oulipoly-state/tests/repositories_contract.rs`
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
