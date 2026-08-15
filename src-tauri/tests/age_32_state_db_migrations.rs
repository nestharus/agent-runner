#![cfg(unix)]

#[path = "../../crates/oulipoly-state/tests/fixtures/mod.rs"]
mod state_fixtures;

use oulipoly_setup::memory::MemoryGraph;
use oulipoly_state::mailbox::{CompletionEventRegistrationInput, MailboxDb};
use oulipoly_state::schema::CURRENT_SCHEMA_VERSION;
use oulipoly_state::{InvocationStart, StateDb, schema_probe};
use rusqlite::Connection;
use serde_json::Value;
use state_fixtures::future_db::build_future_db;
use state_fixtures::schema4_invocations::{SCHEMA4_ROOT_UUID, build_schema4_invocation_fixture};
use state_fixtures::v3_full_state_db::build_v3_full_state_db;
use state_fixtures::versionless_drifted_setup::build_versionless_drifted_setup_db;
use state_fixtures::versionless_unrecognized::build_versionless_unrecognized_db;
use state_fixtures::{ROOT_INVOCATION_UUID, default_state_path, user_version};
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const HOT_JOURNAL_HELPER_ENV: &str = "AGE_32_HOT_JOURNAL_HELPER";
const HOT_JOURNAL_PATH_ENV: &str = "AGE_32_HOT_JOURNAL_PATH";

struct CliFixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

impl CliFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        Self {
            _dir: dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        default_state_path(&self.data_home)
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn write_provider_that_marks_execution(&self) -> PathBuf {
        let marker = self._dir.path().join("provider-ran");
        let script = self._dir.path().join("fixture-provider.sh");
        fs::write(
            &script,
            format!(
                "#!/usr/bin/env bash\nprintf ran > {}\nprintf 'provider response\\n'\n",
                shell_quote(&marker)
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        fs::create_dir_all(&self.models_dir).unwrap();
        fs::write(
            self.models_dir.join("fixture-model.toml"),
            "[[providers]]\nname = \"fixture-provider\"\n",
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                "[fixture-provider]\ncommand = {:?}\nargs = []\nprompt_mode = \"arg\"\n",
                script.to_string_lossy()
            ),
        )
        .unwrap();
        marker
    }

    fn run_trace_json(&self) -> Output {
        let mut cmd = self.command();
        cmd.arg("trace").arg(ROOT_INVOCATION_UUID).arg("--json");
        cmd.output().unwrap()
    }

    fn run_direct_execution(&self) -> Output {
        let mut cmd = self.command();
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg("fixture-model")
            .arg("ping");
        cmd.output().unwrap()
    }

    fn run_rebuild(&self) -> Output {
        let mut cmd = self.command();
        cmd.arg("migrate").arg("--rebuild");
        cmd.output().unwrap()
    }

    fn run_rebuild_in(&self, data_dir: &Path) -> Output {
        let mut cmd = self.command();
        cmd.env("OULIPOLY_DATA_DIR", data_dir);
        cmd.arg("migrate").arg("--rebuild");
        cmd.output().unwrap()
    }
}

fn assert_future_schema_error(command: &str, stderr: &str, db_path: &Path) {
    assert!(
        stderr.contains("schema is incompatible"),
        "{command}: stderr must preserve schema incompatibility signal: {stderr}"
    );
    assert!(
        stderr.contains(&format!("stored={}", CURRENT_SCHEMA_VERSION + 1))
            && stderr.contains(&format!("current={CURRENT_SCHEMA_VERSION}")),
        "{command}: stderr must include stored/current schema versions: {stderr}"
    );
    assert!(
        stderr.contains("db=") && stderr.contains(db_path.to_string_lossy().as_ref()),
        "{command}: stderr must name the incompatible DB path: {stderr}"
    );
    assert!(
        stderr.contains("agents migrate --rebuild"),
        "{command}: stderr must carry rebuild guidance: {stderr}"
    );
}

#[test]
fn ti_05_ti_25_trace_json_reads_historical_rows_after_forward_migration() {
    let fixture = CliFixture::new();
    build_v3_full_state_db(&fixture.db_path());

    let output = fixture.run_trace_json();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["root"]["invocation"]["id"], ROOT_INVOCATION_UUID);
    assert_eq!(json["root"]["invocation"]["status"], "succeeded");
    assert_eq!(
        json["root"]["children"][0]["invocation"]["id"],
        state_fixtures::CHILD_INVOCATION_UUID
    );
    let conn = Connection::open(fixture.db_path()).unwrap();
    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
}

