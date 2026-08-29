#![cfg(unix)]

//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter

use chrono::{DateTime, Utc};
use oulipoly_state::{ImportedSessionDisplayMetadataUpsert, StateDb};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct CliFixture {
    dir: tempfile::TempDir,
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
            dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn run_import_replace_with_stdin(&self, session_id: &str, stdin: &[u8]) -> Output {
        let mut cmd = self.command();
        cmd.arg("session").arg("import-replace").arg(session_id);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        child.stdin.as_mut().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().unwrap()
    }

    fn write_model(&self, name: &str, body: &str) -> PathBuf {
        let path = self.models_dir.join(format!("{name}.toml"));
        fs::write(&path, body).unwrap();
        path
    }

    fn providers_path(&self) -> PathBuf {
        self.app_config_dir.join("providers.toml")
    }

    fn sessions_path(&self) -> PathBuf {
        self.app_config_dir.join("sessions.toml")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap()
}

fn ts(value: &str) -> DateTime<Utc> {
    value.parse::<DateTime<Utc>>().unwrap()
}

fn read_optional(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn table_counts_if_db_exists(path: &Path) -> BTreeMap<String, i64> {
    if !path.exists() {
        return BTreeMap::new();
    }
    let conn = Connection::open(path).unwrap();
    let mut counts = BTreeMap::new();
    for table in [
        "invocations",
        "session_turns",
        "session_chains",
        "session_chain_segments",
        "provider_quotas",
        "provider_quota_windows",
    ] {
        let count = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(0);
        counts.insert(table.to_string(), count);
    }
    counts
}

fn assert_no_state_or_replace_artifacts(fixture: &CliFixture) {
    assert!(!fixture.db_path().exists());
    assert!(
        !fixture
            .data_home
            .join("oulipoly-agent-runner/replace_journal")
            .exists()
    );
}

fn migrate_config_output(fixture: &CliFixture) -> Output {
    fixture
        .command()
        .arg("migrate-config")
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .output()
        .unwrap()
}

fn seed_session_list_rows(fixture: &CliFixture) {
    let db = StateDb::open(&fixture.db_path()).unwrap();
    db.mint_imported_chain_if_absent(
        "provider-a",
        "provider-a-native",
        &ts("2026-06-01T00:01:00Z"),
        "<unknown>",
    )
    .unwrap();
    db.upsert_imported_session_display_metadata(&ImportedSessionDisplayMetadataUpsert {
        provider_name: "provider-a".to_string(),
        provider_session_id: "provider-a-native".to_string(),
        title: Some("Provider A imported".to_string()),
        cwd: Some("/tmp/provider-a".to_string()),
        turn_count: Some(3),
        provider_updated_at: Some(ts("2026-06-01T00:01:00Z")),
        seen_at: ts("2026-06-01T00:02:00Z"),
    })
    .unwrap();
    db.mint_imported_chain_if_absent(
        "opencode",
        "opencode-native",
        &ts("2026-06-01T00:03:00Z"),
        "<unknown>",
    )
    .unwrap();
    db.upsert_imported_session_display_metadata(&ImportedSessionDisplayMetadataUpsert {
        provider_name: "opencode".to_string(),
        provider_session_id: "opencode-native".to_string(),
        title: Some("OpenCode imported".to_string()),
        cwd: None,
        turn_count: Some(5),
        provider_updated_at: Some(ts("2026-06-01T00:03:00Z")),
        seen_at: ts("2026-06-01T00:04:00Z"),
    })
    .unwrap();
}

use std::io::Write;

#[test]
fn age134_session_export_invalid_session_id_returns_json_without_state_open() {
    let fixture = CliFixture::new();

    let output = fixture
        .command()
        .arg("session")
        .arg("export")
        .arg("not-a-uuid")
        .arg("--format")
        .arg("canonical-jsonl")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let json = stderr_json(&output);
    assert_eq!(json["error"]["code"], "invalid-session-id");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not-a-uuid"),
        "{json}"
    );
    assert_no_state_or_replace_artifacts(&fixture);
}

#[test]
fn age134_session_import_replace_invalid_session_id_returns_json_without_mutation() {
    let fixture = CliFixture::new();

    let output =
        fixture.run_import_replace_with_stdin("not-a-uuid", b"{\"canonical\":\"input\"}\n");

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let json = stderr_json(&output);
    assert_eq!(json["error"]["code"], "invalid-session-id");
    assert_eq!(json["error"]["input"], "not-a-uuid");
    assert_no_state_or_replace_artifacts(&fixture);
}

#[test]
fn age134_migrate_without_rebuild_errors_without_backup_or_fresh_db_creation() {
    let fixture = CliFixture::new();

    let output = fixture.command().arg("migrate").output().unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        stderr(&output).contains("missing required flag: --rebuild"),
        "{output:?}"
    );
    assert!(!fixture.db_path().exists());
    assert!(
        !fixture
            .data_home
            .join("oulipoly-agent-runner/state-backups")
            .exists()
    );
}

