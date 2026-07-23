#![cfg(unix)]

//! Production-built, isolated runtime proof for OpenCode native resume ownership.

use oulipoly_state::{SessionTurnIngest, StateDb};
use rusqlite::params;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SESSION_ID: &str = "ses_fixture_native_storage_migration_01";
const MODEL: &str = "opencode-storage-owner";
const STALE_PROVIDER: &str = "opencode";
const CURRENT_PROVIDER: &str = "opencode5";
const STALE_CHAIN: &str = "ab164c03-137b-4786-87f0-0344f1a22008";
const CURRENT_CHAIN: &str = "d89cfee3-1be5-48e8-88af-67300c4da148";
const SECOND_CURRENT_CHAIN: &str = "e89cfee3-1be5-48e8-88af-67300c4da149";
const BASE_TIMESTAMP: &str = "2026-07-18T00:30:17.334Z";
const LATER_TIMESTAMP: &str = "2026-07-18T01:30:17.334Z";
const PRIVATE_EXPORT_SENTINEL: &str = "PRIVATE_EXPORT_SENTINEL";

#[derive(Clone, Copy)]
enum StorageState {
    OwnedUsable,
    OwnedUnusable,
    Miss,
    IndeterminateExport,
}

#[derive(Clone, Copy)]
struct ChainSeed {
    chain_id: &'static str,
    provider: &'static str,
    last_used_at: &'static str,
    started_at: &'static str,
    transition: &'static str,
    turn_count: usize,
}

impl ChainSeed {
    fn stale() -> Self {
        Self {
            chain_id: STALE_CHAIN,
            provider: STALE_PROVIDER,
            last_used_at: BASE_TIMESTAMP,
            started_at: BASE_TIMESTAMP,
            transition: "imported",
            turn_count: 0,
        }
    }

    fn current() -> Self {
        Self {
            chain_id: CURRENT_CHAIN,
            provider: CURRENT_PROVIDER,
            last_used_at: BASE_TIMESTAMP,
            started_at: BASE_TIMESTAMP,
            transition: "initial",
            turn_count: 0,
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    cache_home: PathBuf,
    models_dir: PathBuf,
    caller_cwd: PathBuf,
    original_cwd: PathBuf,
    provider_record: PathBuf,
    stale_data_home: PathBuf,
    current_data_home: PathBuf,
    stale_export_record: PathBuf,
    current_export_record: PathBuf,
}

struct RunEvidence {
    output: Output,
    stdout: String,
    stderr: String,
    provider_records: String,
}

impl RunEvidence {
    fn context(&self) -> String {
        format!(
            "status={:?}\nstdout={}\nstderr={}\nprovider_records={}",
            self.output.status.code(),
            self.stdout,
            self.stderr,
            self.provider_records
        )
    }
}

impl Fixture {
    fn new(
        chains: &[ChainSeed],
        stale_storage_state: StorageState,
        current_storage_state: StorageState,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let config_home = root.join("config");
        let data_home = root.join("data");
        let cache_home = root.join("cache");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let caller_cwd = root.join("caller-cwd");
        let original_cwd = root.join("original-cwd");
        let stale_data_home = root.join("provider-data/stale");
        let current_data_home = root.join("provider-data/current");
        let stale_base_dir = stale_data_home.join("opencode");
        let current_base_dir = current_data_home.join("opencode");
        let bin_dir = root.join("bin");
        let records_dir = root.join("records");
        let provider_record = records_dir.join("provider-launch.log");
        let stale_export_record = records_dir.join("stale-export.log");
        let current_export_record = records_dir.join("current-export.log");

        for path in [
            &models_dir,
            &caller_cwd,
            &original_cwd,
            &stale_base_dir,
            &current_base_dir,
            &bin_dir,
            &records_dir,
            &cache_home,
        ] {
            fs::create_dir_all(path).unwrap();
        }

        let stale_export = write_fake_export(
            &bin_dir.join("fake-opencode-stale"),
            STALE_PROVIDER,
            &stale_data_home,
            &stale_export_record,
            stale_storage_state,
            &original_cwd,
            root,
        );
        let current_export = write_fake_export(
            &bin_dir.join("fake-opencode-current"),
            CURRENT_PROVIDER,
            &current_data_home,
            &current_export_record,
            current_storage_state,
            &original_cwd,
            root,
        );
        let stale_command = write_provider_script(
            &bin_dir.join("launch-stale-provider"),
            STALE_PROVIDER,
            &provider_record,
        );
        let current_command = write_provider_script(
            &bin_dir.join("launch-current-provider"),
            CURRENT_PROVIDER,
            &provider_record,
        );

        fs::write(models_dir.join(format!("{MODEL}.toml")), model_toml()).unwrap();
        fs::write(
            app_config_dir.join("providers.toml"),
            providers_toml(
                &stale_command,
                &stale_base_dir,
                &stale_export,
                &current_command,
                &current_base_dir,
                &current_export,
            ),
        )
        .unwrap();

        let fixture = Self {
            _dir: dir,
            config_home,
            data_home,
            cache_home,
            models_dir,
            caller_cwd,
            original_cwd,
            provider_record,
            stale_data_home,
            current_data_home,
            stale_export_record,
            current_export_record,
        };
        fixture.seed_runner_state(chains);
        fixture
    }

    fn seed_runner_state(&self, chains: &[ChainSeed]) {
        let db_path = self
            .data_home
            .join("oulipoly-agent-runner")
            .join("state.db");
        let state = StateDb::open(&db_path).unwrap();
        for chain in chains {
            state
                .connection()
                .execute(
                    "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![chain.chain_id, BASE_TIMESTAMP, chain.last_used_at, MODEL],
                )
                .unwrap();
            state
                .connection()
                .execute(
                    "INSERT INTO session_chain_segments
                        (chain_id, provider_name, session_id, started_at, transition_reason)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        chain.chain_id,
                        chain.provider,
                        SESSION_ID,
                        chain.started_at,
                        chain.transition
                    ],
                )
                .unwrap();
            if chain.turn_count > 0 {
                state
                    .ingest_session_turns_batch(
                        chain.provider,
                        &seed_turns(chain.provider, chain.turn_count),
                    )
                    .unwrap();
            }
        }
    }

    fn run_resume(&self, input: &str) -> RunEvidence {
        let output = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
            .arg("--resume")
            .arg(input)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .current_dir(&self.caller_cwd)
            .env_clear()
            .env("HOME", self._dir.path())
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_CACHE_HOME", &self.cache_home)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        RunEvidence {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            provider_records: fs::read_to_string(&self.provider_record).unwrap_or_default(),
            output,
        }
    }