#[test]
fn ti_13_direct_execution_fails_closed_on_incompatible_state_db_without_memory_fallback() {
    let fixture = CliFixture::new();
    build_future_db(&fixture.db_path());
    let marker = fixture.write_provider_that_marks_execution();

    let output = fixture.run_direct_execution();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_future_schema_error("direct-execution", &stderr, &fixture.db_path());
    assert!(
        !marker.exists(),
        "provider executed despite incompatible persistent state DB"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "direct execution must not emit provider output on migration failure"
    );
}

#[test]
fn ti_14_representative_state_touching_cli_paths_migrate_before_use() {
    for (name, args) in [
        ("trace", vec!["trace", ROOT_INVOCATION_UUID, "--json"]),
        (
            "resume-list",
            vec!["resume", "--list", state_fixtures::SESSION_ID],
        ),
        ("migrate-db", vec!["migrate-db"]),
    ] {
        let fixture = CliFixture::new();
        build_v3_full_state_db(&fixture.db_path());

        let mut cmd = fixture.command();
        cmd.args(args);
        let output = cmd.output().unwrap();

        assert_eq!(
            output.status.code(),
            Some(0),
            "{name} should migrate supported old DB before use: {output:?}"
        );
        let conn = Connection::open(fixture.db_path()).unwrap();
        assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    }

    let fixture = CliFixture::new();
    build_v3_full_state_db(&fixture.db_path());
    fixture.write_provider_that_marks_execution();
    let output = fixture.run_direct_execution();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let conn = Connection::open(fixture.db_path()).unwrap();
    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
}

#[test]
fn ti_14_age_54_read_like_cli_paths_preserve_invocation_count() {
    for (name, args) in [
        ("trace", vec!["trace", SCHEMA4_ROOT_UUID, "--json"]),
        ("migrate-db", vec!["migrate-db"]),
    ] {
        let fixture = CliFixture::new();
        build_schema4_invocation_fixture(&fixture.db_path());
        let before = invocation_count(&fixture.db_path());

        let mut cmd = fixture.command();
        cmd.args(args);
        let output = cmd.output().unwrap();

        assert_eq!(
            output.status.code(),
            Some(0),
            "{name} should preserve populated invocations: {output:?}"
        );
        assert_eq!(invocation_count(&fixture.db_path()), before);
        let conn = Connection::open(fixture.db_path()).unwrap();
        assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    }
}

#[test]
fn ti_15_tauri_state_command_open_errors_preserve_variant_and_rebuild_guidance() {
    let fixture = CliFixture::new();
    build_future_db(&fixture.db_path());

    let err = match StateDb::open(&fixture.db_path()) {
        Ok(_) => panic!("future DB unexpectedly opened"),
        Err(err) => err,
    };
    let message = err.to_string();

    assert!(
        message.contains("Incompatible")
            || message.contains("Future")
            || message.contains("stored="),
        "error should preserve a schema compatibility variant/signal: {message}"
    );
    assert!(message.contains("agents migrate --rebuild"), "{message}");
    assert!(
        message.contains(fixture.db_path().to_string_lossy().as_ref()),
        "{message}"
    );
    let lib_commands_source = include_str!("../src/lib_commands.rs");
    let command_accessor_source = include_str!("../src/commands/accessor.rs");
    let app_state_source = include_str!("../src/app_state.rs");
    assert!(
        lib_commands_source.contains("#[path = \"commands/accessor.rs\"]")
            && lib_commands_source.contains("pub mod accessor;")
            && command_accessor_source.contains("fn open_state_db")
            && command_accessor_source.contains("state.state_db_opener.open_at(&state.db_path())"),
        "Tauri state commands must share the migration-aware StateDb opener"
    );
    assert!(
        app_state_source.contains("pub fn db_path(&self) -> PathBuf")
            && app_state_source.contains("join(\"state.db\")"),
        "AppState must keep deriving state.db beside models_dir.parent()"
    );
}