#[test]
fn age134_resume_list_empty_outputs_no_chains_for_user_and_hidden_syntax() {
    let uuid = "99999999-9999-4999-8999-999999999999";
    for args in [vec!["resume", "--list", uuid], vec!["resume-list", uuid]] {
        let fixture = CliFixture::new();

        let output = fixture.command().args(args).output().unwrap();

        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(stdout(&output), format!("No chains found for {uuid}\n"));
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

#[test]
fn age134_session_list_empty_outputs_clean_human_and_json() {
    let fixture = CliFixture::new();

    let output = fixture
        .command()
        .arg("session")
        .arg("list")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(stdout(&output), "No sessions found\n");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(
        !fixture.db_path().exists(),
        "session list must not create state.db"
    );

    let output = fixture
        .command()
        .arg("session")
        .arg("list")
        .arg("--json")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        Value::Array(vec![])
    );
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(
        !fixture.db_path().exists(),
        "session list --json must not create state.db"
    );
}

#[test]
fn age134_session_list_skips_startup_recovery_side_effects() {
    let fixture = CliFixture::new();
    let journal_root = fixture
        .data_home
        .join("oulipoly-agent-runner/replace_journal");
    fs::create_dir_all(&journal_root).unwrap();
    let pending = journal_root.join("session-bad.pending");
    fs::write(&pending, b"not json").unwrap();

    let output = fixture
        .command()
        .arg("session")
        .arg("list")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(stdout(&output), "No sessions found\n");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(
        pending.exists(),
        "session list must not recover pending journals"
    );
    assert!(
        !journal_root.join("quarantine/session-bad.pending").exists(),
        "session list must not quarantine pending journals"
    );
    assert!(
        !fixture.db_path().exists(),
        "session list must not create state.db"
    );
}

#[test]
fn age134_session_list_prints_seeded_rows_and_json() {
    let fixture = CliFixture::new();
    seed_session_list_rows(&fixture);

    let output = fixture
        .command()
        .arg("session")
        .arg("list")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = stdout(&output);
    assert!(stdout.contains("ACTIVE_PROVIDER_SESSION_ID"), "{stdout}");
    assert!(stdout.contains("opencode-native"), "{stdout}");
    assert!(stdout.contains("OpenCode imported"), "{stdout}");
    assert!(stdout.contains("provider-a-native"), "{stdout}");
    assert!(output.stderr.is_empty(), "{output:?}");

    let output = fixture
        .command()
        .arg("session")
        .arg("list")
        .arg("--json")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2, "{rows}");
    assert_eq!(rows[0]["active_provider"], "opencode");
    assert_eq!(rows[0]["active_provider_session_id"], "opencode-native");
    assert_eq!(rows[0]["turn_count"], 0);
    assert_eq!(rows[0]["is_imported"], true);
    assert_eq!(rows[1]["active_provider"], "provider-a");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn age134_migrate_config_top_level_runtime_fields_without_command_currently_rewrites_no_provider() {
    let fixture = CliFixture::new();
    let model_path = fixture.write_model(
        "legacy",
        r#"args = ["--runtime-like"]
prompt_mode = "arg"
"#,
    );
    let before = fs::read(&model_path).unwrap();

    let output = migrate_config_output(&fixture);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        stdout(&output).contains("migrate-config: providers_touched=0 model_files_rewritten=1"),
        "{output:?}"
    );
    assert_ne!(fs::read(&model_path).unwrap(), before);
    let after = fs::read_to_string(&model_path).unwrap();
    assert!(after.contains("args = [\"--runtime-like\"]"), "{after}");
    assert!(!after.contains("prompt_mode"), "{after}");
    assert!(
        !fixture.providers_path().exists(),
        "current behavior does not create providers.toml for this shape"
    );
}