    fn assert_export_calls(&self, stale_count: usize, current_count: usize) {
        assert_export_record(
            &self.stale_export_record,
            STALE_PROVIDER,
            &self.stale_data_home,
            stale_count,
        );
        assert_export_record(
            &self.current_export_record,
            CURRENT_PROVIDER,
            &self.current_data_home,
            current_count,
        );
    }
}

fn python3() -> &'static Path {
    for candidate in [Path::new("/usr/bin/python3"), Path::new("/bin/python3")] {
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("an absolute Python 3 interpreter is required for ownership tests");
}

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_executable(path: &Path, body: &str) -> PathBuf {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
    path.to_path_buf()
}

#[allow(clippy::too_many_arguments)]
fn write_fake_export(
    path: &Path,
    provider: &str,
    expected_xdg_data_home: &Path,
    record: &Path,
    state: StorageState,
    original_cwd: &Path,
    root: &Path,
) -> PathBuf {
    let state_body = match state {
        StorageState::OwnedUsable => export_success_body(original_cwd),
        StorageState::OwnedUnusable => export_success_body(&root.join("missing-owned-workspace")),
        StorageState::Miss => "printf '%s' 'Session not found' >&2\nexit 1".to_string(),
        StorageState::IndeterminateExport => format!(
            "printf '%s' {} >&2\nexit 2",
            shell_quote(PRIVATE_EXPORT_SENTINEL)
        ),
    };
    let db_state_body = match state {
        StorageState::OwnedUsable => db_success_body(original_cwd),
        StorageState::OwnedUnusable => db_success_body(&root.join("missing-owned-workspace")),
        StorageState::Miss => "printf '%s\\n' '[]'".to_string(),
        StorageState::IndeterminateExport => format!(
            "printf '%s' {} >&2\nexit 2",
            shell_quote(PRIVATE_EXPORT_SENTINEL)
        ),
    };
    write_executable(
        path,
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s|%s|%s|%s\n' {provider} "${{XDG_DATA_HOME-}}" "${{1-}}" "${{2-}}" >> {record}
if [ "${{XDG_DATA_HOME-}}" != {expected_xdg} ]; then
  printf '%s' 'wrong isolated data root' >&2
  exit 97
fi
if [ "$#" -eq 4 ] && [ "$1" = 'db' ] && [ "$2" = '--format' ] && [ "$3" = 'json' ]; then
  {db_state_body}
  exit 0
fi
if [ "$#" -eq 2 ] && [ "$1" = 'export' ] && [ "$2" = {session_id} ]; then
  {state_body}
fi
printf '%s' 'wrong opencode argv' >&2
exit 98
"#,
            provider = shell_quote(provider),
            record = shell_quote(&record.display().to_string()),
            session_id = shell_quote(SESSION_ID),
            expected_xdg = shell_quote(&expected_xdg_data_home.display().to_string()),
        ),
    )
}

