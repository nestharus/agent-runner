mod fixtures;

use fixtures::schema4_invocations::build_schema4_invocation_fixture;
use fixtures::v3_full_state_db::{
    assert_representative_state_rows_preserved, build_current_full_state_db,
    build_v3_full_state_db, fixture_schema_version,
};
use fixtures::v3_setup_only_db::build_versionless_setup_only_db;
use fixtures::versionless_unrecognized::build_versionless_unrecognized_db;
use fixtures::{schema_fingerprint, table_names, user_version};
use oulipoly_state::migrations;
use oulipoly_state::schema::{
    self, CURRENT_SCHEMA_VERSION, MINIMUM_SUPPORTED_SCHEMA_VERSION, SchemaCompatibility,
};
use oulipoly_state::{StateDb, schema_probe};
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

#[test]
fn ti_01_fresh_state_db_open_sets_current_user_version_and_required_tables() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");

    let db = StateDb::open(&db_path).unwrap();
    let conn = db.connection();

    assert_eq!(user_version(conn), CURRENT_SCHEMA_VERSION);
    for table in [
        "invocations",
        "providers",
        "provider_quotas",
        "provider_quota_windows",
        "memory_nodes",
        "memory_edges",
        "setup_sessions",
        "setup_turns",
        "session_turns",
        "session_chains",
        "session_chain_segments",
    ] {
        assert!(
            table_names(conn).contains(&table.to_string()),
            "fresh DB is missing required table {table}"
        );
    }
}

#[test]
fn ti_02_ti_23_previous_version_db_migrates_forward_and_preserves_representative_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_v3_full_state_db(&db_path);

    let before_conn = Connection::open(&db_path).unwrap();
    let before = assert_representative_state_rows_preserved(&before_conn);
    assert_eq!(before.user_version, MINIMUM_SUPPORTED_SCHEMA_VERSION);
    drop(before_conn);

    let db = StateDb::open(&db_path).unwrap();
    let after = assert_representative_state_rows_preserved(db.connection());

    assert_eq!(after.user_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(after.invocations, before.invocations);
    assert_eq!(after.providers, before.providers);
    assert_eq!(after.provider_quotas, before.provider_quotas);
    assert_eq!(after.provider_quota_windows, before.provider_quota_windows);
    assert_eq!(after.session_turns, before.session_turns);
    assert_eq!(after.session_chains, before.session_chains);
    assert_eq!(after.session_chain_segments, before.session_chain_segments);
    assert_eq!(after.assistant_body, before.assistant_body);
    assert_eq!(after.segment_last_turn_id, before.segment_last_turn_id);
}