#[test]
fn age134_migrate_config_rejects_bad_args_types_without_partial_rewrite() {
    for (name, args_line) in [("scalar", r#"args = "not-array""#), ("item", "args = [1]")] {
        let fixture = CliFixture::new();
        let model_path = fixture.write_model(
            name,
            &format!(
                r#"[[providers]]
name = "legacy"
command = "legacy-command"
{args_line}
"#
            ),
        );
        let before_model = fs::read(&model_path).unwrap();
        let before_providers = read_optional(&fixture.providers_path());

        let output = migrate_config_output(&fixture);

        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(
            stderr(&output).contains("args must be an array of strings"),
            "{output:?}"
        );
        assert_eq!(fs::read(&model_path).unwrap(), before_model);
        assert_eq!(read_optional(&fixture.providers_path()), before_providers);
    }
}

#[test]
fn age134_migrate_config_turn_script_storage_maps_codex_quotes_paths_and_ignores_unknown() {
    let fixture = CliFixture::new();
    fs::write(
        fixture.providers_path(),
        r#"[codex]
command = "codex"
args = []
prompt_mode = "arg"

[unknown]
command = "unknown"
args = []
prompt_mode = "arg"
"#,
    )
    .unwrap();
    fs::write(
        fixture.sessions_path(),
        r#"[codex]
turn_script = '''codex-turns "/tmp/storage root/with ' quote"'''
state_dir = "/tmp/state"

[unknown]
turn_script = "made-up-turns /tmp/ignored"
state_dir = "/tmp/state"
"#,
    )
    .unwrap();

    let output = migrate_config_output(&fixture);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let providers = fs::read_to_string(fixture.providers_path()).unwrap();
    let parsed = providers.parse::<toml::Table>().unwrap();
    let codex = parsed["codex"].as_table().unwrap();
    let storage = codex["session_storage"].as_table().unwrap();
    assert_eq!(storage["kind"].as_str(), Some("script"));
    assert_eq!(storage["storage_type"].as_str(), Some("codex_session"));
    assert_eq!(
        storage["cwd_script"].as_str(),
        Some("codex-cwd '/tmp/storage root/with '\\'' quote'")
    );
    assert_eq!(
        storage["transcript_script"].as_str(),
        Some("codex-locate-transcript '/tmp/storage root/with '\\'' quote'")
    );
    assert!(
        parsed["unknown"]
            .as_table()
            .unwrap()
            .get("session_storage")
            .is_none(),
        "{providers}"
    );
}

#[test]
fn age134_compaction_jsonl_skips_bad_and_non_summary_lines_and_flags_one_boundary() {
    let fixture = CliFixture::new();
    let transcript = fixture.dir.path().join("transcript.jsonl");
    fs::write(
        &transcript,
        "not json\n\
         {\"type\":\"assistant\",\"uuid\":\"ignored-normal\"}\n\
         {\"isCompactSummary\":true}\n\
         {\"isCompactSummary\":true,\"uuid\":\"compact-turn\"}\n",
    )
    .unwrap();
    fs::write(
        fixture.sessions_path(),
        format!(
            r#"[fixture-provider]
turn_script = "true"
transcript_locator = "printf '%s\n' {}"
state_dir = "{}"
"#,
            shell_quote(&transcript.to_string_lossy()),
            fixture.dir.path().join("locator-state").display()
        ),
    )
    .unwrap();
    let _ = oulipoly_state::StateDb::open(&fixture.db_path()).unwrap();
    let conn = Connection::open(fixture.db_path()).unwrap();
    seed_chain_and_turns(&conn);
    seed_owned_compaction_evidence(&fixture.db_path(), "fixture-session", "compact-turn");

    let output = fixture.command().arg("migrate-db").output().unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        stdout(&output).contains(
            "compaction backfill session: provider=fixture-provider session_id=fixture-session flagged=1"
        ),
        "{output:?}"
    );
    assert!(
        stdout(&output).contains("compaction backfill: 1 turns flagged across 1 sessions"),
        "{output:?}"
    );
    assert_eq!(compaction_flag(&conn, "compact-turn"), 1);
    assert_eq!(compaction_flag(&conn, "ignored-normal"), 0);
}