fn db_success_body(cwd: &Path) -> String {
    let rows = json!([{
        "id": SESSION_ID,
        "directory": cwd.display().to_string(),
    }]);
    format!(
        "printf '%s\\n' {}",
        shell_quote(&serde_json::to_string(&rows).unwrap())
    )
}

fn export_success_body(cwd: &Path) -> String {
    let export = json!({
        "info": {
            "id": SESSION_ID,
            "directory": cwd.display().to_string(),
        }
    });
    format!(
        "printf '%s\\n' {}",
        shell_quote(&serde_json::to_string(&export).unwrap())
    )
}

fn write_provider_script(path: &Path, provider: &str, record: &Path) -> PathBuf {
    write_executable(
        path,
        &format!(
            "#!/bin/sh\nset -eu\nprintf '%s|%s|%s\\n' {} \"$PWD\" \"$*\" >> {}\nexit 0\n",
            shell_quote(provider),
            shell_quote(&record.display().to_string()),
        ),
    )
}

fn seed_turns(provider: &str, count: usize) -> Vec<SessionTurnIngest> {
    let timestamp = chrono::DateTime::parse_from_rfc3339(BASE_TIMESTAMP)
        .unwrap()
        .with_timezone(&chrono::Utc);
    (0..count)
        .map(|index| SessionTurnIngest {
            session_id: SESSION_ID.to_string(),
            turn_id: format!("{provider}-turn-{index}"),
            timestamp,
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: Some("ordering signal".to_string()),
        })
        .collect()
}

fn model_toml() -> String {
    format!(
        r#"[[providers]]
name = "{STALE_PROVIDER}"
args = []

[[providers]]
name = "{CURRENT_PROVIDER}"
args = []
"#,
    )
}

fn providers_toml(
    stale_command: &Path,
    stale_base_dir: &Path,
    stale_export: &Path,
    current_command: &Path,
    current_base_dir: &Path,
    current_export: &Path,
) -> String {
    format!(
        r#"[{STALE_PROVIDER}]
command = {stale_command}
args = ["run"]
interactive_args = ["run"]
prompt_mode = "arg"

[{STALE_PROVIDER}.resume]
kind = "flag"
flag = "--session"

[{STALE_PROVIDER}.session_storage]
kind = "script"
cwd_script = {stale_cwd_script}

[{CURRENT_PROVIDER}]
command = {current_command}
args = ["run"]
interactive_args = ["run"]
prompt_mode = "arg"

[{CURRENT_PROVIDER}.resume]
kind = "flag"
flag = "--session"

[{CURRENT_PROVIDER}.session_storage]
kind = "script"
cwd_script = {current_cwd_script}
"#,
        stale_command = toml_string(&stale_command.display().to_string()),
        stale_cwd_script = toml_string(&opencode_cwd_command(stale_base_dir, stale_export)),
        current_command = toml_string(&current_command.display().to_string()),
        current_cwd_script = toml_string(&opencode_cwd_command(current_base_dir, current_export)),
    )
}

fn opencode_cwd_command(base_dir: &Path, export_bin: &Path) -> String {
    format!(
        "OPENCODE_BIN={} {} {} {}",
        shell_quote(&export_bin.display().to_string()),
        shell_quote(&python3().display().to_string()),
        shell_quote(&scripts_dir().join("opencode-cwd").display().to_string()),
        shell_quote(&base_dir.display().to_string()),
    )
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

fn assert_export_record(path: &Path, provider: &str, expected_xdg: &Path, expected_count: usize) {
    let records = fs::read_to_string(path).unwrap_or_default();
    let expected_db = format!("{provider}|{}|db|--format", expected_xdg.display());
    let expected_export = format!("{provider}|{}|export|{SESSION_ID}", expected_xdg.display());
    let lines = records.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), expected_count, "records={records:?}");
    assert!(
        lines
            .iter()
            .all(|line| *line == expected_db || *line == expected_export),
        "records={records:?} expected={expected_db:?} or {expected_export:?}"
    );
}

