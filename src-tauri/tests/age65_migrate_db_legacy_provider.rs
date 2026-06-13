//! ## Declared roles
//!
//! - orchestration
//! - formatter
//! - accessor
//! - validator
//!
//! Role set: { orchestration, formatter, accessor, validator }
//!
//! VI-005 end-to-end validation for the PP-001 inversion: `agents migrate-db`
//! resolves legacy invocation provider names through the installed models config
//! on the app side, and a missing/corrupt models config degrades non-fatally to
//! `status='legacy'` (per V10 — observable, not silent).
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/tests/age65_migrate_db_legacy_provider.rs
//!     role: intrinsic-surface
//!     Domain: migrate-db-legacy-provider-integration-harness
//!     Owns:
//!       - the temp XDG_CONFIG_HOME/XDG_DATA_HOME layout, installed model TOML
//!         fixtures, and legacy pre-UUID state.db this test plants and inspects
//!       - process/filesystem/SQLite fixture carriers used only by this harness:
//!         std::process::Command/Output, std::fs, tempfile::TempDir,
//!         rusqlite::Connection, and oulipoly_state::StateDb fixture open
//! ```
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use oulipoly_state::StateDb;
use rusqlite::{Connection, params};
use tempfile::TempDir;

struct MigrateDbFixture {
    _dir: TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    db_path: PathBuf,
}

fn migrate_db_fixture() -> MigrateDbFixture {
    let fixture = migrate_db_fixture_layout();
    create_fixture_dirs(&fixture);
    fixture
}

fn migrate_db_fixture_layout() -> MigrateDbFixture {
    let dir = tempfile::tempdir().unwrap();
    let config_home = dir.path().join("config");
    let data_home = dir.path().join("data");
    let models_dir = config_home.join("oulipoly-agent-runner").join("models");
    let db_path = data_home.join("oulipoly-agent-runner").join("state.db");
    MigrateDbFixture {
        _dir: dir,
        config_home,
        data_home,
        models_dir,
        db_path,
    }
}

fn create_fixture_dirs(fixture: &MigrateDbFixture) {
    std::fs::create_dir_all(&fixture.models_dir).unwrap();
    std::fs::create_dir_all(fixture.db_path.parent().unwrap()).unwrap();
}

fn write_model_toml(models_dir: &Path, model_name: &str, provider_name: &str) {
    std::fs::write(
        models_dir.join(format!("{model_name}.toml")),
        model_toml(provider_name),
    )
    .unwrap();
}

fn model_toml(provider_name: &str) -> String {
    format!("[[providers]]\nname = \"{provider_name}\"\nargs = []\n")
}

fn write_corrupt_model_toml(models_dir: &Path) {
    std::fs::write(
        models_dir.join("broken.toml"),
        "this = is = not = valid = toml",
    )
    .unwrap();
}

fn seed_legacy_invocations_db(db_path: &Path, model_name: &str) {
    // Materialize a full current-schema DB so every non-invocations table exists,
    // then replace only the invocations table with the legacy pre-UUID shape so
    // migrate-db's schema repair exercises the legacy invocation-row migration.
    StateDb::open(db_path).unwrap();
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        "DROP TABLE invocations;
            CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                success INTEGER NOT NULL,
                exit_code INTEGER NOT NULL,
                error_category TEXT,
                created_at TEXT NOT NULL
            );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO invocations
            (model_name, provider_index, success, exit_code, error_category, created_at)
         VALUES (?1, 0, 1, 0, NULL, '2026-04-17T08:00:00Z')",
        params![model_name],
    )
    .unwrap();
}

fn run_migrate_db(fixture: &MigrateDbFixture) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    cmd.arg("migrate-db");
    cmd.env("XDG_CONFIG_HOME", &fixture.config_home);
    cmd.env("XDG_DATA_HOME", &fixture.data_home);
    cmd.env("HOME", &fixture.data_home);
    cmd.env_remove("OULIPOLY_DATA_DIR");
    cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
    cmd.output().unwrap()
}

fn migrated_invocation_row(db_path: &Path) -> (String, Option<String>, String) {
    Connection::open(db_path)
        .unwrap()
        .query_row(
            "SELECT model_name, provider_name, status FROM invocations",
            [],
            migrated_invocation_row_tuple,
        )
        .unwrap()
}

fn migrated_invocation_row_tuple(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, Option<String>, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn stderr_mentions_models_config_failure(stderr: &str) -> bool {
    stderr.contains("failed to load models config")
}

#[test]
fn migrate_db_maps_legacy_provider_name_through_installed_models_config() {
    let fixture = migrate_db_fixture();
    write_model_toml(&fixture.models_dir, "legacy-model", "legacy-provider");
    seed_legacy_invocations_db(&fixture.db_path, "legacy-model");

    let output = run_migrate_db(&fixture);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let (model, provider, status) = migrated_invocation_row(&fixture.db_path);
    assert_eq!(model, "legacy-model");
    assert_eq!(
        provider.as_deref(),
        Some("legacy-provider"),
        "migrate-db must resolve the legacy provider_name through the installed model TOML"
    );
    assert_eq!(status, "succeeded");
}

#[test]
fn migrate_db_with_corrupt_models_config_degrades_legacy_non_fatally() {
    let fixture = migrate_db_fixture();
    write_corrupt_model_toml(&fixture.models_dir);
    seed_legacy_invocations_db(&fixture.db_path, "legacy-model");

    let output = run_migrate_db(&fixture);

    assert_eq!(
        output.status.code(),
        Some(0),
        "corrupt models config must not abort migrate-db: {output:?}"
    );
    let stderr = stderr_text(&output);
    assert!(
        stderr_mentions_models_config_failure(&stderr),
        "corrupt config must emit an observable degradation warning: {stderr}"
    );
    let (model, provider, status) = migrated_invocation_row(&fixture.db_path);
    assert_eq!(model, "legacy-model");
    assert_eq!(
        provider, None,
        "corrupt config must leave provider_name NULL"
    );
    assert_eq!(status, "legacy", "corrupt config must mark the row legacy");
}
