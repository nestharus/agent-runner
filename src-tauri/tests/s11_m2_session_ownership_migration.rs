#![cfg(unix)]

use oulipoly_state::mailbox::{MailboxDb, SessionRuntimeUpsert};
use oulipoly_state::{StateDb, CURRENT_SCHEMA_VERSION};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

const FIXED_TS: &str = "2026-06-20T10:00:00Z";
const LATER_TS: &str = "2026-06-20T10:05:00Z";

#[derive(Debug)]
struct Fixture {
    dir: tempfile::TempDir,
    state_path: PathBuf,
    mailbox_path: PathBuf,
    scratch_dir: PathBuf,
    models_dir: PathBuf,
    config_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SegmentSnapshot {
    chain_id: String,
    provider_name: String,
    session_id: String,
    started_at: String,
    ended_at: Option<String>,
    last_turn_id: Option<String>,
    transition_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnSnapshot {
    provider_name: String,
    session_id: String,
    turn_id: String,
    timestamp: String,
    role: String,
    parent_turn_id: Option<String>,
    is_sidechain: i64,
    is_compaction_boundary: i64,
    source_file: Option<String>,
    ingested_at: String,
    body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChainSnapshot {
    model_name: String,
    created_at: String,
    last_used_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnershipSnapshot {
    chains: BTreeMap<String, ChainSnapshot>,
    segments: Vec<SegmentSnapshot>,
    turns: Vec<TurnSnapshot>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state-live");
        let config_dir = dir.path().join("config");
        let scratch_dir = dir.path().join("scratch");
        let models_dir = config_dir.join("models");
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&scratch_dir).unwrap();
        fs::create_dir_all(&models_dir).unwrap();
        Self {
            state_path: state_dir.join("state.db"),
            mailbox_path: state_dir.join("pid-identity.db"),
            dir,
            scratch_dir,
            models_dir,
            config_dir,
        }
    }

    fn conn(&self) -> Connection {
        let _ = StateDb::open(&self.state_path).unwrap();
        Connection::open(&self.state_path).unwrap()
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_dir);
        cmd.env("XDG_DATA_HOME", self.dir.path().join("xdg-data"));
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.arg("migrate-session-ownership")
            .arg("--dry-run")
            .arg("--scratch-dir")
            .arg(&self.scratch_dir)
            .arg("--state-db")
            .arg(&self.state_path)
            .arg("--models-dir")
            .arg(&self.models_dir);
        cmd
    }

    fn run(&self) -> Output {
        self.command().output().unwrap()
    }

    fn write_target_config(&self) {
        fs::create_dir_all(&self.models_dir).unwrap();
        fs::write(
            self.models_dir.join(format!("{}.toml", target_model_name())),
            format!(
                "provider = {{ binary = {:?} }}\n\n[[providers]]\nname = {:?}\nargs = []\n\n[[providers]]\nname = {:?}\nargs = []\n",
                target_binary(),
                canonical_account(),
                accepted_account()
            ),
        )
        .unwrap();
        self.write_provider_entry(&canonical_account(), &self.success_script("ok-provider"));
        self.append_provider_entry(
            &accepted_account(),
            &self.success_script("accepted-provider"),
        );
    }

    fn write_target_config_with_failing_provider(&self) {
        fs::write(
            self.models_dir
                .join(format!("{}.toml", target_model_name())),
            format!(
                "provider = {{ binary = {:?} }}\n\n[[providers]]\nname = {:?}\nargs = []\n",
                target_binary(),
                canonical_account()
            ),
        )
        .unwrap();
        self.write_provider_entry(&canonical_account(), &self.failing_script("proof-fails"));
    }

    fn success_script(&self, name: &str) -> PathBuf {
        self.write_script(name, "printf '%s\\n' ok")
    }

    fn failing_script(&self, name: &str) -> PathBuf {
        self.write_script(name, "printf '%s\\n' proof-failed >&2\nexit 42")
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(format!("{name}.sh"));
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn write_provider_entry(&self, name: &str, command: &Path) {
        fs::write(
            self.config_dir.join("providers.toml"),
            provider_toml_entry(name, command),
        )
        .unwrap();
    }

    fn append_provider_entry(&self, name: &str, command: &Path) {
        let mut body = fs::read_to_string(self.config_dir.join("providers.toml")).unwrap();
        body.push_str(&provider_toml_entry(name, command));
        fs::write(self.config_dir.join("providers.toml"), body).unwrap();
    }

    fn seed_base_population(&self) -> SeededIds {
        let conn = self.conn();
        seed_chain(&conn, "chain-active", &source_model_name());
        seed_segment(
            &conn,
            "chain-active",
            &accepted_account(),
            "session-active",
            None,
            Some("turn-active-last"),
            "initial",
        );
        seed_turn(
            &conn,
            &accepted_account(),
            "session-active",
            "turn-active-last",
            "assistant",
        );

        seed_chain(&conn, "chain-unregistered", &source_model_name());
        seed_segment(
            &conn,
            "chain-unregistered",
            &unregistered_account_a(),
            "session-unregistered-a",
            None,
            Some("turn-remap-1"),
            "manual",
        );
        seed_turn(
            &conn,
            &unregistered_account_a(),
            "session-unregistered-a",
            "turn-remap-1",
            "user",
        );
        seed_turn(
            &conn,
            &unregistered_account_a(),
            "session-unregistered-a",
            "turn-remap-2",
            "assistant",
        );
        seed_turn(
            &conn,
            &accepted_account(),
            "session-unregistered-a",
            "turn-control-provider",
            "user",
        );
        seed_turn(
            &conn,
            &unregistered_account_a(),
            "session-other",
            "turn-control-session",
            "user",
        );

        seed_chain(&conn, "chain-closed", &source_model_name());
        seed_segment(
            &conn,
            "chain-closed",
            &accepted_account(),
            "session-closed",
            Some(LATER_TS),
            Some("turn-closed-last"),
            "imported",
        );
        seed_turn(
            &conn,
            &accepted_account(),
            "session-closed",
            "turn-closed-last",
            "assistant",
        );

        seed_chain(&conn, "chain-control", &target_model_name());
        seed_segment(
            &conn,
            "chain-control",
            &accepted_account(),
            "session-control",
            None,
            Some("turn-control-last"),
            "initial",
        );
        seed_turn(
            &conn,
            &accepted_account(),
            "session-control",
            "turn-control-last",
            "assistant",
        );

        seed_mailbox(
            &self.mailbox_path,
            "session-active",
            Some(self.dir.path().to_str().unwrap()),
        );
        seed_mailbox(&self.mailbox_path, "session-unregistered-a", None);
        seed_mailbox(&self.mailbox_path, "session-closed", Some("relative/path"));

        SeededIds { issue52_count: 1 }
    }
}

struct SeededIds {
    issue52_count: i64,
}

#[test]
fn rows_8_9_10_11_15_18_20_dry_run_preserves_live_db_and_reports_reversible_copy() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    let seeded = fixture.seed_base_population();
    let live_before = snapshot(&fixture.state_path);
    let hash_before = file_hash(&fixture.state_path);
    let mtime_before = file_mtime(&fixture.state_path);

    let output = fixture.run();

    assert_success(&output);
    assert_eq!(
        file_hash(&fixture.state_path),
        hash_before,
        "live DB hash changed"
    );
    assert_eq!(
        file_mtime(&fixture.state_path),
        mtime_before,
        "live DB mtime changed"
    );
    assert_eq!(
        snapshot(&fixture.state_path),
        live_before,
        "live DB rows changed"
    );

    let state_copy = find_artifact(&fixture.scratch_dir, "state-copy.db");
    let rollback_copy = find_artifact(&fixture.scratch_dir, "rollback-copy.db");
    let mailbox_copy = find_artifact(&fixture.scratch_dir, "pid-identity.db");
    let report_path = find_artifact(&fixture.scratch_dir, "dry-run-report.md");
    assert!(state_copy.exists(), "state copy missing");
    assert!(rollback_copy.exists(), "rollback copy missing");
    assert!(mailbox_copy.exists(), "mailbox sidecar copy missing");

    let copy_after = snapshot(&state_copy);
    assert_eq!(
        copy_after.chains["chain-active"].model_name,
        target_model_name()
    );
    assert_eq!(
        copy_after.chains["chain-unregistered"].model_name,
        target_model_name()
    );
    assert_eq!(
        copy_after.chains["chain-closed"].model_name,
        target_model_name()
    );
    assert_eq!(
        copy_after.chains["chain-control"].model_name,
        target_model_name()
    );
    assert_eq!(
        copy_after.chains.len(),
        live_before.chains.len(),
        "chain delete detected"
    );
    assert_eq!(
        copy_after.segments.len(),
        live_before.segments.len(),
        "segment delete detected"
    );
    assert_eq!(
        copy_after.turns.len(),
        live_before.turns.len(),
        "turn delete detected"
    );
    assert_preserved_migrated_chain_timestamps(&live_before, &copy_after);
    assert_preserved_non_owned_segment_fields(&live_before, &copy_after);
    assert_four_population_segment_ownership(&copy_after);
    assert_turn_consistency(&live_before, &copy_after);

    let rollback_after = snapshot(&rollback_copy);
    assert_eq!(
        rollback_after, live_before,
        "rollback copy did not restore preimage"
    );

    let report = read_report(&fixture.scratch_dir);
    assert_report_contains_required_fields(
        &report,
        &fixture.state_path,
        &fixture.scratch_dir,
        &[&state_copy, &rollback_copy, &mailbox_copy, &report_path],
    );
    assert_report_count(
        &report,
        "issue52_unregistered_segments",
        seeded.issue52_count,
    );
    assert_candidate_report_counts(&report);
    assert_first_forward_counts(&report);
    assert_idempotence_counts(&report);
    assert_rollback_report_counts(&report);
    assert_cwd_completeness_counts(&report);
    assert!(
        report.contains("live_db_mutated: no"),
        "report must state live DB was not mutated: {report}"
    );
    assert!(
        !report.to_lowercase().contains(&provider_token()),
        "report leaked raw provider token"
    );
}

#[test]
fn row_12_rollback_drift_fails_closed_without_restoring_drifted_copy() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    fixture
        .conn()
        .execute_batch(
            "CREATE TRIGGER s11_m2_test_chain_drift
         AFTER UPDATE OF model_name ON session_chains
         WHEN NEW.chain_id = 'chain-active'
         BEGIN
             UPDATE session_chains SET model_name = 'drifted-after-forward'
             WHERE chain_id = NEW.chain_id;
         END;",
        )
        .unwrap();