fn assert_owner_launch(fixture: &Fixture, evidence: &RunEvidence) {
    let context = evidence.context();
    assert_eq!(evidence.output.status.code(), Some(0), "{context}");
    assert!(
        evidence
            .stderr
            .contains(&format!("[resume] -> {CURRENT_PROVIDER}")),
        "{context}"
    );
    assert!(
        !evidence.stderr.contains("could not resolve original cwd"),
        "{context}"
    );
    assert_eq!(
        evidence.provider_records,
        format!(
            "{CURRENT_PROVIDER}|{}|run --session {SESSION_ID}\n",
            fixture.original_cwd.display()
        ),
        "{context}"
    );
}

fn assert_rejected_without_launch(evidence: &RunEvidence, diagnostic: &str) {
    let context = evidence.context();
    assert_eq!(evidence.output.status.code(), Some(1), "{context}");
    assert!(evidence.stderr.contains(diagnostic), "{context}");
    assert!(!evidence.stderr.contains("[resume] ->"), "{context}");
    assert!(
        !evidence.stderr.contains("could not resolve original cwd"),
        "{context}"
    );
    assert!(evidence.provider_records.is_empty(), "{context}");
}

#[test]
fn native_resume_uses_the_opencode_storage_that_owns_the_session() {
    let fixture = Fixture::new(
        &[ChainSeed::stale(), ChainSeed::current()],
        StorageState::Miss,
        StorageState::OwnedUsable,
    );
    let evidence = fixture.run_resume(SESSION_ID);
    assert_owner_launch(&fixture, &evidence);
    fixture.assert_export_calls(1, 2);
}

#[test]
fn native_resume_ownership_ignores_state_ordering_signals() {
    let variants = [
        (
            "last-used",
            ChainSeed {
                last_used_at: LATER_TIMESTAMP,
                ..ChainSeed::stale()
            },
            ChainSeed::current(),
        ),
        (
            "segment-start",
            ChainSeed {
                started_at: LATER_TIMESTAMP,
                ..ChainSeed::stale()
            },
            ChainSeed::current(),
        ),
        ("lexical-chain-id", ChainSeed::stale(), ChainSeed::current()),
        (
            "transition-label",
            ChainSeed {
                transition: "initial",
                ..ChainSeed::stale()
            },
            ChainSeed {
                transition: "imported",
                ..ChainSeed::current()
            },
        ),
        (
            "turn-count",
            ChainSeed {
                turn_count: 4,
                ..ChainSeed::stale()
            },
            ChainSeed {
                turn_count: 1,
                ..ChainSeed::current()
            },
        ),
    ];
    for (name, stale, current) in variants {
        let fixture = Fixture::new(
            &[stale, current],
            StorageState::Miss,
            StorageState::OwnedUsable,
        );
        let evidence = fixture.run_resume(SESSION_ID);
        assert!(
            evidence.output.status.success(),
            "ordering variant {name}: {}",
            evidence.context()
        );
        assert_owner_launch(&fixture, &evidence);
        fixture.assert_export_calls(1, 2);
    }
}

#[test]
fn native_resume_keeps_storage_owner_when_owned_cwd_is_unusable() {
    let fixture = Fixture::new(
        &[
            ChainSeed {
                last_used_at: LATER_TIMESTAMP,
                turn_count: 4,
                ..ChainSeed::stale()
            },
            ChainSeed::current(),
        ],
        StorageState::Miss,
        StorageState::OwnedUnusable,
    );
    let evidence = fixture.run_resume(SESSION_ID);
    let context = evidence.context();
    assert_eq!(evidence.output.status.code(), Some(0), "{context}");
    let selection = evidence
        .stderr
        .find(&format!("[resume] -> {CURRENT_PROVIDER}"))
        .expect(&context);
    let warning = evidence
        .stderr
        .find("could not resolve original cwd")
        .expect(&context);
    assert!(selection < warning, "{context}");
    assert!(
        evidence.stderr[warning..].contains(CURRENT_PROVIDER),
        "owner-named warning required: {context}"
    );
    assert!(
        evidence
            .stderr
            .contains(&fixture.caller_cwd.display().to_string()),
        "{context}"
    );
    for rejected in [
        "storage-owner-not-found",
        "storage-ownership-ambiguous",
        "storage-ownership-indeterminate",
    ] {
        assert!(!evidence.stderr.contains(rejected), "{context}");
    }
    assert_eq!(
        evidence.provider_records,
        format!(
            "{CURRENT_PROVIDER}|{}|run --session {SESSION_ID}\n",
            fixture.caller_cwd.display()
        ),
        "{context}"
    );
    fixture.assert_export_calls(1, 2);
}