#[test]
fn ti_03_current_version_open_is_noop_for_rows_and_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_current_full_state_db(&db_path);

    let db = StateDb::open(&db_path).unwrap();
    let first = fixtures::representative_snapshot(db.connection());
    drop(db);

    let db = StateDb::open(&db_path).unwrap();
    let second = fixtures::representative_snapshot(db.connection());

    assert_eq!(second, first);
    assert_eq!(
        db.connection()
            .query_row(
                "SELECT COUNT(*) FROM session_chain_segments WHERE chain_id = ?1",
                [fixtures::CHAIN_ID],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        db.connection()
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE provider_name = ?1",
                [fixtures::PROVIDER_NAME],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn ti_06_probe_and_classifier_report_migratable_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_v3_full_state_db(&db_path);
    let before = std::fs::read(&db_path).unwrap();

    let conn =
        Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let compatibility = schema::classify(&conn).unwrap();
    assert!(
        matches!(
            compatibility,
            SchemaCompatibility::Migratable {
                stored: MINIMUM_SUPPORTED_SCHEMA_VERSION
            }
        ),
        "expected migratable compatibility"
    );

    let report = schema_probe::inspect_schema(&conn, db_path.clone()).unwrap();
    assert_eq!(report.user_version as i32, MINIMUM_SUPPORTED_SCHEMA_VERSION);
    assert!(
        report.migratable,
        "old supported DB must be distinguishable from incompatible old state"
    );
    assert!(!report.compatible);
    drop(conn);

    let after = std::fs::read(&db_path).unwrap();
    assert_eq!(after, before, "read-only probe/classifier mutated DB bytes");
}

#[test]
fn ti_07_ti_22_schema_constants_are_single_source_for_probe_and_fixtures() {
    assert_eq!(fixture_schema_version(), MINIMUM_SUPPORTED_SCHEMA_VERSION);

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    let db = StateDb::open(&db_path).unwrap();
    let report = schema_probe::inspect_schema(db.connection(), db_path).unwrap();

    assert_eq!(
        report.current_schema_version as i32, CURRENT_SCHEMA_VERSION,
        "schema probe must import oulipoly_state::schema::CURRENT_SCHEMA_VERSION"
    );
    assert_eq!(
        report.minimum_supported_schema_version as i32, MINIMUM_SUPPORTED_SCHEMA_VERSION,
        "schema probe must import oulipoly_state::schema::MINIMUM_SUPPORTED_SCHEMA_VERSION"
    );
}

#[test]
fn ti_08_manifest_contains_embedded_ordered_migration_assets() {
    let manifest = migrations::manifest();

    assert!(!manifest.is_empty(), "migration manifest must not be empty");
    for migration in manifest {
        assert!(migration.target_version > 0);
        assert!(!migration.id.trim().is_empty());
        assert!(!migration.sql.trim().is_empty());
        assert!(
            !migration.sql.contains("crates/oulipoly-state/migrations"),
            "manifest SQL must be embedded content, not a runtime path"
        );
    }
}

#[test]
fn ti_09_ti_36_manifest_versions_are_monotonic_and_match_current_schema() {
    let mut previous = None;
    for migration in migrations::manifest() {
        if let Some(previous) = previous {
            assert!(
                migration.target_version > previous,
                "migration versions must be strictly increasing"
            );
        }
        previous = Some(migration.target_version);
    }

    assert_eq!(
        previous,
        Some(CURRENT_SCHEMA_VERSION),
        "highest compiled migration target must equal CURRENT_SCHEMA_VERSION"
    );
}

#[test]
fn ti_10_migration_plan_applies_only_missing_forward_steps() {
    let plan = migrations::plan(MINIMUM_SUPPORTED_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION).unwrap();

    assert!(
        !plan.is_empty(),
        "v3 to current should require at least one step"
    );
    assert!(
        plan.iter()
            .all(|m| m.target_version > MINIMUM_SUPPORTED_SCHEMA_VERSION)
    );
    assert!(
        plan.iter()
            .all(|m| m.target_version <= CURRENT_SCHEMA_VERSION)
    );
    assert_eq!(
        plan.last().map(|m| m.target_version),
        Some(CURRENT_SCHEMA_VERSION)
    );

    let current_plan = migrations::plan(CURRENT_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION).unwrap();
    assert!(
        current_plan.is_empty(),
        "current-version DB must not replay older migration steps"
    );
}

#[test]
fn ti_10_age_54_schema4_plan_contains_only_schema5_step() {
    let plan = migrations::plan(4, CURRENT_SCHEMA_VERSION).unwrap();

    assert_eq!(
        plan.iter()
            .map(|migration| migration.target_version)
            .collect::<Vec<_>>(),
        vec![5],
        "schema-4 DBs must take exactly the AGE-54 schema-5 migration"
    );
    assert_eq!(
        plan.iter()
            .map(|migration| migration.id)
            .collect::<Vec<_>>(),
        vec!["0005_invocation_dual_session_ids"]
    );
}

#[test]
fn ti_23_age_54_schema4_invocation_fixture_migrates_without_row_loss() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_schema4_invocation_fixture(&db_path);
    let before_conn = Connection::open(&db_path).unwrap();
    let before_count = fixtures::count_rows(&before_conn, "invocations");
    drop(before_conn);

    let db = StateDb::open(&db_path).unwrap();

    assert_eq!(user_version(db.connection()), 5);
    assert_eq!(
        fixtures::count_rows(db.connection(), "invocations"),
        before_count
    );
}