    let output = fixture.run();

    assert_failure(&output);
    let combined = combined_output(&output);
    assert!(
        combined.contains("drift")
            || read_report_if_present(&fixture.scratch_dir).contains("drift"),
        "rollback drift was not reported: {combined}"
    );
    let state_copy = find_artifact(&fixture.scratch_dir, "state-copy.db");
    let copy = snapshot(&state_copy);
    assert_eq!(
        copy.chains["chain-active"].model_name,
        "drifted-after-forward"
    );
}

#[test]
fn row_13_segment_tuple_collision_aborts_with_unchanged_copy() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let conn = fixture.conn();
    seed_segment(
        &conn,
        "chain-unregistered",
        &canonical_account(),
        "session-unregistered-a",
        Some(LATER_TS),
        Some("turn-collision-segment"),
        "manual",
    );
    let live_before = snapshot(&fixture.state_path);

    let output = fixture.run();

    assert_failure(&output);
    assert_failure_mentions(
        &output,
        &fixture.scratch_dir,
        "UNIQUE(chain_id, provider_name, session_id)",
    );
    assert_copied_db_unchanged_if_present(&fixture.scratch_dir, &live_before);
}

#[test]
fn row_14_turn_tuple_collision_aborts_with_unchanged_copy() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    fixture
        .conn()
        .execute(
            "INSERT INTO session_turns
             (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at)
             VALUES (?1, 'session-unregistered-a', 'turn-remap-1', ?2, 'user', 'fixture.jsonl', ?2)",
            params![canonical_account(), FIXED_TS],
        )
        .unwrap();
    let live_before = snapshot(&fixture.state_path);

    let output = fixture.run();

    assert_failure(&output);
    assert_failure_mentions(
        &output,
        &fixture.scratch_dir,
        "UNIQUE(provider_name, session_id, turn_id)",
    );
    assert_copied_db_unchanged_if_present(&fixture.scratch_dir, &live_before);
}