#[test]
fn age134_compaction_jsonl_adapter_boundary_ignores_extra_provider_transcript_fields() {
    const COMPACT_UUID: &str = "11111111-2222-4333-8444-555555555555";

    #[derive(Debug, Eq, PartialEq)]
    struct Observation {
        code: Option<i32>,
        stdout: String,
        stderr: Vec<u8>,
        counts: BTreeMap<String, i64>,
        flagged_count: i64,
        compact_flag: i64,
        ignored_flag: i64,
        compact_turn_projection: (String, String, String, String, String, String),
    }

    let minimal_declared_fields =
        format!("{{\"isCompactSummary\":true,\"uuid\":\"{COMPACT_UUID}\"}}\n");
    let extra_provider_transcript_fields = format!(
        r#"{{"isCompactSummary":true,"uuid":"{COMPACT_UUID}","bodyText":"provider body","role":"user","timestamp":"2099-01-01T00:00:00Z","parentUuid":"parent-from-provider","userType":"external","sessionId":"provider-session-id","version":"provider-version","metadata":{{"nested":{{"fields":true}}}}}}
"#
    );
    let mut observations = Vec::new();

    for transcript_body in [
        minimal_declared_fields.as_str(),
        extra_provider_transcript_fields.as_str(),
    ] {
        let fixture = CliFixture::new();
        let transcript = fixture.dir.path().join("transcript.jsonl");
        fs::write(&transcript, transcript_body).unwrap();
        fs::write(
            fixture.sessions_path(),
            format!(
                r#"[fixture-provider]
turn_script = "true"
transcript_locator = "printf '%s\n' {}"
state_dir = "{}"
"#,
                shell_quote(&transcript.to_string_lossy()),
                fixture.dir.path().join("locator-state").display()
            ),
        )
        .unwrap();
        let _ = oulipoly_state::StateDb::open(&fixture.db_path()).unwrap();
        let conn = Connection::open(fixture.db_path()).unwrap();
        seed_chain_and_turns(&conn);
        conn.execute(
            "UPDATE session_turns SET turn_id = ?1 WHERE turn_id = 'compact-turn'",
            params![COMPACT_UUID],
        )
        .unwrap();
        seed_owned_compaction_evidence(&fixture.db_path(), "fixture-session", COMPACT_UUID);
        let before_counts = table_counts_if_db_exists(&fixture.db_path());

        let output = fixture.command().arg("migrate-db").output().unwrap();

        let after_counts = table_counts_if_db_exists(&fixture.db_path());
        assert_eq!(before_counts, after_counts);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert!(
            stdout(&output).contains(
                "compaction backfill session: provider=fixture-provider session_id=fixture-session flagged=1"
            ),
            "{output:?}"
        );
        assert!(
            stdout(&output).contains("compaction backfill: 1 turns flagged across 1 sessions"),
            "{output:?}"
        );

        let flagged_count = conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns WHERE is_compaction_boundary = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        let compact_turn_projection = conn
            .query_row(
                "SELECT provider_name, session_id, turn_id, timestamp, role, source_file
                 FROM session_turns
                 WHERE turn_id = ?1",
                params![COMPACT_UUID],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap();

        observations.push(Observation {
            code: output.status.code(),
            stdout: stdout(&output),
            stderr: output.stderr,
            counts: after_counts,
            flagged_count,
            compact_flag: compaction_flag(&conn, COMPACT_UUID),
            ignored_flag: compaction_flag(&conn, "ignored-normal"),
            compact_turn_projection,
        });
    }

    assert_eq!(observations.len(), 2);
    for observation in &observations {
        assert_eq!(observation.flagged_count, 1);
        assert_eq!(observation.compact_flag, 1);
        assert_eq!(observation.ignored_flag, 0);
        assert_eq!(
            observation.compact_turn_projection,
            (
                "fixture-provider".to_string(),
                "fixture-session".to_string(),
                COMPACT_UUID.to_string(),
                "2026-04-17T08:00:00Z".to_string(),
                "assistant".to_string(),
                "/tmp/source.jsonl".to_string(),
            )
        );
    }
    assert_eq!(observations[1], observations[0]);
}

#[test]
fn age134_compaction_backfill_skips_segments_without_existing_source() {
    let fixture = CliFixture::new();
    let _ = oulipoly_state::StateDb::open(&fixture.db_path()).unwrap();
    let conn = Connection::open(fixture.db_path()).unwrap();
    seed_chain_and_turns(&conn);
    let before = table_counts_if_db_exists(&fixture.db_path());

    let output = fixture.command().arg("migrate-db").output().unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        stdout(&output).contains("compaction backfill: 0 turns flagged across 1 sessions"),
        "{output:?}"
    );
    assert_eq!(table_counts_if_db_exists(&fixture.db_path()), before);
}