#[test]
fn ti_16_ti_29_memory_graph_open_routes_through_state_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_versionless_drifted_setup_db(&db_path);

    let graph = MemoryGraph::open(&db_path).unwrap();
    let snapshot = graph.snapshot().unwrap();
    assert_eq!(snapshot.nodes.len(), 2);
    assert_eq!(snapshot.edges.len(), 1);

    let conn = Connection::open(&db_path).unwrap();
    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    assert!(memory_edges_has_foreign_keys(&conn));
}

#[test]
fn ti_17_migrate_rebuild_command_is_accepted_by_cli() {
    let fixture = CliFixture::new();

    let output = fixture.run_rebuild();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        stdout.contains("no state.db to rebuild")
            || stdout.contains("historical state was not preserved"),
        "migrate --rebuild should dispatch to the rebuild path: {stdout}"
    );
}

#[test]
fn ti_18_ti_19_ti_21_ti_26_rebuild_backs_up_sidecars_and_creates_fresh_current_db() {
    let fixture = CliFixture::new();
    build_v3_full_state_db(&fixture.db_path());
    fs::write(journal_path(&fixture.db_path()), "journal-sidecar").unwrap();
    fs::write(wal_path(&fixture.db_path()), "wal-sidecar").unwrap();
    fs::write(shm_path(&fixture.db_path()), "shm-sidecar").unwrap();

    let output = fixture.run_rebuild();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stdout.contains("backup"), "{stdout}");
    assert!(
        stdout.contains("historical state was not preserved"),
        "rebuild must warn that live historical rows are not preserved: {stdout}"
    );

    let backup_root = fixture
        .data_home
        .join("oulipoly-agent-runner")
        .join("state-backups");
    let backups = backup_dirs(&backup_root);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert!(backups[0].join("state.db").is_file());
    assert!(backups[0].join("state.db-journal").is_file());
    assert!(backups[0].join("state.db-wal").is_file());
    assert!(backups[0].join("state.db-shm").is_file());

    let conn = Connection::open(fixture.db_path()).unwrap();
    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    let old_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        old_rows, 0,
        "destructive rebuild should create a fresh live DB"
    );
}