#[test]
fn row_16_absent_target_config_fails_before_copied_mutation() {
    let fixture = Fixture::new();
    fixture.seed_base_population();
    fs::write(
        fixture.models_dir.join("unrelated.toml"),
        format!(
            "[[providers]]\nname = {:?}\nargs = []\n",
            accepted_account()
        ),
    )
    .unwrap();
    let live_before = snapshot(&fixture.state_path);

    let output = fixture.run();

    assert_failure(&output);
    assert_failure_mentions(&output, &fixture.scratch_dir, "target");
    assert_copied_db_unchanged_if_present(&fixture.scratch_dir, &live_before);
    assert_eq!(snapshot(&fixture.state_path), live_before);
}

#[test]
fn row_17_provider_proof_failure_blocks_without_local_success_fallback() {
    let fixture = Fixture::new();
    fixture.write_target_config_with_failing_provider();
    fixture.seed_base_population();
    let live_before = snapshot(&fixture.state_path);

    let output = fixture.run();

    assert_failure(&output);
    let combined = combined_output(&output) + &read_report_if_present(&fixture.scratch_dir);
    assert!(
        combined.contains("proof") || combined.contains("provider"),
        "provider proof failure not visible: {combined}"
    );
    assert!(
        !combined.contains("local fallback accepted"),
        "local fallback was accepted: {combined}"
    );
    assert_eq!(snapshot(&fixture.state_path), live_before);
}