#[test]
fn ti_11_fresh_schema_and_migrated_schema_are_structurally_equivalent() {
    let dir = tempfile::tempdir().unwrap();
    let fresh_path = dir.path().join("fresh.db");
    let migrated_path = dir.path().join("migrated.db");

    let fresh = StateDb::open(&fresh_path).unwrap();
    let fresh_schema = normalized_schema(fresh.connection());
    drop(fresh);

    build_v3_full_state_db(&migrated_path);
    let migrated = StateDb::open(&migrated_path).unwrap();
    let migrated_schema = normalized_schema(migrated.connection());

    assert_eq!(migrated_schema, fresh_schema);
}

#[test]
fn ti_30_versionless_setup_only_db_migrates_to_current_and_preserves_setup_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_versionless_setup_only_db(&db_path);

    let db = StateDb::open(&db_path).unwrap();
    let conn = db.connection();

    assert_eq!(user_version(conn), CURRENT_SCHEMA_VERSION);
    assert_eq!(fixtures::count_rows(conn, "memory_nodes"), 2);
    assert_eq!(fixtures::count_rows(conn, "memory_edges"), 1);
    assert_eq!(fixtures::count_rows(conn, "setup_sessions"), 1);
    assert_eq!(fixtures::count_rows(conn, "setup_turns"), 1);
    assert!(memory_edges_has_foreign_keys(conn));
}

#[test]
fn ti_35_migrations_are_embedded_and_independent_of_current_working_directory() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_v3_full_state_db(&db_path);

    let _guard = cwd_lock().lock().unwrap();
    let cwd = std::env::current_dir().unwrap();
    let other_cwd = tempfile::tempdir().unwrap();
    std::env::set_current_dir(other_cwd.path()).unwrap();
    let result = StateDb::open(&db_path);
    std::env::set_current_dir(cwd).unwrap();

    let db = result.unwrap();
    assert_eq!(user_version(db.connection()), CURRENT_SCHEMA_VERSION);
}

#[test]
fn ti_37_state_migrations_stay_rusqlite_embedded_without_runtime_migration_dependencies() {
    let cargo_toml = include_str!("../Cargo.toml");
    for forbidden in ["sqlx", "refinery", "rusqlite_migration"] {
        assert!(
            !cargo_toml.contains(forbidden),
            "AGE-32 contract chose embedded rusqlite migrations; document and review any {forbidden} dependency"
        );
    }
}

#[test]
fn ti_38_versionless_unrecognized_shape_fails_closed_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_versionless_unrecognized_db(&db_path);
    let before_conn = Connection::open(&db_path).unwrap();
    let before_fingerprint = schema_fingerprint(&before_conn);
    drop(before_conn);

    let err = match StateDb::open(&db_path) {
        Ok(_) => panic!("unrecognized versionless DB unexpectedly opened"),
        Err(err) => err,
    };
    let message = err.to_string();

    assert!(message.contains("agents migrate --rebuild"), "{message}");
    assert!(
        message.contains(db_path.to_string_lossy().as_ref()),
        "{message}"
    );
    let after_conn = Connection::open(&db_path).unwrap();
    assert_eq!(schema_fingerprint(&after_conn), before_fingerprint);
    assert_eq!(user_version(&after_conn), 0);

    let compatibility = schema::classify(&after_conn).unwrap();
    assert!(
        matches!(compatibility, SchemaCompatibility::UnrecognizedVersionless),
        "expected unrecognized versionless compatibility"
    );
}