#[test]
fn native_resume_rejects_when_no_candidate_storage_owns_session() {
    let fixture = Fixture::new(
        &[ChainSeed::stale(), ChainSeed::current()],
        StorageState::Miss,
        StorageState::Miss,
    );
    let evidence = fixture.run_resume(SESSION_ID);
    assert_rejected_without_launch(&evidence, "storage-owner-not-found");
    fixture.assert_export_calls(1, 1);
}

#[test]
fn native_resume_rejects_when_multiple_candidate_storages_own_session() {
    let fixture = Fixture::new(
        &[ChainSeed::stale(), ChainSeed::current()],
        StorageState::OwnedUsable,
        StorageState::OwnedUsable,
    );
    let evidence = fixture.run_resume(SESSION_ID);
    assert_rejected_without_launch(&evidence, "storage-ownership-ambiguous");
    let context = evidence.context();
    for value in [STALE_PROVIDER, CURRENT_PROVIDER, STALE_CHAIN, CURRENT_CHAIN] {
        assert!(evidence.stderr.contains(value), "{context}");
    }
    fixture.assert_export_calls(1, 1);
}

#[test]
fn native_resume_rejects_indeterminate_candidate_ownership() {
    let fixture = Fixture::new(
        &[ChainSeed::stale(), ChainSeed::current()],
        StorageState::IndeterminateExport,
        StorageState::OwnedUsable,
    );
    let evidence = fixture.run_resume(SESSION_ID);
    assert_rejected_without_launch(&evidence, "storage-ownership-indeterminate");
    let context = evidence.context();
    assert!(evidence.stderr.contains(STALE_PROVIDER), "{context}");
    assert!(
        !evidence.stderr.contains(PRIVATE_EXPORT_SENTINEL),
        "{context}"
    );
    assert!(
        !evidence.stdout.contains(PRIVATE_EXPORT_SENTINEL),
        "{context}"
    );
    assert!(
        !evidence.stderr.contains("storage-owner-not-found"),
        "{context}"
    );
    fixture.assert_export_calls(2, 1);
}

#[test]
fn native_resume_rejects_owned_provider_with_multiple_candidate_chains() {
    let fixture = Fixture::new(
        &[
            ChainSeed::current(),
            ChainSeed {
                chain_id: SECOND_CURRENT_CHAIN,
                ..ChainSeed::current()
            },
        ],
        StorageState::Miss,
        StorageState::OwnedUsable,
    );
    let evidence = fixture.run_resume(SESSION_ID);
    assert_rejected_without_launch(&evidence, "storage-owner-chain-ambiguous");
    let context = evidence.context();
    for value in [CURRENT_PROVIDER, CURRENT_CHAIN, SECOND_CURRENT_CHAIN] {
        assert!(evidence.stderr.contains(value), "{context}");
    }
    fixture.assert_export_calls(0, 1);
}

#[test]
fn exact_chain_resume_preserves_explicit_chain_compatibility() {
    let fixture = Fixture::new(
        &[ChainSeed::stale(), ChainSeed::current()],
        StorageState::OwnedUsable,
        StorageState::OwnedUsable,
    );
    let evidence = fixture.run_resume(CURRENT_CHAIN);
    assert_owner_launch(&fixture, &evidence);
    fixture.assert_export_calls(0, 1);
}

#[test]
fn single_candidate_native_resume_preserves_state_only_compatibility() {
    let fixture = Fixture::new(
        &[ChainSeed::current()],
        StorageState::Miss,
        StorageState::Miss,
    );
    let evidence = fixture.run_resume(SESSION_ID);
    let context = evidence.context();
    assert_eq!(evidence.output.status.code(), Some(0), "{context}");
    assert!(
        evidence
            .stderr
            .contains(&format!("[resume] -> {CURRENT_PROVIDER}")),
        "{context}"
    );
    assert!(
        evidence.stderr.contains("could not resolve original cwd"),
        "{context}"
    );
    assert_eq!(
        evidence.provider_records,
        format!(
            "{CURRENT_PROVIDER}|{}|run --session {SESSION_ID}\n",
            fixture.caller_cwd.display()
        ),
        "{context}"
    );
    fixture.assert_export_calls(0, 1);
}