#[test]
fn row_19_preflight_records_valid_schema_and_aborts_unsupported_or_drifted_schema() {
    let valid = Fixture::new();
    valid.write_target_config();
    valid.seed_base_population();
    assert_success(&valid.run());
    let valid_report = read_report(&valid.scratch_dir);
    assert!(
        valid_report.contains("quick_check")
            && valid_report.contains(&format!("user_version: {CURRENT_SCHEMA_VERSION}")),
        "valid preflight details missing: {valid_report}"
    );

    let future = Fixture::new();
    future.write_target_config();
    future.seed_base_population();
    future
        .conn()
        .pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION + 1)
        .unwrap();
    assert_preflight_failure(&future, "user_version");

    let missing_column = Fixture::new();
    missing_column.write_target_config();
    missing_column.seed_base_population();
    missing_column
        .conn()
        .execute_batch("ALTER TABLE session_turns DROP COLUMN source_file;")
        .unwrap();
    assert_preflight_failure(&missing_column, "source_file");

    let missing_unique = Fixture::new();
    missing_unique.write_target_config();
    missing_unique.seed_base_population();
    remove_segment_unique(&missing_unique.state_path);
    assert_preflight_failure(
        &missing_unique,
        "UNIQUE(chain_id, provider_name, session_id)",
    );

    let missing_turn_unique = Fixture::new();
    missing_turn_unique.write_target_config();
    missing_turn_unique.seed_base_population();
    remove_turn_unique(&missing_turn_unique.state_path);
    assert_preflight_failure(
        &missing_turn_unique,
        "UNIQUE(provider_name, session_id, turn_id)",
    );
}

#[test]
fn row_21_state_db_open_does_not_run_session_ownership_migration_or_extend_schema_chain() {
    assert_no_schema_chain_entry();

    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let before = snapshot(&fixture.state_path);

    drop(StateDb::open(&fixture.state_path).unwrap());

    assert_eq!(
        snapshot(&fixture.state_path),
        before,
        "ordinary DB open ran ownership migration"
    );
}

fn provider_token() -> String {
    ["cla", "ude"].concat()
}

fn target_binary() -> String {
    format!("agent-runner-{}", provider_token())
}

fn target_model_name() -> String {
    format!("s11-m2-target-{}", provider_token())
}

fn source_model_name() -> String {
    format!("legacy-{}-model", provider_token())
}

fn canonical_account() -> String {
    format!("acct-main-{}", provider_token())
}

fn accepted_account() -> String {
    format!("acct-accepted-{}", provider_token())
}

fn unregistered_account_a() -> String {
    format!("acct-unreg-a-{}", provider_token())
}

fn provider_toml_entry(name: &str, command: &Path) -> String {
    format!(
        "[{name}]\ncommand = {:?}\nargs = []\nprompt_mode = \"arg\"\n\n",
        command.to_string_lossy()
    )
}

fn seed_chain(conn: &Connection, chain_id: &str, model_name: &str) {
    conn.execute(
        "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
         VALUES (?1, ?2, ?2, ?3)",
        params![chain_id, FIXED_TS, model_name],
    )
    .unwrap();
}