#[test]
fn ti_40_legacy_repair_helpers_are_allow_listed_and_migration_represented() {
    let db_source = include_str!("../src/db.rs");
    let helper_names = find_ensure_schema_helpers(db_source);
    let allowed: BTreeSet<&str> = [
        "ensure_invocations_schema",
        "ensure_providers_schema",
        "ensure_session_turns_schema",
        "ensure_provider_quotas_schema",
        "ensure_provider_quotas_topology_schema",
        "ensure_provider_quota_windows_schema",
    ]
    .into_iter()
    .collect();

    for helper in &helper_names {
        assert!(
            allowed.contains(helper.as_str()),
            "new ad hoc schema repair helper {helper} must be represented as an ordered migration"
        );
    }

    let mut migration_sql = String::new();
    for migration in migrations::manifest() {
        migration_sql.push_str(&normalize_sql(migration.sql));
        migration_sql.push('\n');
    }

    for helper in [
        "ensure_invocations_schema",
        "ensure_providers_schema",
        "validate_providers_schema",
        "ensure_session_turns_schema",
        "ensure_provider_quotas_schema",
        "ensure_provider_quotas_topology_schema",
        "ensure_provider_quota_windows_schema",
        "backfill_session_chains",
    ] {
        let body = extract_function_body(db_source, helper);
        for statement in mutating_sql_statements(&body) {
            let normalized = normalize_sql(&statement);
            assert!(
                migration_sql.contains(&normalized),
                "{helper} contains schema-mutating SQL not represented in compiled migrations: {statement}"
            );
        }
    }
}

fn normalized_schema(conn: &Connection) -> BTreeMap<String, String> {
    let mut stmt = conn
        .prepare(
            "SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
             ORDER BY type, name",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((
            format!(
                "{}:{}:{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?
            ),
            normalize_sql(&row.get::<_, String>(3)?),
        ))
    })
    .unwrap()
    .collect::<Result<BTreeMap<_, _>, _>>()
    .unwrap()
}

fn memory_edges_has_foreign_keys(conn: &Connection) -> bool {
    let mut stmt = conn
        .prepare("PRAGMA foreign_key_list(memory_edges)")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(2)?, row.get::<_, String>(3)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows.contains(&("memory_nodes".to_string(), "source_id".to_string()))
        && rows.contains(&("memory_nodes".to_string(), "target_id".to_string()))
}

#[allow(clippy::collapsible_if)]
fn find_ensure_schema_helpers(source: &str) -> Vec<String> {
    let mut helpers = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("fn ensure_") {
            if let Some((name_tail, _)) = rest.split_once('(') {
                let name = format!("ensure_{name_tail}");
                if name.ends_with("_schema") {
                    helpers.push(name);
                }
            }
        }
    }
    helpers
}

fn extract_function_body(source: &str, function_name: &str) -> String {
    let start = source
        .find(&format!("fn {function_name}"))
        .unwrap_or_else(|| panic!("missing function {function_name}"));
    let brace_start = source[start..].find('{').unwrap() + start;
    let mut depth = 0usize;
    let mut end = brace_start;
    for (offset, ch) in source[brace_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = brace_start + offset + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    source[brace_start..end].to_string()
}

fn mutating_sql_statements(rust_body: &str) -> Vec<String> {
    extract_rust_string_literals(rust_body)
        .into_iter()
        .flat_map(|literal| {
            strip_sql_comments(&literal)
                .split(';')
                .map(str::trim)
                .filter(|statement| is_schema_mutation(statement))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn extract_rust_string_literals(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            let mut literal = String::new();
            index += 1;
            let mut escaped = false;
            while index < bytes.len() {
                let ch = bytes[index] as char;
                if escaped {
                    literal.push(ch);
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    break;
                } else {
                    literal.push(ch);
                }
                index += 1;
            }
            literals.push(literal);
        }
        index += 1;
    }
    literals
}

fn strip_sql_comments(sql: &str) -> String {
    sql.lines()
        .map(|line| line.split_once("--").map_or(line, |(prefix, _)| prefix))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_schema_mutation(statement: &str) -> bool {
    let normalized = normalize_sql(statement);
    [
        "alter table",
        "create table",
        "create index",
        "drop table",
        "drop column",
    ]
    .iter()
    .any(|prefix| normalized.starts_with(prefix))
        || (normalized.contains("alter table") && normalized.contains(" rename "))
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn cwd_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