#[test]
fn age_299_s2_rebuild_backs_up_resets_and_readmits_state_sidecar_continuity() {
    let fixture = CliFixture::new();
    let state_path = fixture.db_path();
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let first_uuid = "a1111111-1111-4111-8111-111111111111";
    let mut state = StateDb::open(&state_path).unwrap();
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: first_uuid.to_string(),
            model_name: "age299-s2-rebuild".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .register_completion_event_with_obligation(
            "age299-s2-rebuild-first-admission",
            completion_registration(
                "age299-s2-rebuild-first-event",
                first_uuid,
                "age299-s2-rebuild-first-session",
            ),
        )
        .unwrap();
    let old_generation = MailboxDb::open(&sidecar_path)
        .unwrap()
        .sidecar_generation()
        .unwrap();
    drop(state);

    let output = fixture.run_rebuild();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let backup_root = fixture
        .data_home
        .join("oulipoly-agent-runner")
        .join("state-backups");
    let backups = backup_dirs(&backup_root);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert!(backups[0].join("state.db").is_file());
    assert!(backups[0].join("pid-identity.db").is_file());
    let backup_sidecar = Connection::open(backups[0].join("pid-identity.db")).unwrap();
    let backup_generation: String = backup_sidecar
        .query_row(
            "SELECT generation_uuid FROM mailbox_sidecar_identity WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let backup_continuity_count: i64 = backup_sidecar
        .query_row(
            "SELECT COUNT(*) FROM completion_authority_continuity",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(backup_generation, old_generation);
    assert_eq!(backup_continuity_count, 1);
    drop(backup_sidecar);
    let fresh_sidecar = MailboxDb::open(&sidecar_path).unwrap();
    let fresh_generation = fresh_sidecar.sidecar_generation().unwrap();
    assert_ne!(fresh_generation, old_generation);
    assert!(
        !fresh_sidecar
            .contains_completion_obligation(
                "age299-s2-rebuild-first-event",
                first_uuid,
                "age299-s2-rebuild-first-session",
            )
            .unwrap()
    );
    drop(fresh_sidecar);
    let second_uuid = "a2222222-2222-4222-8222-222222222222";
    let mut fresh_state = StateDb::open(&state_path).unwrap();
    assert!(
        fresh_state
            .get_invocation_by_uuid(first_uuid)
            .unwrap()
            .is_none()
    );
    fresh_state
        .start_invocation(&InvocationStart {
            invocation_uuid: second_uuid.to_string(),
            model_name: "age299-s2-rebuild".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    fresh_state
        .register_completion_event_with_obligation(
            "age299-s2-rebuild-second-admission",
            completion_registration(
                "age299-s2-rebuild-second-event",
                second_uuid,
                "age299-s2-rebuild-second-session",
            ),
        )
        .unwrap();
    assert_eq!(
        fresh_state
            .completion_obligations_for_invocation(second_uuid)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn age_299_s2_rebuild_sidecar_writer_contention_is_nondestructive_and_retryable() {
    let fixture = CliFixture::new();
    let state_path = fixture.db_path();
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let invocation_uuid = "a3333333-3333-4333-8333-333333333333";
    let mut state = StateDb::open(&state_path).unwrap();
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "age299-s2-rebuild".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .register_completion_event_with_obligation(
            "age299-s2-rebuild-contention-admission",
            completion_registration(
                "age299-s2-rebuild-contention-event",
                invocation_uuid,
                "age299-s2-rebuild-contention-session",
            ),
        )
        .unwrap();
    drop(state);
    let writer = Connection::open(&sidecar_path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();

    let blocked = fixture.run_rebuild();

    assert_ne!(blocked.status.code(), Some(0), "{blocked:?}");
    assert!(
        String::from_utf8_lossy(&blocked.stderr).contains("completion_authority_contention"),
        "{blocked:?}"
    );
    assert!(state_path.is_file());
    assert!(sidecar_path.is_file());
    assert!(
        backup_dirs(
            &fixture
                .data_home
                .join("oulipoly-agent-runner")
                .join("state-backups")
        )
        .is_empty()
    );
    writer.execute_batch("ROLLBACK").unwrap();
    drop(writer);

    let retry = fixture.run_rebuild();

    assert_eq!(retry.status.code(), Some(0), "{retry:?}");
    assert!(StateDb::open(&state_path).is_ok());
    assert!(MailboxDb::open(&sidecar_path).is_ok());
}

#[test]
fn rebuild_backs_up_and_removes_a_hot_rollback_journal_before_fresh_initialization() {
    let fixture = CliFixture::new();
    drop(StateDb::open(&fixture.db_path()).unwrap());
    leave_hot_rollback_journal(&fixture.db_path());
    let journal = journal_path(&fixture.db_path());
    let journal_before = fs::read(&journal).unwrap();
    assert!(journal_before.len() > 512, "journal was not populated");

    let output = fixture.run_rebuild();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let backup_root = fixture
        .data_home
        .join("oulipoly-agent-runner")
        .join("state-backups");
    let backups = backup_dirs(&backup_root);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(
        fs::read(backups[0].join("state.db-journal")).unwrap(),
        journal_before
    );
    assert!(!journal.exists());
    let conn = Connection::open(fixture.db_path()).unwrap();
    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    let probe_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'hot_journal_probe'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(probe_exists, 0, "hot journal restored pre-rebuild state");
}

#[test]
fn rebuild_rejects_orphaned_recovery_artifacts_instead_of_reporting_no_work() {
    for suffix in ["-journal", "-wal", "-shm"] {
        let fixture = CliFixture::new();
        fs::create_dir_all(fixture.db_path().parent().unwrap()).unwrap();
        let artifact = sidecar_path(&fixture.db_path(), suffix);
        fs::write(&artifact, b"orphaned recovery state").unwrap();

        let output = fixture.run_rebuild();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(!output.status.success(), "{output:?}");
        assert!(stderr.contains("recovery artifact remains"), "{stderr}");
        assert!(stderr.contains(suffix), "{stderr}");
        assert!(!stdout.contains("no state.db"), "{stdout}");
        assert_eq!(fs::read(&artifact).unwrap(), b"orphaned recovery state");
        assert!(
            !fixture
                .data_home
                .join("oulipoly-agent-runner")
                .join("state-backups")
                .exists()
        );
    }
}

#[test]
#[ignore = "spawned by rebuild_backs_up_and_removes_a_hot_rollback_journal_before_fresh_initialization"]
fn hot_rollback_journal_helper() {
    if std::env::var_os(HOT_JOURNAL_HELPER_ENV).is_none() {
        return;
    }
    let db_path = PathBuf::from(std::env::var_os(HOT_JOURNAL_PATH_ENV).unwrap());
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=DELETE;
         PRAGMA synchronous=FULL;
         PRAGMA cache_size=1;
         BEGIN IMMEDIATE;
         CREATE TABLE hot_journal_probe(value BLOB NOT NULL);
         INSERT INTO hot_journal_probe(value) VALUES (zeroblob(1048576));",
    )
    .unwrap();
    let journal = journal_path(&db_path);
    let bytes = fs::read(&journal).unwrap();
    assert!(bytes.len() > 512, "journal was not populated");
    std::process::exit(86);
}

fn leave_hot_rollback_journal(db_path: &Path) {
    let output = Command::new(std::env::current_exe().unwrap())
        .arg("hot_rollback_journal_helper")
        .arg("--exact")
        .arg("--ignored")
        .arg("--nocapture")
        .env(HOT_JOURNAL_HELPER_ENV, "1")
        .env(HOT_JOURNAL_PATH_ENV, db_path)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(86),
        "helper failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rebuild_rejects_a_leaf_symlink_before_backup_or_removal() {
    let fixture = CliFixture::new();
    let target_path = fixture.db_path().with_file_name("target.db");
    build_v3_full_state_db(&target_path);
    symlink(&target_path, fixture.db_path()).unwrap();
    let target_before = fs::read(&target_path).unwrap();

    let output = fixture.run_rebuild();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "{output:?}");
    assert!(stderr.contains("leaf symlink"), "{stderr}");
    assert!(
        fs::symlink_metadata(fixture.db_path())
            .unwrap()
            .file_type()
            .is_symlink(),
        "rebuild must preserve the canonical symlink on refusal"
    );
    assert_eq!(fs::read(&target_path).unwrap(), target_before);
    let backup_root = fixture
        .data_home
        .join("oulipoly-agent-runner")
        .join("state-backups");
    assert!(!backup_root.exists());
}

#[test]
fn rebuild_rejects_a_dangling_leaf_symlink_before_no_work_or_backup() {
    let fixture = CliFixture::new();
    fs::create_dir_all(fixture.db_path().parent().unwrap()).unwrap();
    symlink(
        fixture.db_path().with_file_name("missing.db"),
        fixture.db_path(),
    )
    .unwrap();

    let output = fixture.run_rebuild();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "{output:?}");
    assert!(stderr.contains("leaf symlink"), "{stderr}");
    assert!(!stdout.contains("no state.db"), "{stdout}");
    assert!(
        fs::symlink_metadata(fixture.db_path())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        !fixture
            .data_home
            .join("oulipoly-agent-runner")
            .join("state-backups")
            .exists()
    );
}

#[test]
fn rebuild_rejects_a_dangling_ancestor_symlink_before_no_work_or_backup() {
    let fixture = CliFixture::new();
    let missing_data_dir = fixture._dir.path().join("missing-data");
    let data_alias = fixture._dir.path().join("data-alias");
    symlink(&missing_data_dir, &data_alias).unwrap();

    let output = fixture.run_rebuild_in(&data_alias);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "{output:?}");
    assert!(stderr.contains("dangling ancestor symlink"), "{stderr}");
    assert!(!stdout.contains("no state.db"), "{stdout}");
    assert!(!missing_data_dir.exists());
    assert!(
        fs::symlink_metadata(&data_alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn rebuild_rejects_a_non_utf8_default_before_planning_or_reporting() {
    let fixture = CliFixture::new();
    let data_dir = fixture
        ._dir
        .path()
        .join(std::ffi::OsString::from_vec(b"data-\xff".to_vec()));

    let missing = fixture.run_rebuild_in(&data_dir);
    let missing_stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(!missing.status.success(), "{missing:?}");
    assert!(missing_stderr.contains("valid UTF-8"), "{missing_stderr}");
    assert!(!String::from_utf8_lossy(&missing.stdout).contains("no state.db"));
    assert!(!data_dir.exists());

    fs::create_dir(&data_dir).unwrap();
    let state_path = data_dir.join("state.db");
    fs::write(&state_path, b"unchanged").unwrap();
    let existing = fixture.run_rebuild_in(&data_dir);
    let existing_stderr = String::from_utf8_lossy(&existing.stderr);
    assert!(!existing.status.success(), "{existing:?}");
    assert!(existing_stderr.contains("valid UTF-8"), "{existing_stderr}");
    assert_eq!(fs::read(&state_path).unwrap(), b"unchanged");
    assert!(!data_dir.join("state-backups").exists());
    assert!(!data_dir.join("state.db.namespace.lock").exists());
}

#[test]
fn ti_20_rebuild_is_idempotent_and_keeps_prior_backup() {
    let fixture = CliFixture::new();
    build_v3_full_state_db(&fixture.db_path());

    let first = fixture.run_rebuild();
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    fs::write(fixture.db_path(), "not sqlite anymore").unwrap();
    let second = fixture.run_rebuild();
    assert_eq!(second.status.code(), Some(0), "{second:?}");

    let backup_root = fixture
        .data_home
        .join("oulipoly-agent-runner")
        .join("state-backups");
    let backups = backup_dirs(&backup_root);
    assert_eq!(backups.len(), 2, "{backups:?}");
    assert_ne!(
        backups[0], backups[1],
        "backup directories must be distinct"
    );
    assert!(backups[0].join("state.db").is_file());
    assert!(backups[1].join("state.db").is_file());
}

#[test]
fn ti_27_new_invocations_recorded_after_migration_are_visible_to_trace() {
    let fixture = CliFixture::new();
    build_v3_full_state_db(&fixture.db_path());
    fixture.write_provider_that_marks_execution();

    let direct = fixture.run_direct_execution();
    assert_eq!(direct.status.code(), Some(0), "{direct:?}");
    let trace = fixture.run_trace_json();
    assert_eq!(trace.status.code(), Some(0), "{trace:?}");
    let json: Value = serde_json::from_slice(&trace.stdout).unwrap();
    assert_eq!(json["root"]["invocation"]["id"], ROOT_INVOCATION_UUID);

    let conn = Connection::open(fixture.db_path()).unwrap();
    let total_invocations: i64 = conn
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap();
    assert!(
        total_invocations >= 3,
        "old trace rows and post-migration direct invocation should coexist"
    );
}

#[test]
fn ti_28_cli_schema_failures_surface_actionable_rebuild_errors_without_partial_output() {
    for (name, args) in [
        ("trace", vec!["trace", ROOT_INVOCATION_UUID, "--json"]),
        (
            "session-export",
            vec!["session", "export", state_fixtures::SESSION_ID],
        ),
        (
            "resume",
            vec![
                "resume",
                "--session-id",
                state_fixtures::SESSION_ID,
                "--prompt",
                "hello",
            ],
        ),
    ] {
        let fixture = CliFixture::new();
        build_future_db(&fixture.db_path());

        let mut cmd = fixture.command();
        cmd.args(args);
        let output = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_ne!(output.status.code(), Some(0), "{name}: {output:?}");
        assert_future_schema_error(name, &stderr, &fixture.db_path());
        assert!(
            output.stdout.is_empty(),
            "{name} must not emit partial success output"
        );
    }

    let fixture = CliFixture::new();
    build_future_db(&fixture.db_path());
    let marker = fixture.write_provider_that_marks_execution();
    let output = fixture.run_direct_execution();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_future_schema_error("direct-execution", &stderr, &fixture.db_path());
    assert!(!marker.exists());
    assert!(
        output.stdout.is_empty(),
        "direct execution must not emit provider output on migration failure"
    );
}

#[test]
fn ti_31_memory_schema_is_owned_by_migrations_and_memory_graph_has_no_raw_reopen_bypass() {
    let memory_source = include_str!("../../crates/oulipoly-setup/src/memory.rs");

    assert!(
        !memory_source.contains("CREATE TABLE IF NOT EXISTS memory_nodes")
            && !memory_source.contains("CREATE TABLE memory_nodes"),
        "MemoryGraph::open must not own memory table creation after AGE-32"
    );
    assert!(
        !memory_source.contains("Connection::open(path)"),
        "MemoryGraph::open must not raw-open the state DB after the migration gate"
    );
    assert!(
        memory_source.contains("StateDb::open_for_memory"),
        "MemoryGraph::open must route through StateDb::open_for_memory"
    );

    let manifest_sql = oulipoly_state::migrations::manifest()
        .iter()
        .map(|migration| migration.sql)
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    assert!(manifest_sql.contains("create table") && manifest_sql.contains("memory_edges"));
    assert!(manifest_sql.contains("references memory_nodes(id)"));
}

#[test]
fn ti_32_setup_memory_errors_are_actionable_and_do_not_write_setup_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_versionless_unrecognized_db(&db_path);
    let before = state_fixtures::schema_fingerprint(&Connection::open(&db_path).unwrap());

    let err = match MemoryGraph::open(&db_path) {
        Ok(_) => panic!("unrecognized setup DB unexpectedly opened"),
        Err(err) => err,
    };

    assert!(
        err.contains("UnrecognizedShape") || err.contains("unrecognized"),
        "setup memory error should preserve the schema error variant/signal: {err}"
    );
    assert!(err.contains("agents migrate --rebuild"), "{err}");
    let conn = Connection::open(&db_path).unwrap();
    assert_eq!(state_fixtures::schema_fingerprint(&conn), before);
    assert!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'setup_turns'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            == 0,
        "setup rows/table must not be created after migration failure"
    );
}

#[test]
fn ti_33_ti_34_docs_describe_schema_migrations_and_rebuild_recovery() {
    let agents = include_str!("../../AGENTS.md");
    let readme = include_str!("../../README.md");
    for (name, text) in [("AGENTS.md", agents), ("README.md", readme)] {
        for needle in [
            "State DB Schema Migrations",
            "crates/oulipoly-state/migrations/",
            "crates/oulipoly-state/src/schema.rs",
            "CURRENT_SCHEMA_VERSION",
            "agents migrate --rebuild",
            "agents migrate-db",
            "session schema-probe",
        ] {
            assert!(
                text.contains(needle),
                "{name} missing migration doc needle {needle}"
            );
        }
    }
}

#[test]
fn ti_38_cli_and_read_only_probe_fail_closed_for_unrecognized_versionless_default_db() {
    let fixture = CliFixture::new();
    build_versionless_unrecognized_db(&fixture.db_path());
    let before = fs::read(fixture.db_path()).unwrap();

    let mut cmd = fixture.command();
    cmd.arg("trace").arg(ROOT_INVOCATION_UUID);
    let output = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr.contains("agents migrate --rebuild"), "{stderr}");
    assert_eq!(fs::read(fixture.db_path()).unwrap(), before);

    let conn = Connection::open_with_flags(
        fixture.db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let report = schema_probe::inspect_schema(&conn, fixture.db_path()).unwrap();
    assert!(!report.compatible);
    assert_eq!(report.user_version, 0);
    assert_eq!(fs::read(fixture.db_path()).unwrap(), before);
}

fn completion_registration<'a>(
    event_id: &'a str,
    owner_invocation_uuid: &'a str,
    owner_session_id: &'a str,
) -> CompletionEventRegistrationInput<'a> {
    CompletionEventRegistrationInput {
        event_id,
        delivery_mode: "async",
        owner_session_id: Some(owner_session_id),
        owner_invocation_uuid: Some(owner_invocation_uuid),
        state_dir: "/tmp/age299-s2-rebuild-state",
        meta_path: "/tmp/age299-s2-rebuild-meta",
        log_path: "/tmp/age299-s2-rebuild-log",
        rc_path: "/tmp/age299-s2-rebuild-rc",
    }
}

fn backup_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    dirs.sort();
    dirs
}

fn wal_path(path: &Path) -> PathBuf {
    sidecar_path(path, "-wal")
}

fn shm_path(path: &Path) -> PathBuf {
    sidecar_path(path, "-shm")
}

fn journal_path(path: &Path) -> PathBuf {
    sidecar_path(path, "-journal")
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
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

fn invocation_count(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