fn seed_segment(
    conn: &Connection,
    chain_id: &str,
    provider_name: &str,
    session_id: &str,
    ended_at: Option<&str>,
    last_turn_id: Option<&str>,
    transition_reason: &str,
) {
    conn.execute(
        "INSERT INTO session_chain_segments
         (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![chain_id, provider_name, session_id, FIXED_TS, ended_at, last_turn_id, transition_reason],
    )
    .unwrap();
}

fn seed_turn(conn: &Connection, provider_name: &str, session_id: &str, turn_id: &str, role: &str) {
    conn.execute(
        "INSERT INTO session_turns
         (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, 'fixture.jsonl', ?4, ?6)",
        params![
            provider_name,
            session_id,
            turn_id,
            FIXED_TS,
            role,
            format!("body-{turn_id}")
        ],
    )
    .unwrap();
}

fn seed_mailbox(path: &Path, session_id: &str, cwd: Option<&str>) {
    let mut db = MailboxDb::open(path).unwrap();
    db.upsert_session_runtime(SessionRuntimeUpsert {
        session_id,
        mode: "pty_interactive",
        invocation_uuid: None,
        provider_name: Some(&canonical_account()),
        model_name: Some(&target_model_name()),
        pty_control_path: None,
        models_dir: None,
        effective_cwd: cwd,
    })
    .unwrap();
}

fn snapshot(path: &Path) -> OwnershipSnapshot {
    let conn = Connection::open(path).unwrap();
    OwnershipSnapshot {
        chains: query_chains(&conn),
        segments: query_segments(&conn),
        turns: query_turns(&conn),
    }
}

fn query_chains(conn: &Connection) -> BTreeMap<String, ChainSnapshot> {
    let mut stmt = conn
        .prepare(
            "SELECT chain_id, model_name, created_at, last_used_at
             FROM session_chains
             ORDER BY chain_id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            ChainSnapshot {
                model_name: row.get(1)?,
                created_at: row.get(2)?,
                last_used_at: row.get(3)?,
            },
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn assert_preserved_migrated_chain_timestamps(
    before: &OwnershipSnapshot,
    after: &OwnershipSnapshot,
) {
    for (chain_id, chain) in &before.chains {
        let updated = after
            .chains
            .get(chain_id)
            .unwrap_or_else(|| panic!("missing chain after migration: {chain_id}"));
        if chain.model_name != updated.model_name {
            assert_eq!(
                updated.created_at, chain.created_at,
                "{chain_id} created_at"
            );
            assert_eq!(
                updated.last_used_at, chain.last_used_at,
                "{chain_id} last_used_at"
            );
        }
    }
}

fn query_segments(conn: &Connection) -> Vec<SegmentSnapshot> {
    let mut stmt = conn
        .prepare(
            "SELECT chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason
             FROM session_chain_segments
             ORDER BY chain_id, session_id, provider_name",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok(SegmentSnapshot {
            chain_id: row.get(0)?,
            provider_name: row.get(1)?,
            session_id: row.get(2)?,
            started_at: row.get(3)?,
            ended_at: row.get(4)?,
            last_turn_id: row.get(5)?,
            transition_reason: row.get(6)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn query_turns(conn: &Connection) -> Vec<TurnSnapshot> {
    let source_file_expr = if table_has_column(conn, "session_turns", "source_file") {
        "source_file"
    } else {
        "NULL"
    };
    let mut stmt = conn
        .prepare(&format!(
            "SELECT provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
                    is_sidechain, is_compaction_boundary, {source_file_expr}, ingested_at, body
             FROM session_turns
              ORDER BY session_id, turn_id, provider_name"
        ))
        .unwrap();
    stmt.query_map([], |row| {
        Ok(TurnSnapshot {
            provider_name: row.get(0)?,
            session_id: row.get(1)?,
            turn_id: row.get(2)?,
            timestamp: row.get(3)?,
            role: row.get(4)?,
            parent_turn_id: row.get(5)?,
            is_sidechain: row.get(6)?,
            is_compaction_boundary: row.get(7)?,
            source_file: row.get(8)?,
            ingested_at: row.get(9)?,
            body: row.get(10)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn assert_preserved_non_owned_segment_fields(
    before: &OwnershipSnapshot,
    after: &OwnershipSnapshot,
) {
    let after_by_key: BTreeMap<_, _> = after
        .segments
        .iter()
        .map(|segment| {
            (
                (segment.chain_id.as_str(), segment.session_id.as_str()),
                segment,
            )
        })
        .collect();
    for segment in &before.segments {
        let updated = after_by_key
            .get(&(segment.chain_id.as_str(), segment.session_id.as_str()))
            .unwrap_or_else(|| panic!("missing segment after migration: {segment:?}"));
        assert_eq!(updated.session_id, segment.session_id);
        assert_eq!(updated.started_at, segment.started_at);
        assert_eq!(updated.ended_at, segment.ended_at);
        assert_eq!(updated.last_turn_id, segment.last_turn_id);
        assert_eq!(updated.transition_reason, segment.transition_reason);
    }
}

fn assert_four_population_segment_ownership(after: &OwnershipSnapshot) {
    assert_segment_owner(after, "chain-active", &accepted_account(), "session-active");
    assert_segment_owner(
        after,
        "chain-unregistered",
        &canonical_account(),
        "session-unregistered-a",
    );
    assert_segment_owner(after, "chain-closed", &accepted_account(), "session-closed");
    assert_segment_owner(
        after,
        "chain-control",
        &accepted_account(),
        "session-control",
    );
}

fn assert_segment_owner(
    snapshot: &OwnershipSnapshot,
    chain_id: &str,
    expected_provider: &str,
    expected_session: &str,
) {
    let segment = snapshot
        .segments
        .iter()
        .find(|segment| segment.chain_id == chain_id && segment.session_id == expected_session)
        .unwrap_or_else(|| panic!("missing segment {chain_id}/{expected_session}"));
    assert_eq!(segment.provider_name, expected_provider, "{chain_id}");
    assert_eq!(segment.session_id, expected_session, "{chain_id}");
}

fn assert_turn_consistency(before: &OwnershipSnapshot, after: &OwnershipSnapshot) {
    assert_eq!(before.turns.len(), after.turns.len());

    let remapped = after
        .turns
        .iter()
        .filter(|turn| {
            turn.session_id == "session-unregistered-a" && turn.turn_id.starts_with("turn-remap")
        })
        .filter(|turn| turn.provider_name == canonical_account())
        .count();
    assert_eq!(
        remapped, 2,
        "matching turns were not remapped with segment ownership"
    );

    let control_provider = after
        .turns
        .iter()
        .find(|turn| turn.turn_id == "turn-control-provider")
        .unwrap();
    assert_eq!(control_provider.provider_name, accepted_account());
    let control_session = after
        .turns
        .iter()
        .find(|turn| turn.turn_id == "turn-control-session")
        .unwrap();
    assert_eq!(control_session.provider_name, unregistered_account_a());

    for turn in &before.turns {
        let expected_provider = if turn.provider_name == unregistered_account_a()
            && turn.session_id == "session-unregistered-a"
            && turn.turn_id.starts_with("turn-remap")
        {
            canonical_account()
        } else {
            turn.provider_name.clone()
        };
        let updated = after
            .turns
            .iter()
            .find(|updated| {
                updated.provider_name == expected_provider
                    && updated.session_id == turn.session_id
                    && updated.turn_id == turn.turn_id
            })
            .unwrap_or_else(|| panic!("missing turn after migration: {turn:?}"));

        assert_eq!(updated.provider_name, expected_provider);
        assert_eq!(updated.session_id, turn.session_id);
        assert_eq!(updated.turn_id, turn.turn_id);
        assert_eq!(updated.timestamp, turn.timestamp);
        assert_eq!(updated.role, turn.role);
        assert_eq!(updated.parent_turn_id, turn.parent_turn_id);
        assert_eq!(updated.is_sidechain, turn.is_sidechain);
        assert_eq!(updated.is_compaction_boundary, turn.is_compaction_boundary);
        assert_eq!(updated.source_file, turn.source_file);
        assert_eq!(updated.ingested_at, turn.ingested_at);
        assert_eq!(updated.body, turn.body);
    }
}

fn assert_report_contains_required_fields(
    report: &str,
    source_path: &Path,
    scratch_dir: &Path,
    scratch_paths: &[&PathBuf],
) {
    for required in [
        "candidate",
        "eligible",
        "blocked",
        "issue52_unregistered_segments",
        "live_db_mutated: no",
    ] {
        assert!(
            report.contains(required),
            "report missing {required}: {report}"
        );
    }
    assert!(
        report.contains(&source_path.display().to_string()),
        "report missing source path {}: {report}",
        source_path.display()
    );
    assert!(
        report.contains(&scratch_dir.display().to_string()),
        "report missing scratch root {}: {report}",
        scratch_dir.display()
    );
    for path in scratch_paths {
        assert!(
            report.contains(&path.display().to_string()),
            "report missing scratch artifact path {}: {report}",
            path.display()
        );
    }
    assert_phase_pragma(report, &BEFORE_FORWARD_PHASE);
    assert_phase_pragma(report, &AFTER_IDEMPOTENCE_PHASE);
    assert_phase_pragma(report, &ROLLBACK_COPY_PHASE);
}

const BEFORE_FORWARD_PHASE: [&str; 2] = ["before forward", "before_forward"];
const FIRST_FORWARD_PHASE: [&str; 3] = ["first forward", "first-forward", "first_forward"];
const AFTER_IDEMPOTENCE_PHASE: [&str; 4] = [
    "after idempotence",
    "after_idempotence",
    "idempotence second run",
    "idempotence_second_run",
];
const ROLLBACK_COPY_PHASE: [&str; 3] = ["rollback copy", "rollback_copy", "rollback"];
const CWD_PHASE: [&str; 2] = ["cwd completeness", "cwd_completeness"];

fn assert_phase_pragma(report: &str, phase: &[&str]) {
    assert_report_context_contains(report, phase, "quick_check");
    assert_eq!(
        report_context_count(report, phase, "user_version")
            .unwrap_or_else(|| panic!("report missing user_version for {phase:?}: {report}")),
        i64::from(CURRENT_SCHEMA_VERSION)
    );
}

fn assert_first_forward_counts(report: &str) {
    assert_report_context_count(
        report,
        &FIRST_FORWARD_PHASE,
        "chain_model_updates_to_apply",
        3,
    );
    assert_report_context_count(
        report,
        &FIRST_FORWARD_PHASE,
        "segment_provider_updates_to_apply",
        1,
    );
    assert_report_context_count(
        report,
        &FIRST_FORWARD_PHASE,
        "turn_provider_updates_to_apply",
        2,
    );
}

fn assert_candidate_report_counts(report: &str) {
    assert_report_count(report, "candidate_chains", 3);
    assert_report_count(report, "candidate_segments", 3);
    assert_report_count(report, "eligible_segments", 3);
    assert_report_count(report, "blocked_segments", 0);
}

fn assert_idempotence_counts(report: &str) {
    assert_report_context_count(
        report,
        &AFTER_IDEMPOTENCE_PHASE,
        "chain_model_updates_to_apply",
        0,
    );
    assert_report_context_count(
        report,
        &AFTER_IDEMPOTENCE_PHASE,
        "segment_provider_updates_to_apply",
        0,
    );
    assert_report_context_count(
        report,
        &AFTER_IDEMPOTENCE_PHASE,
        "turn_provider_updates_to_apply",
        0,
    );
}

fn assert_rollback_report_counts(report: &str) {
    assert_report_truthy(report, &ROLLBACK_COPY_PHASE, "restored");
    assert_report_context_count(report, &ROLLBACK_COPY_PHASE, "chain_mismatch", 0);
    assert_report_context_count(report, &ROLLBACK_COPY_PHASE, "segment_mismatch", 0);
    assert_report_context_count(report, &ROLLBACK_COPY_PHASE, "turn_mismatch", 0);
}

fn assert_cwd_completeness_counts(report: &str) {
    assert_report_context_count(report, &CWD_PHASE, "missing", 0);
    assert_report_context_count(report, &CWD_PHASE, "null", 1);
    assert_report_context_count(report, &CWD_PHASE, "non-absolute", 0);
}

fn assert_report_count(report: &str, key: &str, expected: i64) {
    let count = report_count(report, key)
        .unwrap_or_else(|| panic!("report missing count for {key}: {report}"));
    assert_eq!(count, expected, "report count mismatch for {key}: {report}");
}

fn report_count(report: &str, key: &str) -> Option<i64> {
    report.lines().find_map(|line| {
        if !line.contains(key) {
            return None;
        }
        extract_last_i64(line)
    })
}

fn assert_report_context_contains(report: &str, context: &[&str], key: &str) {
    assert!(
        report_context_line(report, context, key).is_some(),
        "report missing {key} in {context:?}: {report}"
    );
}

fn assert_report_context_count(report: &str, context: &[&str], key: &str, expected: i64) {
    let count = report_context_count(report, context, key)
        .unwrap_or_else(|| panic!("report missing count for {key} in {context:?}: {report}"));
    assert_eq!(
        count, expected,
        "report count mismatch for {key} in {context:?}: {report}"
    );
}

fn assert_report_truthy(report: &str, context: &[&str], key: &str) {
    let line = report_context_line(report, context, key)
        .unwrap_or_else(|| panic!("report missing {key} in {context:?}: {report}"));
    let normalized = normalize_report_text(line);
    assert!(
        normalized.contains("true") || normalized.contains("yes"),
        "report field {key} in {context:?} was not truthy: {line}\n{report}"
    );
}

fn report_context_count(report: &str, context: &[&str], key: &str) -> Option<i64> {
    report_context_line(report, context, key).and_then(extract_last_i64)
}

fn report_context_line<'a>(report: &'a str, context: &[&str], key: &str) -> Option<&'a str> {
    let normalized_key = normalize_report_text(key);
    let normalized_context: Vec<_> = context
        .iter()
        .map(|marker| normalize_report_text(marker))
        .collect();
    let lines: Vec<_> = report.lines().collect();

    for line in &lines {
        let normalized = normalize_report_text(line);
        if normalized.contains(&normalized_key)
            && normalized_context
                .iter()
                .any(|marker| normalized.contains(marker))
        {
            return Some(line);
        }
    }

    for (index, line) in lines.iter().enumerate() {
        let normalized = normalize_report_text(line);
        if !normalized_context
            .iter()
            .any(|marker| normalized.contains(marker))
        {
            continue;
        }
        for candidate in lines.iter().skip(index + 1).take(12) {
            let normalized_candidate = normalize_report_text(candidate);
            if normalized_candidate.contains(&normalized_key) {
                return Some(candidate);
            }
            if looks_like_report_header(candidate) {
                break;
            }
        }
    }
    None
}

fn normalize_report_text(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch == '_' || ch == '-' { ' ' } else { ch })
        .collect()
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .any(|name| name == column)
}

fn looks_like_report_header(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('#') || (trimmed.ends_with(':') && !trimmed.contains(char::is_whitespace))
}

fn extract_last_i64(line: &str) -> Option<i64> {
    line.split(|ch: char| !ch.is_ascii_digit())
        .rfind(|part| !part.is_empty())
        .and_then(|digits| digits.parse().ok())
}

fn read_report(scratch_dir: &Path) -> String {
    fs::read_to_string(find_artifact(scratch_dir, "dry-run-report.md")).unwrap()
}

fn read_report_if_present(scratch_dir: &Path) -> String {
    find_optional_artifact(scratch_dir, "dry-run-report.md")
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_default()
}

fn find_artifact(root: &Path, name: &str) -> PathBuf {
    find_optional_artifact(root, name)
        .unwrap_or_else(|| panic!("artifact {name} not found under {}", root.display()))
}

fn find_optional_artifact(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.file_name().and_then(|value| value.to_str()) == Some(name) {
                return Some(path);
            }
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    None
}

fn assert_success(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", combined_output(output));
}

fn assert_failure(output: &Output) {
    assert_ne!(
        output.status.code(),
        Some(0),
        "command unexpectedly succeeded"
    );
}

fn assert_failure_mentions(output: &Output, scratch_dir: &Path, needle: &str) {
    let combined = combined_output(output) + &read_report_if_present(scratch_dir);
    assert!(
        combined.contains(needle),
        "failure did not mention {needle}: {combined}"
    );
}

fn assert_copied_db_unchanged_if_present(scratch_dir: &Path, expected: &OwnershipSnapshot) {
    if let Some(state_copy) = find_optional_artifact(scratch_dir, "state-copy.db") {
        assert_eq!(
            &snapshot(&state_copy),
            expected,
            "copied DB mutated before fail-closed abort"
        );
    }
}

fn assert_preflight_failure(fixture: &Fixture, expected_reason: &str) {
    let before = snapshot(&fixture.state_path);
    let output = fixture.run();
    assert_failure(&output);
    assert_failure_mentions(&output, &fixture.scratch_dir, expected_reason);
    assert_copied_db_unchanged_if_present(&fixture.scratch_dir, &before);
}

fn combined_output(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn file_hash(path: &Path) -> Vec<u8> {
    Sha256::digest(fs::read(path).unwrap()).to_vec()
}

fn file_mtime(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

fn remove_segment_unique(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         ALTER TABLE session_chain_segments RENAME TO session_chain_segments_old;
         CREATE TABLE session_chain_segments (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             chain_id TEXT NOT NULL REFERENCES session_chains(chain_id),
             provider_name TEXT NOT NULL,
             session_id TEXT NOT NULL,
             started_at TEXT NOT NULL,
             ended_at TEXT,
             last_turn_id TEXT,
             transition_reason TEXT NOT NULL CHECK (transition_reason IN
                 ('initial', 'manual', 'quota_threshold', 'exhausted', 'imported'))
         );
         INSERT INTO session_chain_segments
             (id, chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
         SELECT id, chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason
         FROM session_chain_segments_old;
         DROP TABLE session_chain_segments_old;
         CREATE INDEX idx_segments_session ON session_chain_segments(session_id);
         CREATE INDEX idx_segments_chain_active ON session_chain_segments(chain_id, ended_at);
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
}

fn remove_turn_unique(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         ALTER TABLE session_turns RENAME TO session_turns_old;
         CREATE TABLE session_turns (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             provider_name TEXT NOT NULL,
             session_id TEXT NOT NULL,
             turn_id TEXT NOT NULL,
             timestamp TEXT NOT NULL,
             role TEXT NOT NULL,
             parent_turn_id TEXT,
             is_sidechain INTEGER NOT NULL DEFAULT 0,
             is_compaction_boundary INTEGER NOT NULL DEFAULT 0,
             source_file TEXT NOT NULL,
             ingested_at TEXT NOT NULL,
             body TEXT
         );
         INSERT INTO session_turns
             (id, provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
              is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
         SELECT id, provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
                is_sidechain, is_compaction_boundary, source_file, ingested_at, body
         FROM session_turns_old;
         DROP TABLE session_turns_old;
         CREATE INDEX idx_session_turns_provider_ts
             ON session_turns (provider_name, role, timestamp);
         CREATE INDEX idx_session_turns_session_ts
             ON session_turns (provider_name, session_id, timestamp);
         CREATE INDEX idx_session_turns_session_lookup
             ON session_turns (session_id, timestamp);
         CREATE INDEX idx_session_turns_parent
             ON session_turns (provider_name, session_id, parent_turn_id, timestamp);
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
}

fn assert_no_schema_chain_entry() {
    let migrations_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("crates")
        .join("oulipoly-state")
        .join("migrations");
    for entry in fs::read_dir(&migrations_dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(
            !name.contains("session_ownership"),
            "schema migration file must not own S11-M2: {name}"
        );
        assert!(
            !fs::read_to_string(&path)
                .unwrap()
                .contains("s11_wu4_restore_session_ownership_preimage"),
            "schema migration chain contains operational rollback table: {}",
            path.display()
        );
    }
}