fn seed_chain_and_turns(conn: &Connection) {
    conn.execute(
        "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
         VALUES ('fixture-chain', '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'fixture')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_chain_segments
            (chain_id, provider_name, session_id, started_at, transition_reason)
         VALUES ('fixture-chain', 'fixture-provider', 'fixture-session', '2026-04-17T08:00:00Z', 'initial')",
        [],
    )
    .unwrap();
    for (turn_id, is_boundary) in [("ignored-normal", 0_i64), ("compact-turn", 0_i64)] {
        conn.execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role, is_compaction_boundary, source_file, ingested_at)
             VALUES ('fixture-provider', 'fixture-session', ?1, '2026-04-17T08:00:00Z', 'assistant', ?2, '/tmp/source.jsonl', '2026-04-17T08:00:01Z')",
            params![turn_id, is_boundary],
        )
        .unwrap();
    }
}

fn seed_owned_compaction_evidence(path: &Path, session_id: &str, turn_uuid: &str) {
    let state = oulipoly_state::StateDb::open(path).unwrap();
    state
        .insert_owned_turn_event_rows(&[oulipoly_state::OwnedTurnEventRow {
            session_id: session_id.to_string(),
            turn_uuid: turn_uuid.to_string(),
            is_compaction_boundary: true,
            summary_metadata_json: None,
        }])
        .unwrap();
}

fn compaction_flag(conn: &Connection, turn_id: &str) -> i64 {
    conn.query_row(
        "SELECT is_compaction_boundary FROM session_turns WHERE turn_id = ?1",
        params![turn_id],
        |row| row.get(0),
    )
    .unwrap()
}

fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', r#"'\''"#))
}
