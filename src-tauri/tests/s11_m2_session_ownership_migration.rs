#![cfg(unix)]

use oulipoly_config::{ProvidersConfig, load_models};
use oulipoly_state::mailbox::{MailboxDb, SessionMetadataUpsert};
use oulipoly_state::{CURRENT_SCHEMA_VERSION, ResolvedResume, StateDb};
use rusqlite::{Connection, params};
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
    provider_bin_dir: PathBuf,
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
struct InvocationSnapshot {
    id: i64,
    model_name: String,
    provider_name: Option<String>,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SegmentRowSnapshot {
    id: i64,
    chain_id: String,
    provider_name: String,
    session_id: String,
    started_at: String,
    ended_at: Option<String>,
    last_turn_id: Option<String>,
    transition_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnRowSnapshot {
    id: i64,
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
struct FullOwnershipSnapshot {
    chains: BTreeMap<String, ChainSnapshot>,
    segments: Vec<SegmentRowSnapshot>,
    turns: Vec<TurnRowSnapshot>,
    invocations: Vec<InvocationSnapshot>,
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
    invocations: Vec<InvocationSnapshot>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state-live");
        let config_dir = dir.path().join("config");
        let scratch_dir = dir.path().join("scratch");
        let models_dir = config_dir.join("models");
        let provider_bin_dir = dir.path().join("provider-bin");
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&scratch_dir).unwrap();
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&provider_bin_dir).unwrap();
        Self {
            state_path: state_dir.join("state.db"),
            mailbox_path: state_dir.join("pid-identity.db"),
            dir,
            scratch_dir,
            models_dir,
            config_dir,
            provider_bin_dir,
        }
    }

    fn conn(&self) -> Connection {
        let _ = StateDb::open(&self.state_path).unwrap();
        Connection::open(&self.state_path).unwrap()
    }

    fn command(&self) -> Command {
        let mut cmd = self.base_command();
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

    fn base_command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_dir);
        let data_home = self.dir.path().join("xdg-data");
        cmd.env("XDG_DATA_HOME", &data_home);
        cmd.env("OULIPOLY_DATA_DIR", data_home.join("oulipoly-agent-runner"));
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        let prior_path = std::env::var_os("PATH").unwrap_or_default();
        let path_entries = std::iter::once(self.provider_bin_dir.clone())
            .chain(std::env::split_paths(&prior_path));
        cmd.env("PATH", std::env::join_paths(path_entries).unwrap());
        cmd
    }

    fn apply_command(
        &self,
        backup_dir: Option<&Path>,
        confirm_mutate_live_db: bool,
        skip_provider_proof: bool,
        confirm_skip_provider_proof: bool,
    ) -> Command {
        let mut cmd = self.base_command();
        cmd.arg("migrate-session-ownership")
            .arg("--apply")
            .arg("--state-db")
            .arg(&self.state_path)
            .arg("--models-dir")
            .arg(&self.models_dir);
        if let Some(backup_dir) = backup_dir {
            cmd.arg("--backup-dir").arg(backup_dir);
        }
        if confirm_mutate_live_db {
            cmd.arg("--confirm-mutate-live-db");
        }
        if skip_provider_proof {
            cmd.arg("--skip-provider-proof");
        }
        if confirm_skip_provider_proof {
            cmd.arg("--confirm-skip-provider-proof");
        }
        assert_live_command_guard(&cmd, self);
        cmd
    }

    fn rollback_command(&self, confirm_mutate_live_db: bool) -> Command {
        let mut cmd = self.base_command();
        cmd.arg("migrate-session-ownership")
            .arg("--rollback")
            .arg("--state-db")
            .arg(&self.state_path)
            .arg("--models-dir")
            .arg(&self.models_dir);
        if confirm_mutate_live_db {
            cmd.arg("--confirm-mutate-live-db");
        }
        assert_live_command_guard(&cmd, self);
        cmd
    }

    fn corrective_dry_run_command(&self) -> Command {
        let mut cmd = self.command();
        cmd.arg("--corrective");
        cmd
    }

    fn corrective_apply_command(
        &self,
        backup_dir: Option<&Path>,
        confirm_mutate_live_db: bool,
    ) -> Command {
        let mut cmd = self.apply_command(backup_dir, confirm_mutate_live_db, false, false);
        cmd.arg("--corrective");
        assert_live_command_guard(&cmd, self);
        cmd
    }

    fn corrective_rollback_command(&self, confirm_mutate_live_db: bool) -> Command {
        let mut cmd = self.rollback_command(confirm_mutate_live_db);
        cmd.arg("--corrective");
        assert_live_command_guard(&cmd, self);
        cmd
    }

    fn backup_dir(&self) -> PathBuf {
        let backup_dir = self.dir.path().join("backups");
        fs::create_dir_all(&backup_dir).unwrap();
        backup_dir
    }

    fn run(&self) -> Output {
        self.command().output().unwrap()
    }

    fn write_target_config(&self) {
        self.write_fake_contract_provider_binary();
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

    fn write_fake_contract_provider_binary(&self) -> PathBuf {
        let path = self.provider_bin_dir.join(target_binary());
        fs::write(&path, fake_contract_provider_script()).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn write_target_config_with_failing_provider(&self) {
        let path = self.provider_bin_dir.join(target_binary());
        fs::write(
            &path,
            "provider proof fixture is intentionally not executable\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&path, permissions).unwrap();

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

    fn write_s11_m2c_scope_configs(&self) -> S11M2cScopeModels {
        self.write_s11_m2c_scope_configs_with_storage(None)
    }

    fn write_s11_m2c_scope_configs_with_transcript_storage(&self) -> S11M2cScopeModels {
        let projects_dir = self.transcript_projects_dir();
        fs::create_dir_all(&projects_dir).unwrap();
        self.write_s11_m2c_scope_configs_with_storage(Some(&projects_dir))
    }

    fn write_s11_m2c_scope_configs_with_storage(
        &self,
        canonical_projects_dir: Option<&Path>,
    ) -> S11M2cScopeModels {
        self.write_fake_contract_provider_binary();
        fs::create_dir_all(&self.models_dir).unwrap();
        let models = S11M2cScopeModels::new();
        for model_name in [&models.target, &models.middle, &models.last] {
            self.write_s11_m2c_scope_provider_ref_model(
                model_name,
                &target_binary(),
                &[canonical_account(), accepted_account()],
            );
        }
        self.write_s11_m2c_scope_provider_ref_model(
            &models.non_family_model,
            "agent-runner-gpt",
            std::slice::from_ref(&models.non_family_account),
        );
        let canonical_command = self.success_script("scope-main-provider");
        if let Some(projects_dir) = canonical_projects_dir {
            self.write_provider_entry_with_session_storage(
                &canonical_account(),
                &canonical_command,
                projects_dir,
            );
        } else {
            self.write_provider_entry(&canonical_account(), &canonical_command);
        }
        self.append_provider_entry(
            &accepted_account(),
            &self.success_script("scope-accepted-provider"),
        );
        self.append_provider_entry(
            &models.non_family_account,
            &self.success_script("scope-gpt-provider"),
        );
        models
    }

    fn transcript_projects_dir(&self) -> PathBuf {
        self.dir.path().join("transcript-projects")
    }

    fn write_s11_m2c_scope_provider_ref_model(
        &self,
        model_name: &str,
        binary: &str,
        providers: &[String],
    ) {
        let provider_entries = providers
            .iter()
            .map(|provider| format!("[[providers]]\nname = {provider:?}\nargs = []\n"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(
            self.models_dir.join(format!("{model_name}.toml")),
            format!("provider = {{ binary = {binary:?} }}\n\n{provider_entries}"),
        )
        .unwrap();
    }

    fn write_s11_m2c_local_shadow_model(&self) {
        fs::write(
            self.models_dir
                .join(format!("{}.toml", source_model_name())),
            format!(
                "[[providers]]\nname = {:?}\nargs = []\n\n[[providers]]\nname = {:?}\nargs = []\n",
                canonical_account(),
                accepted_account()
            ),
        )
        .unwrap();
    }

    fn rewrite_scope_provider_entries_with_resume(&self) {
        fs::write(
            self.config_dir.join("providers.toml"),
            provider_toml_entry_with_resume(
                &canonical_account(),
                &self.success_script("repl-main-provider"),
            ),
        )
        .unwrap();
        let mut body = fs::read_to_string(self.config_dir.join("providers.toml")).unwrap();
        body.push_str(&provider_toml_entry_with_resume(
            &accepted_account(),
            &self.success_script("repl-accepted-provider"),
        ));
        fs::write(self.config_dir.join("providers.toml"), body).unwrap();
    }

    fn rewrite_scope_provider_entries_with_resume_storage(
        &self,
        canonical_command: &Path,
        projects_dir: &Path,
    ) {
        fs::write(
            self.config_dir.join("providers.toml"),
            provider_toml_entry_with_resume_storage(
                &canonical_account(),
                canonical_command,
                projects_dir,
            ),
        )
        .unwrap();
        let mut body = fs::read_to_string(self.config_dir.join("providers.toml")).unwrap();
        body.push_str(&provider_toml_entry_with_resume_storage(
            &accepted_account(),
            &self.success_script("repl-bound-accepted-provider"),
            projects_dir,
        ));
        fs::write(self.config_dir.join("providers.toml"), body).unwrap();
    }

    fn default_runtime_state_path(&self) -> PathBuf {
        self.dir
            .path()
            .join("xdg-data")
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn copy_state_to_default_runtime_path(&self) -> PathBuf {
        let runtime_state = self.default_runtime_state_path();
        fs::create_dir_all(runtime_state.parent().unwrap()).unwrap();
        fs::copy(&self.state_path, &runtime_state).unwrap();
        runtime_state
    }

    fn default_runtime_mailbox_path(&self) -> PathBuf {
        self.dir
            .path()
            .join("xdg-data")
            .join("oulipoly-agent-runner")
            .join("pid-identity.db")
    }

    fn repl_resume_command(&self, resume_input: &str) -> Command {
        self.repl_resume_command_with_model(resume_input, None)
    }

    fn repl_resume_command_with_model(
        &self,
        resume_input: &str,
        model_name: Option<&str>,
    ) -> Command {
        let mut cmd = self.base_command();
        cmd.current_dir(self.dir.path())
            .arg("repl")
            .arg("--resume")
            .arg(resume_input)
            .arg("--models-dir")
            .arg(&self.models_dir);
        if let Some(model_name) = model_name {
            cmd.arg(model_name);
        }
        cmd
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

    fn recording_script(&self, name: &str, argv_path: &Path) -> PathBuf {
        self.write_script(
            name,
            &format!("printf '%s\\n' \"$@\" > {}", shell_quote_path(argv_path)),
        )
    }

    fn recording_script_with_pwd(&self, name: &str, argv_path: &Path, pwd_path: &Path) -> PathBuf {
        self.write_script(
            name,
            &format!(
                "printf '%s\\n' \"$@\" > {}\npwd > {}",
                shell_quote_path(argv_path),
                shell_quote_path(pwd_path)
            ),
        )
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

    fn write_provider_entry_with_session_storage(
        &self,
        name: &str,
        command: &Path,
        projects_dir: &Path,
    ) {
        fs::write(
            self.config_dir.join("providers.toml"),
            provider_toml_entry_with_session_storage(name, command, projects_dir),
        )
        .unwrap();
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

fn shell_quote_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

struct SeededIds {
    issue52_count: i64,
}

#[derive(Debug, Clone)]
struct S11M2cScopeModels {
    target: String,
    middle: String,
    last: String,
    non_family_model: String,
    non_family_account: String,
}

#[derive(Clone, Copy, Debug)]
enum ProviderRefNonRotatedCase {
    NoBoundary,
    BoundaryNotFound,
    AlreadyBounded,
}

impl ProviderRefNonRotatedCase {
    fn label(self) -> &'static str {
        match self {
            Self::NoBoundary => "no-boundary",
            Self::BoundaryNotFound => "boundary-not-found",
            Self::AlreadyBounded => "already-bounded",
        }
    }
}

#[derive(Debug, Clone)]
struct WidenedComprehensiveSessions {
    merged_session: &'static str,
    orphan_session: &'static str,
    rotation_session: &'static str,
    accepted_session: &'static str,
}

impl S11M2cScopeModels {
    fn new() -> Self {
        Self {
            target: format!("aaa-ref-{}", provider_token()),
            middle: format!("mmm-ref-{}", provider_token()),
            last: format!("zzz-ref-{}", provider_token()),
            non_family_model: "gpt-thing".to_string(),
            non_family_account: "acct-gpt".to_string(),
        }
    }
}

const RESUME_CHAIN_ORPHAN: &str = "11111111-1111-4111-8111-111111111111";
const RESUME_CHAIN_ROTATION: &str = "22222222-2222-4222-8222-222222222222";
const RESUME_CHAIN_VALID: &str = "33333333-3333-4333-8333-333333333333";
const RESUME_CHAIN_REPL: &str = "44444444-4444-4444-8444-444444444444";
const RESUME_CHAIN_STALE_PREIMAGE: &str = "55555555-5555-4555-8555-555555555555";
const RESUME_STALE_PREIMAGE_SESSION: &str = "session-resume-stale-preimage";
const PROVIDER_REF_BOUNDARY_TURN_ID: &str = "9a9d64d6-58e5-4efe-b688-a98329ff1f4a";
const SESSION_OWNERSHIP_MIGRATION_ID: &str = "s11-m2-session-ownership";

fn load_fixture_models(fixture: &Fixture) -> oulipoly_state::ModelStore {
    let providers = ProvidersConfig::load(&fixture.config_dir.join("providers.toml")).unwrap();
    load_models(&fixture.models_dir, Some(&providers)).unwrap()
}

fn resolve_fixture_resume(fixture: &Fixture, resume_input: &str) -> ResolvedResume {
    let db = StateDb::open(&fixture.state_path).unwrap();
    let models = load_fixture_models(fixture);
    db.resolve_resume(&models, resume_input, None).unwrap()
}

fn assert_provider_ref_resume(
    fixture: &Fixture,
    resume_input: &str,
    expected_model: &str,
    expected_provider: &str,
) {
    let resolved = resolve_fixture_resume(fixture, resume_input);
    assert_eq!(resolved.model_name.as_deref(), Some(expected_model));
    assert_eq!(resolved.active_provider, expected_provider);
    assert!(
        resolved
            .model
            .as_ref()
            .is_some_and(|model| model.provider.is_some()),
        "resolved resume model must be provider-ref and skip default migration: {resolved:?}"
    );
    assert_provider_ref_default_migration_skips(&resolved);
}

fn assert_provider_ref_default_migration_skips(resolved: &ResolvedResume) {
    let manual_migrate_is_none = true;
    let skip = manual_migrate_is_none
        && resolved
            .model
            .as_ref()
            .is_some_and(|model| model.provider.is_some());
    assert!(
        skip,
        "provider-ref default migration did not skip: {resolved:?}"
    );
}

fn assert_no_external_identity_errors(output: &Output) {
    let combined = combined_output(output);
    assert!(
        !combined.contains("external rotation target"),
        "resume hit external rotation target path: {combined}"
    );
    assert!(
        !combined.contains("malformed external identity"),
        "resume hit malformed external identity path: {combined}"
    );
}

fn invocation_snapshot_for_session(path: &Path, session_id: &str) -> Vec<InvocationSnapshot> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT id, model_name, provider_name, status
             FROM invocations
             WHERE COALESCE(provider_session_id, session_id) = ?1
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map([session_id], |row| {
        Ok(InvocationSnapshot {
            id: row.get(0)?,
            model_name: row.get(1)?,
            provider_name: row.get(2)?,
            status: row.get(3)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn assert_invocations_reconciled(
    fixture: &Fixture,
    session_id: &str,
    expected_model: &str,
    expected_provider: &str,
) {
    let rows = invocation_snapshot_for_session(&fixture.state_path, session_id);
    assert!(
        !rows.is_empty(),
        "fixture has no invocations for {session_id}"
    );
    for row in rows {
        assert_eq!(row.model_name, expected_model, "invocation {row:?}");
        assert_eq!(
            row.provider_name.as_deref(),
            Some(expected_provider),
            "invocation {row:?}"
        );
    }
}

fn assert_invocation_models_and_provider_reconciled(
    fixture: &Fixture,
    session_id: &str,
    expected_provider: &str,
    expected_models_by_uuid: &[(&str, &str)],
) {
    let rows = invocation_snapshot_for_session(&fixture.state_path, session_id);
    assert_eq!(
        rows.len(),
        expected_models_by_uuid.len(),
        "unexpected invocation count for {session_id}: {rows:?}"
    );
    for row in &rows {
        assert_eq!(
            row.provider_name.as_deref(),
            Some(expected_provider),
            "invocation {row:?}"
        );
    }

    let expected: BTreeMap<&str, &str> = expected_models_by_uuid.iter().copied().collect();
    assert_eq!(
        expected.len(),
        expected_models_by_uuid.len(),
        "duplicate expected invocation UUID for {session_id}"
    );

    let conn = Connection::open(&fixture.state_path).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT invocation_uuid, model_name
             FROM invocations
             WHERE COALESCE(provider_session_id, session_id) = ?1
             ORDER BY invocation_uuid",
        )
        .unwrap();
    let actual: BTreeMap<String, String> = stmt
        .query_map([session_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert_eq!(
        actual.len(),
        expected.len(),
        "unexpected invocation UUID set for {session_id}: {actual:?}"
    );
    for (uuid, expected_model) in expected {
        assert_eq!(
            actual.get(uuid).map(String::as_str),
            Some(expected_model),
            "invocation {uuid} in {session_id}"
        );
    }
}

fn assert_invocations_preserved(
    fixture: &Fixture,
    session_id: &str,
    expected: &[InvocationSnapshot],
) {
    let actual = invocation_snapshot_for_session(&fixture.state_path, session_id);
    assert_eq!(actual, expected, "invocations changed for {session_id}");
}

fn invocation_preimage_count(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage
         WHERE entity_kind = 'invocation'",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn invocation_preimage_count_for_identity(
    path: &Path,
    invocation_id: i64,
    old_model_name: &str,
    new_model_name: &str,
    old_provider_name: &str,
    new_provider_name: &str,
) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage
         WHERE migration_id = ?1
           AND entity_kind = 'invocation'
           AND row_pk = ?2
           AND old_model_name = ?3
           AND new_model_name = ?4
           AND old_provider_name = ?5
           AND new_provider_name = ?6",
        params![
            SESSION_OWNERSHIP_MIGRATION_ID,
            invocation_id.to_string(),
            old_model_name,
            new_model_name,
            old_provider_name,
            new_provider_name,
        ],
        |row| row.get(0),
    )
    .unwrap()
}

fn seed_resume_orphaned_chain_with_shadow_invocations(fixture: &Fixture) {
    let conn = fixture.conn();
    seed_chain(&conn, RESUME_CHAIN_ORPHAN, "<unknown>");
    seed_segment(
        &conn,
        RESUME_CHAIN_ORPHAN,
        &unregistered_account_a(),
        "session-resume-orphan",
        None,
        Some("turn-resume-orphan"),
        "manual",
    );
    seed_turn(
        &conn,
        &unregistered_account_a(),
        "session-resume-orphan",
        "turn-resume-orphan",
        "assistant",
    );
    seed_invocation(
        &conn,
        "10000000-0000-4000-8000-000000000001",
        "<unknown>",
        &unregistered_account_a(),
        "session-resume-orphan",
        "failed",
        "2026-06-20T10:01:00Z",
    );
    seed_invocation(
        &conn,
        "10000000-0000-4000-8000-000000000002",
        &source_model_name(),
        &unregistered_account_a(),
        "session-resume-orphan",
        "succeeded",
        "2026-06-20T10:02:00Z",
    );
}

fn seed_resume_rotation_chain_with_shadow_invocation(fixture: &Fixture) {
    let conn = fixture.conn();
    seed_chain(&conn, RESUME_CHAIN_ROTATION, &source_model_name());
    seed_segment(
        &conn,
        RESUME_CHAIN_ROTATION,
        &unregistered_account_a(),
        "session-resume-rotation",
        None,
        Some("turn-resume-rotation"),
        "manual",
    );
    seed_turn(
        &conn,
        &unregistered_account_a(),
        "session-resume-rotation",
        "turn-resume-rotation",
        "assistant",
    );
    seed_invocation(
        &conn,
        "20000000-0000-4000-8000-000000000001",
        &source_model_name(),
        &unregistered_account_a(),
        "session-resume-rotation",
        "running",
        "2026-06-20T10:03:00Z",
    );
}

fn seed_resume_valid_chain_with_aligned_invocation(fixture: &Fixture, models: &S11M2cScopeModels) {
    let conn = fixture.conn();
    seed_chain(&conn, RESUME_CHAIN_VALID, &models.middle);
    seed_segment(
        &conn,
        RESUME_CHAIN_VALID,
        &accepted_account(),
        "session-resume-valid",
        None,
        Some("turn-resume-valid"),
        "initial",
    );
    seed_turn(
        &conn,
        &accepted_account(),
        "session-resume-valid",
        "turn-resume-valid",
        "assistant",
    );
    seed_invocation(
        &conn,
        "30000000-0000-4000-8000-000000000001",
        &models.middle,
        &accepted_account(),
        "session-resume-valid",
        "succeeded",
        "2026-06-20T10:04:00Z",
    );
}

fn seed_resume_repl_orphaned_provider_ref_chain(fixture: &Fixture) {
    let conn = fixture.conn();
    seed_chain(&conn, RESUME_CHAIN_REPL, "<unknown>");
    seed_segment(
        &conn,
        RESUME_CHAIN_REPL,
        &canonical_account(),
        "session-resume-repl",
        None,
        Some("turn-resume-repl"),
        "initial",
    );
    seed_turn(
        &conn,
        &canonical_account(),
        "session-resume-repl",
        "turn-resume-repl",
        "assistant",
    );
    seed_invocation(
        &conn,
        "40000000-0000-4000-8000-000000000001",
        "<unknown>",
        &canonical_account(),
        "session-resume-repl",
        "succeeded",
        "2026-06-20T10:05:00Z",
    );
}

fn seed_provider_ref_boundary_resume_chain(fixture: &Fixture, model_name: &str) {
    let conn = fixture.conn();
    seed_chain(&conn, RESUME_CHAIN_REPL, model_name);
    seed_segment(
        &conn,
        RESUME_CHAIN_REPL,
        &canonical_account(),
        "session-resume-repl",
        None,
        Some(PROVIDER_REF_BOUNDARY_TURN_ID),
        "initial",
    );
    seed_turn_full(
        &conn,
        &canonical_account(),
        "session-resume-repl",
        "turn-before-provider-ref-boundary",
        "2026-06-20T10:04:00Z",
        "user",
        None,
        0,
        0,
        "provider-ref-boundary.jsonl",
        "2026-06-20T10:04:00Z",
        Some("pre-boundary repl prompt"),
    );
    seed_turn_full(
        &conn,
        &canonical_account(),
        "session-resume-repl",
        PROVIDER_REF_BOUNDARY_TURN_ID,
        "2026-06-20T10:05:00Z",
        "assistant",
        Some("turn-before-provider-ref-boundary"),
        0,
        1,
        "provider-ref-boundary.jsonl",
        "2026-06-20T10:05:00Z",
        Some("compact summary"),
    );
    seed_invocation(
        &conn,
        "41000000-0000-4000-8000-000000000001",
        model_name,
        &canonical_account(),
        "session-resume-repl",
        "succeeded",
        "2026-06-20T10:05:00Z",
    );
}

fn seed_provider_ref_no_boundary_resume_chain(fixture: &Fixture, model_name: &str) {
    let conn = fixture.conn();
    seed_chain(&conn, RESUME_CHAIN_REPL, model_name);
    seed_segment(
        &conn,
        RESUME_CHAIN_REPL,
        &canonical_account(),
        "session-resume-repl",
        None,
        Some("turn-provider-ref-no-boundary"),
        "initial",
    );
    seed_turn_full(
        &conn,
        &canonical_account(),
        "session-resume-repl",
        "turn-provider-ref-no-boundary",
        "2026-06-20T10:05:00Z",
        "assistant",
        None,
        0,
        0,
        "provider-ref-no-boundary.jsonl",
        "2026-06-20T10:05:00Z",
        Some("repl prompt without recorded boundary"),
    );
    seed_invocation(
        &conn,
        "42000000-0000-4000-8000-000000000001",
        model_name,
        &canonical_account(),
        "session-resume-repl",
        "succeeded",
        "2026-06-20T10:05:00Z",
    );
}

fn stage_provider_ref_boundary_jsonl(
    projects_dir: &Path,
    cwd: &Path,
    session_id: &str,
) -> (PathBuf, String, String, String) {
    let transcript_dir = projects_dir.join(claude_project_dir_name(cwd));
    fs::create_dir_all(&transcript_dir).unwrap();
    let source_path = transcript_dir.join(format!("{session_id}.jsonl"));
    let pre_boundary = serde_json::json!({
        "uuid": "turn-before-provider-ref-boundary",
        "sessionId": session_id,
        "timestamp": "2026-06-20T10:04:00Z",
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": "pre-boundary repl prompt"}],
        },
    });
    let boundary = serde_json::json!({
        "uuid": PROVIDER_REF_BOUNDARY_TURN_ID,
        "parentUuid": "turn-before-provider-ref-boundary",
        "sessionId": session_id,
        "timestamp": "2026-06-20T10:05:00Z",
        "type": "assistant",
        "isCompactSummary": true,
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "compact summary"}],
        },
    });
    let post_boundary = serde_json::json!({
        "uuid": "turn-after-provider-ref-boundary",
        "parentUuid": PROVIDER_REF_BOUNDARY_TURN_ID,
        "sessionId": session_id,
        "timestamp": "2026-06-20T10:06:00Z",
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": "post-boundary repl prompt"}],
        },
    });
    let pre_boundary_line = pre_boundary.to_string();
    let boundary_line = boundary.to_string();
    let post_boundary_line = post_boundary.to_string();
    fs::write(
        &source_path,
        format!("{pre_boundary_line}\n{boundary_line}\n{post_boundary_line}\n"),
    )
    .unwrap();
    (
        source_path,
        boundary_line,
        pre_boundary_line,
        post_boundary_line,
    )
}

fn stage_provider_ref_jsonl_without_boundary(
    projects_dir: &Path,
    cwd: &Path,
    session_id: &str,
) -> PathBuf {
    let transcript_dir = projects_dir.join(claude_project_dir_name(cwd));
    fs::create_dir_all(&transcript_dir).unwrap();
    let source_path = transcript_dir.join(format!("{session_id}.jsonl"));
    let first = serde_json::json!({
        "uuid": "turn-provider-ref-no-boundary-a",
        "sessionId": session_id,
        "timestamp": "2026-06-20T10:04:00Z",
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": "repl no-boundary prompt"}],
        },
    });
    let second = serde_json::json!({
        "uuid": "turn-provider-ref-no-boundary-b",
        "parentUuid": "turn-provider-ref-no-boundary-a",
        "sessionId": session_id,
        "timestamp": "2026-06-20T10:05:00Z",
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "repl no-boundary answer"}],
        },
    });
    fs::write(&source_path, format!("{first}\n{second}\n")).unwrap();
    source_path
}

fn stage_provider_ref_boundary_at_head_jsonl(
    projects_dir: &Path,
    cwd: &Path,
    session_id: &str,
) -> PathBuf {
    let transcript_dir = projects_dir.join(claude_project_dir_name(cwd));
    fs::create_dir_all(&transcript_dir).unwrap();
    let source_path = transcript_dir.join(format!("{session_id}.jsonl"));
    let boundary = serde_json::json!({
        "uuid": PROVIDER_REF_BOUNDARY_TURN_ID,
        "parentUuid": null,
        "sessionId": session_id,
        "timestamp": "2026-06-20T10:05:00Z",
        "type": "assistant",
        "isCompactSummary": true,
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "compact summary"}],
        },
    });
    let post_boundary = serde_json::json!({
        "uuid": "turn-after-provider-ref-boundary",
        "parentUuid": PROVIDER_REF_BOUNDARY_TURN_ID,
        "sessionId": session_id,
        "timestamp": "2026-06-20T10:06:00Z",
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": "post-boundary repl prompt"}],
        },
    });
    fs::write(&source_path, format!("{boundary}\n{post_boundary}\n")).unwrap();
    source_path
}

fn seed_resume_reapply_reconciliation_fixture(
    fixture: &Fixture,
    models: &S11M2cScopeModels,
) -> i64 {
    let conn = fixture.conn();
    let stale_provider = unregistered_account_family(2);
    seed_chain(&conn, RESUME_CHAIN_STALE_PREIMAGE, &models.middle);
    seed_segment_with_started_at(
        &conn,
        RESUME_CHAIN_STALE_PREIMAGE,
        &stale_provider,
        RESUME_STALE_PREIMAGE_SESSION,
        FIXED_TS,
        None,
        Some("turn-resume-stale-preimage"),
        "manual",
    );
    seed_turn(
        &conn,
        &stale_provider,
        RESUME_STALE_PREIMAGE_SESSION,
        "turn-resume-stale-preimage",
        "assistant",
    );
    seed_invocation(
        &conn,
        "50000000-0000-4000-8000-000000000001",
        &models.middle,
        &stale_provider,
        RESUME_STALE_PREIMAGE_SESSION,
        "succeeded",
        "2026-06-20T10:06:00Z",
    )
}

fn seed_stale_invocation_preimage_row(
    conn: &Connection,
    invocation_id: i64,
    model_name: &str,
    provider_name: &str,
) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS s11_wu4_restore_session_ownership_preimage (
            migration_id TEXT NOT NULL,
            entity_kind TEXT NOT NULL CHECK(entity_kind IN ('chain', 'segment', 'turn', 'invocation', 'segment_delete', 'turn_delete', 'segment_merge_survivor')),
            row_pk TEXT NOT NULL,
            chain_id TEXT,
            segment_id INTEGER,
            turn_row_id INTEGER,
            old_model_name TEXT,
            new_model_name TEXT,
            old_provider_name TEXT,
            new_provider_name TEXT,
            session_id TEXT,
            segment_started_at TEXT,
            segment_ended_at TEXT,
            segment_last_turn_id TEXT,
            segment_transition_reason TEXT,
            turn_id TEXT,
            turn_timestamp TEXT,
            turn_role TEXT,
            turn_parent_turn_id TEXT,
            turn_is_sidechain INTEGER,
            turn_is_compaction_boundary INTEGER,
            turn_source_file TEXT,
            turn_ingested_at TEXT,
            turn_body TEXT,
            new_started_at TEXT,
            new_ended_at TEXT,
            new_last_turn_id TEXT,
            new_transition_reason TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (migration_id, entity_kind, row_pk)
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO s11_wu4_restore_session_ownership_preimage
         (migration_id, entity_kind, row_pk, old_model_name, new_model_name,
          old_provider_name, new_provider_name)
         VALUES (?1, 'invocation', ?2, ?3, ?3, ?4, ?4)",
        params![
            SESSION_OWNERSHIP_MIGRATION_ID,
            invocation_id.to_string(),
            model_name,
            provider_name,
        ],
    )
    .unwrap();
}

fn seed_old_check_invocation_preimage_table(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE s11_wu4_restore_session_ownership_preimage (
            migration_id TEXT NOT NULL,
            entity_kind TEXT NOT NULL CHECK(entity_kind IN ('chain', 'segment', 'turn', 'segment_delete', 'turn_delete', 'segment_merge_survivor')),
            row_pk TEXT NOT NULL,
            chain_id TEXT,
            segment_id INTEGER,
            turn_row_id INTEGER,
            old_model_name TEXT,
            new_model_name TEXT,
            old_provider_name TEXT,
            new_provider_name TEXT,
            session_id TEXT,
            segment_started_at TEXT,
            segment_ended_at TEXT,
            segment_last_turn_id TEXT,
            segment_transition_reason TEXT,
            turn_id TEXT,
            turn_timestamp TEXT,
            turn_role TEXT,
            turn_parent_turn_id TEXT,
            turn_is_sidechain INTEGER,
            turn_is_compaction_boundary INTEGER,
            turn_source_file TEXT,
            turn_ingested_at TEXT,
            turn_body TEXT,
            new_started_at TEXT,
            new_ended_at TEXT,
            new_last_turn_id TEXT,
            new_transition_reason TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (migration_id, entity_kind, row_pk)
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO s11_wu4_restore_session_ownership_preimage
         (migration_id, entity_kind, row_pk, chain_id, old_model_name, new_model_name)
         VALUES (?1, 'chain', 'old-check-chain-row', 'old-check-chain', 'old-model', 'new-model')",
        [SESSION_OWNERSHIP_MIGRATION_ID],
    )
    .unwrap();
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
fn row_13_segment_tuple_collision_merges_with_greatest_id_tiebreak() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let conn = fixture.conn();
    let first_id = seed_segment_with_started_at(
        &conn,
        "chain-unregistered",
        &canonical_account(),
        "session-unregistered-a",
        FIXED_TS,
        Some(LATER_TS),
        Some("turn-collision-segment"),
        "manual",
    );
    let winner_id = seed_segment_with_started_at(
        &conn,
        "chain-unregistered",
        &unregistered_account_family(2),
        "session-unregistered-a",
        FIXED_TS,
        None,
        Some("turn-collision-segment-open"),
        "exhausted",
    );
    assert!(
        winner_id > first_id,
        "fixture must exercise greatest-id tie-break"
    );
    seed_turn(
        &conn,
        &unregistered_account_family(2),
        "session-unregistered-a",
        "turn-collision-segment-open",
        "assistant",
    );
    drop(conn);

    let backup_dir = fixture.backup_dir();
    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let segments = query_segments_with_id(&Connection::open(&fixture.state_path).unwrap());
    let merged: Vec<_> = segments
        .iter()
        .filter(|segment| {
            segment.chain_id == "chain-unregistered"
                && segment.session_id == "session-unregistered-a"
        })
        .collect();
    assert_eq!(
        merged.len(),
        1,
        "row 13 collision group was not merged: {merged:?}"
    );
    let survivor = merged[0];
    assert_eq!(
        survivor.id, winner_id,
        "equal-start survivor must be greatest id"
    );
    assert_eq!(survivor.provider_name, canonical_account());
    assert_eq!(survivor.started_at, FIXED_TS);
    assert_eq!(survivor.ended_at, None);
    assert_eq!(
        survivor.last_turn_id.as_deref(),
        Some("turn-collision-segment-open")
    );
    assert_eq!(survivor.transition_reason, "exhausted");
}

#[test]
fn s11_m2c_segment_merge_preserves_open_survivor_when_closed_row_started_later() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    let (open_id, closed_later_id) = seed_open_survivor_collision(&fixture);
    assert!(
        closed_later_id > open_id,
        "fixture must expose the prior latest-start/id survivor bug"
    );
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let after = full_snapshot(&fixture.state_path);
    let merged: Vec<_> = after
        .segments
        .iter()
        .filter(|segment| {
            segment.chain_id == "chain-open-survivor-regression"
                && segment.session_id == "session-open-survivor-regression"
        })
        .collect();
    assert_eq!(
        merged.len(),
        1,
        "merge must collapse the post-alias tuple: {merged:?}"
    );
    let survivor = merged[0];
    assert_eq!(survivor.id, open_id, "open segment must survive");
    assert_eq!(survivor.provider_name, canonical_account());
    assert_eq!(survivor.started_at, "2026-06-20T10:00:00Z");
    assert_eq!(survivor.ended_at, None, "merged segment must stay open");
    assert_eq!(
        survivor.last_turn_id.as_deref(),
        Some("turn-closed-later-tail"),
        "latest boundary tail must be selected independently from survivor identity"
    );
    assert_eq!(survivor.transition_reason, "manual");
    let open_count = after
        .segments
        .iter()
        .filter(|segment| {
            segment.chain_id == "chain-open-survivor-regression" && segment.ended_at.is_none()
        })
        .count();
    assert_eq!(
        open_count, 1,
        "chain must have exactly one open segment after merge"
    );

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_eq!(full_snapshot(&fixture.state_path), before);
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
    assert_failure_mentions(&output, &fixture.scratch_dir, "divergent");
    assert_copied_db_unchanged_if_present(&fixture.scratch_dir, &live_before);
}

#[test]
fn s11_m2c_t1_segment_merge_three_sequential_segments_preserves_latest_and_unrelated_rows() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_three_segment_collision(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let conn = Connection::open(&fixture.state_path).unwrap();
    assert_zero_collision_counts(&conn);
    let after_segments = query_segments_with_id(&conn);
    let after = full_snapshot(&fixture.state_path);
    let planned_dedup_losers = planned_dedup_loser_ids(
        &before,
        "session-merge",
        &["turn-merge-first", "turn-merge-middle", "turn-merge-last"],
    );
    assert!(
        planned_dedup_losers.is_empty(),
        "T1 fixture should have only unique turns: {planned_dedup_losers:?}"
    );
    assert_preserved_non_dedup_turns_by_id(&before, &after, &planned_dedup_losers);
    let merged: Vec<_> = after_segments
        .iter()
        .filter(|segment| {
            segment.chain_id == "chain-merge-three" && segment.session_id == "session-merge"
        })
        .collect();
    assert_eq!(
        merged.len(),
        1,
        "sequential segments were not merged: {merged:?}"
    );
    let survivor = merged[0];
    assert_eq!(survivor.started_at, "2026-06-20T10:00:00Z");
    assert_eq!(
        survivor.ended_at, None,
        "latest open segment must keep merged segment open"
    );
    assert_eq!(survivor.last_turn_id.as_deref(), Some("turn-merge-last"));
    assert_eq!(survivor.transition_reason, "exhausted");
    assert_eq!(survivor.provider_name, canonical_account());
    assert_eq!(
        survivor.id,
        latest_segment_id(&before, "chain-merge-three", "session-merge")
    );

    let unrelated_before = before
        .segments
        .iter()
        .find(|segment| {
            segment.chain_id == "chain-merge-three" && segment.session_id == "session-unrelated"
        })
        .unwrap();
    let unrelated_after = after_segments
        .iter()
        .find(|segment| {
            segment.chain_id == "chain-merge-three" && segment.session_id == "session-unrelated"
        })
        .unwrap();
    assert_eq!(unrelated_after.id, unrelated_before.id);
    assert_eq!(unrelated_after.started_at, unrelated_before.started_at);
    assert_eq!(unrelated_after.ended_at, unrelated_before.ended_at);
    assert_eq!(unrelated_after.last_turn_id, unrelated_before.last_turn_id);
    assert_eq!(
        unrelated_after.transition_reason,
        unrelated_before.transition_reason
    );
}

#[test]
fn s11_m2c_t2_turn_dedup_identical_duplicates_preserves_min_id_content_and_counts() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    let expected_winner = seed_turn_dedup_collision(&fixture);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let conn = Connection::open(&fixture.state_path).unwrap();
    assert_zero_collision_counts(&conn);
    let turns = query_turns_with_id(&conn);
    let survivors: Vec<_> = turns
        .iter()
        .filter(|turn| turn.session_id == "session-dedup" && turn.turn_id == "turn-dedup")
        .collect();
    assert_eq!(
        survivors.len(),
        1,
        "duplicate turn rows were not deduped: {survivors:?}"
    );
    let survivor = survivors[0];
    assert_eq!(
        survivor.id, expected_winner.id,
        "dedup winner must be MIN(id)"
    );
    assert_eq!(survivor.provider_name, canonical_account());
    assert_same_turn_content(survivor, &expected_winner);
    assert_eq!(last_run_count(&conn, "turn_rows_deduped_away"), 1);
    assert_eq!(last_run_count(&conn, "segment_rows_merged_away"), 1);
    assert_eq!(last_run_count(&conn, "segment_merge_survivors_updated"), 1);
}

#[test]
fn s11_m2c_t3_t5_divergent_turn_collision_aborts_before_mutation() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_divergent_turn_collision(&fixture);
    let live_before = snapshot(&fixture.state_path);
    let hash_before = file_hash(&fixture.state_path);

    let output = fixture.run();

    assert_failure(&output);
    assert_failure_mentions(&output, &fixture.scratch_dir, "divergent");
    assert_eq!(
        file_hash(&fixture.state_path),
        hash_before,
        "live DB hash changed"
    );
    assert_eq!(
        snapshot(&fixture.state_path),
        live_before,
        "live DB rows changed"
    );
    assert_copied_db_unchanged_if_present(&fixture.scratch_dir, &live_before);
}

#[test]
fn s11_m2c_fix_ingested_at_only_turn_collision_dedups_min_id_and_rolls_back_exactly() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    let expected_winner = seed_ingested_at_only_turn_collision(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let conn = Connection::open(&fixture.state_path).unwrap();
    assert_zero_collision_counts(&conn);
    let after = full_snapshot(&fixture.state_path);
    let planned_losers = planned_dedup_loser_ids(
        &before,
        "session-ingested-at-metadata",
        &["turn-ingested-at-metadata"],
    );
    assert_eq!(
        planned_losers.len(),
        2,
        "fixture must have three colliding rows"
    );
    assert_preserved_non_dedup_turns_by_id(&before, &after, &planned_losers);
    let survivor = single_turn_by_key(
        &after,
        "session-ingested-at-metadata",
        "turn-ingested-at-metadata",
    );
    assert_eq!(
        survivor.id, expected_winner.id,
        "dedup winner must be MIN(id)"
    );
    assert_eq!(survivor.provider_name, canonical_account());
    assert_same_turn_content(survivor, &expected_winner);
    assert_eq!(last_run_count(&conn, "turn_rows_deduped_away"), 2);
    assert_eq!(last_run_count(&conn, "segment_rows_merged_away"), 2);
    assert_live_counts_and_zero_residual(&fixture.state_path);
    drop(conn);

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_eq!(full_snapshot(&fixture.state_path), before);
}

#[test]
fn s11_m2c_fix_parent_turn_id_only_turn_collision_dedups_and_keeps_winner_parent() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    let expected_winner = seed_parent_turn_id_only_turn_collision(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let conn = Connection::open(&fixture.state_path).unwrap();
    assert_zero_collision_counts(&conn);
    let after = full_snapshot(&fixture.state_path);
    let planned_losers = planned_dedup_loser_ids(
        &before,
        "session-parent-metadata",
        &["turn-parent-metadata"],
    );
    assert_eq!(planned_losers.len(), 1, "fixture must have one dedup loser");
    assert_preserved_non_dedup_turns_by_id(&before, &after, &planned_losers);
    let survivor = single_turn_by_key(&after, "session-parent-metadata", "turn-parent-metadata");
    assert_eq!(
        survivor.id, expected_winner.id,
        "dedup winner must be MIN(id)"
    );
    assert_eq!(survivor.provider_name, canonical_account());
    assert_eq!(
        survivor.parent_turn_id, expected_winner.parent_turn_id,
        "survivor must retain winner's own parent_turn_id"
    );
    assert_same_turn_content(survivor, &expected_winner);
    assert_eq!(last_run_count(&conn, "turn_rows_deduped_away"), 1);
}

#[test]
fn s11_m2c_fix_body_divergence_aborts_apply_without_mutation() {
    assert_intrinsic_divergence_aborts_apply_without_mutation("body");
}

#[test]
fn s11_m2c_fix_role_divergence_aborts_apply_without_mutation() {
    assert_intrinsic_divergence_aborts_apply_without_mutation("role");
}

#[test]
fn s11_m2c_fix_timestamp_divergence_aborts_apply_without_mutation() {
    assert_intrinsic_divergence_aborts_apply_without_mutation("timestamp");
}

#[test]
fn s11_m2c_t7_rollback_restores_merged_segments_and_deleted_turns_exactly() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_three_segment_collision(&fixture);
    seed_turn_dedup_collision(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    let after_apply = full_snapshot(&fixture.state_path);
    assert_ne!(after_apply, before);
    let expected_segment_rows_reinserted =
        before.segments.len() as i64 - after_apply.segments.len() as i64;

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_eq!(full_snapshot(&fixture.state_path), before);
    let report = read_report_from_output(&rollback);
    assert_report_context_count(
        &report,
        &ROLLBACK_COPY_PHASE,
        "segment_rows_reinserted",
        expected_segment_rows_reinserted,
    );
    assert_report_context_count(&report, &ROLLBACK_COPY_PHASE, "turn_rows_reinserted", 1);
    assert_report_context_count(
        &report,
        &ROLLBACK_COPY_PHASE,
        "segment_merge_survivors_restored",
        2,
    );
}

#[test]
fn s11_m2c_t8_rollback_aborts_when_deleted_segment_id_is_occupied() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_three_segment_collision(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    let after_apply = full_snapshot(&fixture.state_path);
    let deleted_id = first_deleted_segment_id(&before, &after_apply);
    occupy_segment_id(&fixture.state_path, deleted_id);
    let drifted = full_snapshot(&fixture.state_path);

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_failure(&rollback);
    assert_failure_mentions(&rollback, fixture.dir.path(), "drift");
    assert_eq!(
        full_snapshot(&fixture.state_path),
        drifted,
        "rollback mutated after drift"
    );
}

#[test]
fn s11_m2c_t8_rollback_aborts_when_deleted_turn_id_is_occupied() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    let before_winner = seed_turn_dedup_collision(&fixture);
    let before = full_snapshot(&fixture.state_path);
    assert!(
        before
            .turns
            .iter()
            .any(|turn| turn.id != before_winner.id && turn.session_id == "session-dedup"),
        "fixture must include a dedup loser"
    );
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    let after_apply = full_snapshot(&fixture.state_path);
    let deleted_id = first_deleted_turn_id(&before, &after_apply);
    occupy_turn_id(&fixture.state_path, deleted_id);
    let drifted = full_snapshot(&fixture.state_path);

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_failure(&rollback);
    assert_failure_mentions(&rollback, fixture.dir.path(), "drift");
    assert_eq!(
        full_snapshot(&fixture.state_path),
        drifted,
        "rollback mutated after drift"
    );
}

#[test]
fn s11_m2c_t15_rollback_aborts_when_merge_survivor_row_is_missing() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_three_segment_collision(&fixture);
    seed_turn_dedup_collision(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    let survivor_id = latest_segment_id(&before, "chain-merge-three", "session-merge");
    delete_segment_id(&fixture.state_path, survivor_id);
    let drifted = full_snapshot(&fixture.state_path);

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_failure(&rollback);
    assert_failure_mentions_survivor_missing_or_absent(&rollback, fixture.dir.path());
    assert_failure_does_not_mention(
        &rollback,
        fixture.dir.path(),
        "segment merge survivor drift before rollback",
    );
    assert_eq!(
        full_snapshot(&fixture.state_path),
        drifted,
        "rollback mutated after survivor missing drift"
    );
}

#[test]
fn s11_m2c_t16_rollback_aborts_when_merge_survivor_chain_or_session_identity_drifts() {
    assert_merge_survivor_identity_drift_detected("chain_id");
    assert_merge_survivor_identity_drift_detected("session_id");
}

#[test]
fn s11_m2c_t9_idempotent_after_collision_resolution_has_zero_counts() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_three_segment_collision(&fixture);
    seed_open_survivor_collision(&fixture);
    seed_turn_dedup_collision(&fixture);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    let after_first = full_snapshot(&fixture.state_path);

    let second = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&second);
    assert_eq!(full_snapshot(&fixture.state_path), after_first);
    let report = read_report_from_output(&second);
    for key in [
        "chain_model_updates_to_apply",
        "segment_provider_updates_to_apply",
        "turn_provider_updates_to_apply",
        "invocation_identity_updates_to_apply",
        "segment_rows_merged_away",
        "turn_rows_deduped_away",
        "segment_merge_survivors_updated",
    ] {
        assert_report_count(&report, key, 0);
    }
}

#[test]
fn s11_m2c_t11_larger_multi_account_rotation_merges_and_dedups() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_large_rotation_collision(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let conn = Connection::open(&fixture.state_path).unwrap();
    assert_zero_collision_counts(&conn);
    let after = full_snapshot(&fixture.state_path);
    let planned_merged_away =
        planned_merged_segment_count(&before, "chain-large-rotation", "session-large");
    let planned_dedup_losers = planned_dedup_loser_ids(
        &before,
        "session-large",
        &[
            "turn-large-1",
            "turn-large-2",
            "turn-large-3",
            "turn-large-unique",
        ],
    );
    assert_eq!(
        planned_merged_away, 3,
        "fixture planned segment merge count changed"
    );
    assert_eq!(
        planned_dedup_losers.len(),
        9,
        "fixture planned turn dedup count changed"
    );
    assert_eq!(
        before.segments.len() as i64 - after.segments.len() as i64,
        planned_merged_away,
        "segment row delta must equal only planned merged-away rows"
    );
    assert_preserved_non_dedup_turns_by_id(&before, &after, &planned_dedup_losers);
    assert_eq!(
        after
            .segments
            .iter()
            .filter(|segment| segment.chain_id == "chain-large-rotation"
                && segment.session_id == "session-large")
            .count(),
        1
    );
    for turn_id in ["turn-large-1", "turn-large-2", "turn-large-3"] {
        let matching: Vec<_> = after
            .turns
            .iter()
            .filter(|turn| turn.session_id == "session-large" && turn.turn_id == turn_id)
            .collect();
        assert_eq!(matching.len(), 1, "{turn_id} was not deduped: {matching:?}");
        assert_eq!(
            matching[0].id,
            min_turn_id(&before, "session-large", turn_id)
        );
        assert_eq!(matching[0].provider_name, canonical_account());
    }
    assert_eq!(last_run_count(&conn, "segment_rows_merged_away"), 3);
    assert_eq!(last_run_count(&conn, "turn_rows_deduped_away"), 9);
    assert_large_unrelated_rows_preserved_with_expected_remap(&before, &after);
}

#[test]
fn s11_m2c_t12_post_apply_zero_collisions_and_report_shows_merge_dedup_counts() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_turn_dedup_collision(&fixture);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let conn = Connection::open(&fixture.state_path).unwrap();
    assert_zero_collision_counts(&conn);
    assert_eq!(
        last_run_count(&conn, "post_apply_segment_collision_count"),
        0
    );
    assert_eq!(last_run_count(&conn, "post_apply_turn_collision_count"), 0);
    let report = read_report_from_output(&output);
    assert_report_count(&report, "segment_rows_merged_away", 1);
    assert_report_count(&report, "turn_rows_deduped_away", 1);
    assert_report_truthy_or_zero(&report, "zero remaining segment collisions");
    assert_report_truthy_or_zero(&report, "zero remaining turn collisions");
}

#[test]
fn s11_m2c_t13_preflight_requires_session_turn_body() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    fixture
        .conn()
        .execute_batch("ALTER TABLE session_turns DROP COLUMN body;")
        .unwrap();

    assert_preflight_failure(&fixture, "body");
}

#[test]
fn s11_m2c_t14_dry_run_collision_resolution_preserves_live_and_rolls_back_copy_exactly() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_three_segment_collision(&fixture);
    seed_turn_dedup_collision(&fixture);
    let live_before = full_snapshot(&fixture.state_path);
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
        full_snapshot(&fixture.state_path),
        live_before,
        "live DB rows changed"
    );
    let state_copy = find_artifact(&fixture.scratch_dir, "state-copy.db");
    let rollback_copy = find_artifact(&fixture.scratch_dir, "rollback-copy.db");
    let copy_after = full_snapshot(&state_copy);
    let planned_dedup_losers =
        planned_dedup_loser_ids(&live_before, "session-dedup", &["turn-dedup"]);
    assert_eq!(
        planned_dedup_losers.len(),
        1,
        "fixture planned turn dedup count changed"
    );
    assert_preserved_non_dedup_turns_by_id(&live_before, &copy_after, &planned_dedup_losers);
    assert_ne!(
        copy_after, live_before,
        "state copy did not apply merge/dedup"
    );
    assert_eq!(
        copy_after
            .segments
            .iter()
            .filter(|segment| segment.chain_id == "chain-merge-three"
                && segment.session_id == "session-merge")
            .count(),
        1
    );
    assert_eq!(
        copy_after
            .turns
            .iter()
            .filter(|turn| turn.session_id == "session-dedup" && turn.turn_id == "turn-dedup")
            .count(),
        1
    );
    assert_eq!(full_snapshot(&rollback_copy), live_before);
}

#[test]
fn s11_m2c_turns_merged_away_rotation_owner_is_canonicalized_at_session_level() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    let turn_ids = seed_merged_away_rotation_turns(&fixture);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let after = full_snapshot(&fixture.state_path);
    assert_turns_owned_by(
        &after,
        "session-turns-merged-away",
        &turn_ids,
        &canonical_account(),
    );
    assert_no_noncanonical_rotation_turns_in_sessions(&after, &["session-turns-merged-away"]);
    let conn = Connection::open(&fixture.state_path).unwrap();
    assert_eq!(last_run_count(&conn, "turn_provider_updates_to_apply"), 2);
}

#[test]
fn s11_m2c_turns_byte_identical_widened_collision_dedups_min_id() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    let expected_winner = seed_merged_away_identical_turn_collision(&fixture);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let conn = Connection::open(&fixture.state_path).unwrap();
    assert_zero_collision_counts(&conn);
    assert_eq!(last_run_count(&conn, "turn_rows_deduped_away"), 1);
    assert_eq!(last_run_count(&conn, "post_apply_turn_collision_count"), 0);
    let after = full_snapshot(&fixture.state_path);
    let survivor = single_turn_by_key(&after, "session-turns-identical", "turn-identical");
    assert_eq!(
        survivor.id, expected_winner.id,
        "dedup winner must be MIN(id)"
    );
    assert_eq!(survivor.provider_name, canonical_account());
    assert_same_turn_content(survivor, &expected_winner);
}

#[test]
fn s11_m2c_turns_divergent_widened_collision_aborts_without_mutation() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_merged_away_divergent_turn_collision(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let hash_before = file_hash(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_failure(&output);
    assert_failure_mentions(&output, fixture.dir.path(), "divergent");
    assert_eq!(
        file_hash(&fixture.state_path),
        hash_before,
        "live DB hash changed"
    );
    assert_eq!(
        full_snapshot(&fixture.state_path),
        before,
        "live DB rows changed"
    );
}

#[test]
fn s11_m2c_turns_comprehensive_zero_residual_covers_segments_and_turns() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    let sessions = seed_widened_turn_comprehensive_population(&fixture);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_quick_check_ok(&fixture.state_path);
    assert_live_counts_and_zero_residual(&fixture.state_path);
    let after = full_snapshot(&fixture.state_path);
    assert_turns_owned_by(
        &after,
        sessions.merged_session,
        &["turn-comprehensive-historical"],
        &canonical_account(),
    );
    assert_turns_owned_by(
        &after,
        sessions.orphan_session,
        &["turn-comprehensive-orphan"],
        &canonical_account(),
    );
    assert_turns_owned_by(
        &after,
        sessions.rotation_session,
        &["turn-comprehensive-rotation"],
        &canonical_account(),
    );
    assert_turns_owned_by(
        &after,
        sessions.accepted_session,
        &["turn-comprehensive-accepted"],
        &accepted_account(),
    );
    assert_no_noncanonical_rotation_turns_in_sessions(
        &after,
        &[
            sessions.merged_session,
            sessions.orphan_session,
            sessions.rotation_session,
        ],
    );
}

#[test]
fn s11_m2c_turns_inventory_provider_matching_family_is_not_remapped() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_accepted_inventory_turn(&fixture);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let after = full_snapshot(&fixture.state_path);
    assert_turns_owned_by(
        &after,
        "session-turns-accepted",
        &["turn-accepted-inventory"],
        &accepted_account(),
    );
}

#[test]
fn s11_m2c_turns_rollback_restores_widened_updates_and_dedup_deletes_exactly() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_widened_rollback_population(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let after_apply = full_snapshot(&fixture.state_path);
    assert_ne!(after_apply, before);
    let planned_losers =
        planned_dedup_loser_ids(&before, "session-turns-identical", &["turn-identical"]);
    assert_eq!(
        planned_losers.len(),
        1,
        "fixture must include one widened dedup loser"
    );
    assert_preserved_non_dedup_turns_by_id(&before, &after_apply, &planned_losers);

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_eq!(full_snapshot(&fixture.state_path), before);
    let report = read_report_from_output(&rollback);
    assert_report_context_count(&report, &ROLLBACK_COPY_PHASE, "turn_rows_reinserted", 1);
}

#[test]
fn s11_m2c_turns_reapply_after_widened_success_is_clean_noop() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_widened_rollback_population(&fixture);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    let after_first = full_snapshot(&fixture.state_path);

    let second = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&second);
    assert_eq!(full_snapshot(&fixture.state_path), after_first);
    let report = read_report_from_output(&second);
    for key in [
        "chain_model_updates_to_apply",
        "segment_provider_updates_to_apply",
        "turn_provider_updates_to_apply",
        "invocation_identity_updates_to_apply",
        "segment_rows_merged_away",
        "turn_rows_deduped_away",
        "segment_merge_survivors_updated",
    ] {
        assert_report_count(&report, key, 0);
    }
}

#[test]
fn s11_m2c_turns_fresh_widened_runs_are_deterministic() {
    let first = apply_widened_fixture_snapshot_and_counts();
    let second = apply_widened_fixture_snapshot_and_counts();

    assert_eq!(first, second);
}

#[test]
fn s11_m2c_turns_session_ids_are_stable_across_apply_and_rollback() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_widened_rollback_population(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let after_apply = full_snapshot(&fixture.state_path);
    assert_surviving_session_ids_preserved(&before, &after_apply);

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_eq!(full_snapshot(&fixture.state_path), before);
}

#[test]
fn s11_m2c_turns_dry_run_widened_fixture_preserves_live_and_rolls_back_copy() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_widened_rollback_population(&fixture);
    let live_before = full_snapshot(&fixture.state_path);
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
        full_snapshot(&fixture.state_path),
        live_before,
        "live DB rows changed"
    );
    let state_copy = find_artifact(&fixture.scratch_dir, "state-copy.db");
    let rollback_copy = find_artifact(&fixture.scratch_dir, "rollback-copy.db");
    let copy_after = full_snapshot(&state_copy);
    assert_no_noncanonical_rotation_turns_in_sessions(
        &copy_after,
        &["session-turns-merged-away", "session-turns-identical"],
    );
    assert_eq!(full_snapshot(&rollback_copy), live_before);
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

#[test]
fn apply_on_fixture_success_migrates_candidate_chains_segments_and_turns_to_external_ownership() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let before = snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let after = snapshot(&fixture.state_path);
    assert_eq!(after.chains["chain-active"].model_name, target_model_name());
    assert_eq!(
        after.chains["chain-unregistered"].model_name,
        target_model_name()
    );
    assert_eq!(after.chains["chain-closed"].model_name, target_model_name());
    assert_eq!(
        after.chains["chain-control"].model_name,
        target_model_name()
    );
    assert_eq!(
        after.chains.len(),
        before.chains.len(),
        "chain delete detected"
    );
    assert_eq!(
        after.segments.len(),
        before.segments.len(),
        "segment delete detected"
    );
    assert_eq!(
        after.turns.len(),
        before.turns.len(),
        "turn delete detected"
    );
    assert_preserved_migrated_chain_timestamps(&before, &after);
    assert_preserved_non_owned_segment_fields(&before, &after);
    assert_four_population_segment_ownership(&after);
    assert_turn_consistency(&before, &after);
    let combined = combined_output(&output);
    assert!(combined.contains("live_db_mutated=yes"), "{combined}");
}

#[test]
fn apply_idempotence_second_apply_is_noop() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    let after_first_apply = snapshot(&fixture.state_path);

    let second = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&second);
    assert_eq!(snapshot(&fixture.state_path), after_first_apply);
    let report = read_report_from_output(&second);
    assert_report_count(&report, "chain_model_updates_to_apply", 0);
    assert_report_count(&report, "segment_provider_updates_to_apply", 0);
    assert_report_count(&report, "turn_provider_updates_to_apply", 0);
    assert_report_count(&report, "invocation_identity_updates_to_apply", 0);
}

#[test]
fn rollback_restores_preimage_values_after_apply() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let before = snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    assert_ne!(snapshot(&fixture.state_path), before);

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_eq!(snapshot(&fixture.state_path), before);
    let report = read_report_from_output(&rollback);
    assert!(
        normalize_report_text(&report).contains("restored"),
        "rollback report missing restored status: {report}"
    );
}

#[test]
fn apply_without_confirm_mutate_live_db_refuses_without_mutation() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let before = snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), false, false, false)
        .output()
        .unwrap();

    assert_failure(&output);
    let combined = combined_output(&output);
    assert!(combined.contains("--confirm-mutate-live-db"), "{combined}");
    assert_eq!(snapshot(&fixture.state_path), before);
    assert!(
        directory_is_empty(&backup_dir),
        "backup side effect before ack"
    );
}

#[test]
fn apply_without_backup_dir_refuses_without_mutation() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let before = snapshot(&fixture.state_path);

    let output = fixture
        .apply_command(None, true, false, false)
        .output()
        .unwrap();

    assert_failure(&output);
    let combined = combined_output(&output);
    assert!(combined.contains("--backup-dir"), "{combined}");
    assert_eq!(snapshot(&fixture.state_path), before);
}

#[test]
fn successful_apply_produces_backup_path_that_passes_quick_check() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let backup_path = output_path_value(&output, "backup");
    assert!(backup_path.is_absolute(), "backup path must be absolute");
    assert!(
        backup_path.starts_with(&backup_dir),
        "backup outside fixture"
    );
    assert_quick_check_ok(&backup_path);
}

#[test]
fn post_apply_counts_match_planned_and_zero_residual_old_owned_rows() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_live_counts_and_zero_residual(&fixture.state_path);
}

#[test]
fn larger_synthetic_fixture_exercises_multi_provider_candidate_path() {
    let fixture = Fixture::new();
    fixture.write_target_config();
    fixture.seed_base_population();
    let conn = fixture.conn();
    for index in 2..=6 {
        let chain_id = format!("chain-family-{index}");
        let session_id = format!("session-family-{index}");
        let turn_id = format!("turn-family-{index}");
        let account = unregistered_account_family(index);
        seed_chain(&conn, &chain_id, &source_model_name());
        seed_segment(
            &conn,
            &chain_id,
            &account,
            &session_id,
            None,
            Some(&turn_id),
            "manual",
        );
        seed_turn(&conn, &account, &session_id, &turn_id, "assistant");
        seed_mailbox(&fixture.mailbox_path, &session_id, None);
    }
    drop(conn);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    let after = snapshot(&fixture.state_path);
    for index in 2..=6 {
        assert_segment_owner(
            &after,
            &format!("chain-family-{index}"),
            &canonical_account(),
            &format!("session-family-{index}"),
        );
        let remapped_turn = after
            .turns
            .iter()
            .find(|turn| turn.session_id == format!("session-family-{index}"))
            .unwrap_or_else(|| panic!("missing family turn {index}"));
        assert_eq!(remapped_turn.provider_name, canonical_account());
    }
}

#[test]
fn s11_m2c_perf_scale_smoke_apply_and_rollback() {
    const SCALE_CHAIN_COUNT: usize = 1_000;
    const DUP_TURNS_PER_CHAIN: usize = 30;
    const UNIQUE_OLD_TURNS_PER_CHAIN: usize = 90;
    const EXPECTED_DEDUP_LOSERS: i64 = (SCALE_CHAIN_COUNT * DUP_TURNS_PER_CHAIN) as i64;

    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_scale_smoke_population(
        &fixture,
        SCALE_CHAIN_COUNT,
        DUP_TURNS_PER_CHAIN,
        UNIQUE_OLD_TURNS_PER_CHAIN,
    );
    let before = full_snapshot(&fixture.state_path);
    assert_eq!(before.chains.len(), SCALE_CHAIN_COUNT);
    assert_eq!(before.segments.len(), SCALE_CHAIN_COUNT * 2);
    assert_eq!(before.turns.len(), 150_000);
    let backup_dir = fixture.backup_dir();

    let apply = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&apply);
    assert_quick_check_ok(&fixture.state_path);
    let conn = Connection::open(&fixture.state_path).unwrap();
    assert_eq!(
        last_run_count(&conn, "turn_rows_deduped_away"),
        EXPECTED_DEDUP_LOSERS
    );
    assert!(
        last_run_count(&conn, "segment_rows_merged_away") >= SCALE_CHAIN_COUNT as i64,
        "not enough segment rows merged away"
    );
    assert_eq!(
        last_run_count(&conn, "post_apply_segment_collision_count"),
        0
    );
    assert_eq!(last_run_count(&conn, "post_apply_turn_collision_count"), 0);
    assert_zero_collision_counts(&conn);
    drop(conn);

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_quick_check_ok(&fixture.state_path);
    assert_eq!(full_snapshot(&fixture.state_path), before);
}

#[test]
fn provider_proof_passes_fails_and_skip_bypasses_only_the_proof_gate() {
    let proof_passes = Fixture::new();
    proof_passes.write_target_config();
    proof_passes.seed_base_population();
    assert_success(&proof_passes.run());

    let proof_fails = Fixture::new();
    proof_fails.write_target_config_with_failing_provider();
    proof_fails.seed_base_population();
    let before_failure = snapshot(&proof_fails.state_path);
    let failed = proof_fails.run();
    assert_failure(&failed);
    let combined = combined_output(&failed) + &read_report_if_present(&proof_fails.scratch_dir);
    assert!(
        combined.contains("proof") || combined.contains("provider"),
        "provider proof failure not visible: {combined}"
    );
    assert_eq!(snapshot(&proof_fails.state_path), before_failure);

    let skipped = Fixture::new();
    skipped.write_target_config_with_failing_provider();
    skipped.seed_base_population();
    let backup_dir = skipped.backup_dir();
    let output = skipped
        .apply_command(Some(&backup_dir), true, true, true)
        .output()
        .unwrap();
    assert_success(&output);
    let report = read_report_from_output(&output);
    assert!(
        normalize_report_text(&report).contains("provider proof")
            && normalize_report_text(&report).contains("skipped"),
        "skip was not recorded in report: {report}"
    );
}

#[test]
fn mode_exclusivity_and_skip_provider_proof_ack_refusals_do_not_mutate() {
    let exclusive = Fixture::new();
    exclusive.write_target_config();
    exclusive.seed_base_population();
    let before_exclusive = snapshot(&exclusive.state_path);
    let backup_dir = exclusive.backup_dir();
    let mut two_modes = exclusive.command();
    two_modes
        .arg("--apply")
        .arg("--backup-dir")
        .arg(&backup_dir)
        .arg("--confirm-mutate-live-db");
    let two_modes_output = two_modes.output().unwrap();
    assert_failure(&two_modes_output);
    let two_modes_text = combined_output(&two_modes_output);
    assert!(
        (two_modes_text.contains("only one") || two_modes_text.contains("exactly one"))
            && two_modes_text.contains("--dry-run")
            && two_modes_text.contains("--apply"),
        "mode exclusivity refusal missing: {two_modes_text}"
    );
    assert_eq!(snapshot(&exclusive.state_path), before_exclusive);

    let skip_ack = Fixture::new();
    skip_ack.write_target_config();
    skip_ack.seed_base_population();
    let before_skip_ack = snapshot(&skip_ack.state_path);
    let mut missing_skip_ack = skip_ack.command();
    missing_skip_ack.arg("--skip-provider-proof");
    let skip_ack_output = missing_skip_ack.output().unwrap();
    assert_failure(&skip_ack_output);
    let skip_ack_text = combined_output(&skip_ack_output);
    assert!(
        skip_ack_text.contains("--confirm-skip-provider-proof"),
        "skip ack refusal missing: {skip_ack_text}"
    );
    assert_eq!(snapshot(&skip_ack.state_path), before_skip_ack);
}

#[test]
fn s11_m2c_scope_preserves_valid_models_remaps_rotation_backfills_orphans() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    seed_s11_m2c_scope_population(&fixture, &models);
    let before = snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_quick_check_ok(&fixture.state_path);
    let after = snapshot(&fixture.state_path);
    assert_eq!(
        after.chains.len(),
        before.chains.len(),
        "chain delete detected"
    );
    assert_eq!(
        after.segments.len(),
        before.segments.len(),
        "segment delete detected"
    );
    assert_eq!(
        after.turns.len(),
        before.turns.len(),
        "turn delete detected"
    );

    assert_eq!(after.chains["chain-valid-mmm"].model_name, models.middle);
    assert_eq!(after.chains["chain-valid-zzz"].model_name, models.last);
    assert_eq!(after.chains["chain-target"].model_name, models.target);
    assert_eq!(after.chains["chain-orphan"].model_name, models.target);
    assert_eq!(after.chains["chain-orphan-rot"].model_name, models.target);
    assert_eq!(after.chains["chain-valid-rot"].model_name, models.middle);
    assert_eq!(
        after.chains["chain-gpt"].model_name,
        models.non_family_model
    );

    assert_segment_owner(
        &after,
        "chain-valid-mmm",
        &accepted_account(),
        "session-valid-mmm",
    );
    assert_segment_owner(
        &after,
        "chain-valid-zzz",
        &accepted_account(),
        "session-valid-zzz",
    );
    assert_segment_owner(
        &after,
        "chain-target",
        &accepted_account(),
        "session-target",
    );
    assert_segment_owner(
        &after,
        "chain-orphan",
        &canonical_account(),
        "session-orphan",
    );
    assert_segment_owner(
        &after,
        "chain-orphan-rot",
        &canonical_account(),
        "session-orphan-rot",
    );
    assert_segment_owner(
        &after,
        "chain-valid-rot",
        &canonical_account(),
        "session-valid-rot",
    );
    assert_segment_owner(
        &after,
        "chain-gpt",
        &models.non_family_account,
        "session-gpt",
    );
    assert_turn_owner(
        &after,
        "session-orphan-rot",
        "turn-orphan-rot",
        &canonical_account(),
    );
    assert_turn_owner(
        &after,
        "session-valid-rot",
        "turn-valid-rot",
        &canonical_account(),
    );
    assert_turn_owner(
        &after,
        "session-gpt",
        "turn-gpt",
        &models.non_family_account,
    );
    assert_preserved_chain_segment_session(
        &before,
        &after,
        "chain-orphan-rot",
        "session-orphan-rot",
    );
    assert_preserved_chain_segment_session(&before, &after, "chain-valid-rot", "session-valid-rot");
}

#[test]
fn s11_m2c_forward_apply_uses_per_chain_model_inference_and_preserves_real_invocations() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    fixture.write_s11_m2c_local_shadow_model();
    seed_s11_m2c_scope_population(&fixture, &models);
    {
        let conn = fixture.conn();
        seed_inference_orphan_chain(
            &conn,
            "chain-infer-middle",
            "session-infer-middle",
            &canonical_account(),
        );
        seed_invocation(
            &conn,
            "61000000-0000-4000-8000-000000000001",
            &models.middle,
            &canonical_account(),
            "session-infer-middle",
            "succeeded",
            "2026-06-20T10:01:00Z",
        );
        seed_invocation(
            &conn,
            "61000000-0000-4000-8000-000000000002",
            &models.middle,
            &canonical_account(),
            "session-infer-middle",
            "succeeded",
            "2026-06-20T10:02:00Z",
        );
        seed_invocation(
            &conn,
            "61000000-0000-4000-8000-000000000003",
            &models.last,
            &canonical_account(),
            "session-infer-middle",
            "succeeded",
            "2026-06-20T10:03:00Z",
        );
        seed_invocation(
            &conn,
            "61000000-0000-4000-8000-000000000004",
            "<unknown>",
            &canonical_account(),
            "session-infer-middle",
            "failed",
            "2026-06-20T10:04:00Z",
        );
        seed_invocation(
            &conn,
            "61000000-0000-4000-8000-000000000005",
            &source_model_name(),
            &canonical_account(),
            "session-infer-middle",
            "succeeded",
            "2026-06-20T10:05:00Z",
        );

        seed_inference_orphan_chain(
            &conn,
            "chain-infer-last",
            "session-infer-last",
            &unregistered_account_a(),
        );
        seed_invocation(
            &conn,
            "62000000-0000-4000-8000-000000000001",
            &models.last,
            &unregistered_account_a(),
            "session-infer-last",
            "succeeded",
            "2026-06-20T10:01:00Z",
        );
        seed_invocation(
            &conn,
            "62000000-0000-4000-8000-000000000002",
            &models.last,
            &unregistered_account_a(),
            "session-infer-last",
            "succeeded",
            "2026-06-20T10:02:00Z",
        );
        seed_invocation(
            &conn,
            "62000000-0000-4000-8000-000000000003",
            &models.middle,
            &unregistered_account_a(),
            "session-infer-last",
            "succeeded",
            "2026-06-20T10:03:00Z",
        );

        seed_inference_orphan_chain(
            &conn,
            "chain-infer-no-evidence",
            "session-infer-no-evidence",
            &canonical_account(),
        );
        seed_invocation(
            &conn,
            "63000000-0000-4000-8000-000000000001",
            "<unknown>",
            &canonical_account(),
            "session-infer-no-evidence",
            "failed",
            "2026-06-20T10:01:00Z",
        );
    }
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_quick_check_ok(&fixture.state_path);
    let after = snapshot(&fixture.state_path);
    assert_eq!(after.chains["chain-infer-middle"].model_name, models.middle);
    assert_eq!(after.chains["chain-infer-last"].model_name, models.last);
    assert_eq!(
        after.chains["chain-infer-no-evidence"].model_name,
        models.target
    );
    assert_eq!(after.chains["chain-valid-mmm"].model_name, models.middle);
    assert_eq!(
        after.chains["chain-gpt"].model_name,
        models.non_family_model
    );
    assert_chain_preimage_new_model(&fixture.state_path, "chain-infer-middle", &models.middle);
    assert_chain_preimage_new_model(&fixture.state_path, "chain-infer-last", &models.last);
    assert_chain_preimage_new_model(
        &fixture.state_path,
        "chain-infer-no-evidence",
        &models.target,
    );
    assert_invocation_models_and_provider_reconciled(
        &fixture,
        "session-infer-middle",
        &canonical_account(),
        &[
            ("61000000-0000-4000-8000-000000000001", &models.middle),
            ("61000000-0000-4000-8000-000000000002", &models.middle),
            ("61000000-0000-4000-8000-000000000003", &models.last),
            ("61000000-0000-4000-8000-000000000004", &models.middle),
            ("61000000-0000-4000-8000-000000000005", &models.middle),
        ],
    );
    assert_invocation_models_and_provider_reconciled(
        &fixture,
        "session-infer-last",
        &canonical_account(),
        &[
            ("62000000-0000-4000-8000-000000000001", &models.last),
            ("62000000-0000-4000-8000-000000000002", &models.last),
            ("62000000-0000-4000-8000-000000000003", &models.middle),
        ],
    );
}

#[test]
fn s11_m2c_corrective_apply_is_reversible_idempotent_and_reports_inferred_model() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    seed_corrective_primary_fixture(&fixture, &models);
    let user_version_before = pragma_user_version(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .corrective_apply_command(Some(&backup_dir), true)
        .output()
        .unwrap();

    assert_success(&output);
    assert_quick_check_ok(&fixture.state_path);
    assert_eq!(
        pragma_user_version(&fixture.state_path),
        user_version_before
    );
    let after = snapshot(&fixture.state_path);
    assert_eq!(
        after.chains["chain-corrective-middle"].model_name,
        models.middle
    );
    assert_eq!(
        after.chains["chain-corrective-original-real"].model_name,
        models.target
    );
    assert_eq!(
        after.chains["chain-corrective-no-different-evidence"].model_name,
        models.target
    );
    assert_eq!(
        after.chains["chain-corrective-non-family"].model_name,
        models.non_family_model
    );
    assert_eq!(corrective_preimage_row_count(&fixture.state_path), 1);
    assert_eq!(corrective_residual_default_count(&fixture.state_path), 0);
    let report = read_report_from_output(&output);
    assert_corrective_apply_report(&report, &models.middle, 1, 1);

    let second = fixture
        .corrective_apply_command(Some(&backup_dir), true)
        .output()
        .unwrap();

    assert_success(&second);
    assert_eq!(corrective_preimage_row_count(&fixture.state_path), 1);
    let second_report = read_report_from_output(&second);
    assert_corrective_apply_report(&second_report, &models.middle, 0, 0);

    let rollback = fixture.corrective_rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_quick_check_ok(&fixture.state_path);
    let after_rollback = snapshot(&fixture.state_path);
    assert_eq!(
        after_rollback.chains["chain-corrective-middle"].model_name,
        models.target
    );
    let rollback_report = read_report_from_output(&rollback);
    assert_corrective_rollback_report(&rollback_report, 1);
}

#[test]
fn s11_m2c_corrective_primary_uses_recorded_backfill_default_after_default_changed() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    let conn = fixture.conn();
    seed_corrective_chain(
        &conn,
        "chain-corrective-changed-default",
        "session-corrective-changed-default",
        &models.middle,
        "<unknown>",
        &models.middle,
        &canonical_account(),
    );
    seed_invocation(
        &conn,
        "75000000-0000-4000-8000-000000000001",
        &models.last,
        &canonical_account(),
        "session-corrective-changed-default",
        "succeeded",
        "2026-06-20T10:07:00Z",
    );
    drop(conn);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .corrective_apply_command(Some(&backup_dir), true)
        .output()
        .unwrap();

    assert_success(&output);
    let after = snapshot(&fixture.state_path);
    assert_eq!(
        after.chains["chain-corrective-changed-default"].model_name,
        models.last
    );
    assert_eq!(
        corrective_preimage_models(&fixture.state_path, "chain-corrective-changed-default"),
        (models.middle.clone(), models.last.clone())
    );

    let rollback = fixture.corrective_rollback_command(true).output().unwrap();

    assert_success(&rollback);
    let after_rollback = snapshot(&fixture.state_path);
    assert_eq!(
        after_rollback.chains["chain-corrective-changed-default"].model_name,
        models.middle
    );
    assert_ne!(
        after_rollback.chains["chain-corrective-changed-default"].model_name,
        models.target
    );
}

#[test]
fn s11_m2c_corrective_transcript_fallback_retargets_without_overriding_db_evidence() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs_with_transcript_storage();
    let conn = fixture.conn();
    seed_corrective_chain(
        &conn,
        "chain-corrective-transcript-middle",
        "session-corrective-transcript-middle",
        &models.target,
        "<unknown>",
        &models.target,
        &canonical_account(),
    );
    seed_invocation(
        &conn,
        "76000000-0000-4000-8000-000000000001",
        &models.target,
        &canonical_account(),
        "session-corrective-transcript-middle",
        "succeeded",
        "2026-06-20T10:01:00Z",
    );
    seed_invocation(
        &conn,
        "76000000-0000-4000-8000-000000000002",
        "<unknown>",
        &canonical_account(),
        "session-corrective-transcript-middle",
        "failed",
        "2026-06-20T10:02:00Z",
    );

    seed_corrective_chain(
        &conn,
        "chain-corrective-db-last",
        "session-corrective-db-last",
        &models.target,
        "<unknown>",
        &models.target,
        &canonical_account(),
    );
    seed_invocation(
        &conn,
        "77000000-0000-4000-8000-000000000001",
        &models.last,
        &canonical_account(),
        "session-corrective-db-last",
        "succeeded",
        "2026-06-20T10:03:00Z",
    );

    seed_corrective_chain(
        &conn,
        "chain-corrective-transcript-default-only",
        "session-corrective-transcript-default-only",
        &models.target,
        "<unknown>",
        &models.target,
        &canonical_account(),
    );
    drop(conn);

    write_synthetic_transcript(
        &fixture,
        "session-corrective-transcript-middle",
        &[
            models.middle.as_str(),
            models.middle.as_str(),
            models.middle.as_str(),
            models.target.as_str(),
        ],
    );
    write_synthetic_transcript(
        &fixture,
        "session-corrective-db-last",
        &[
            models.middle.as_str(),
            models.middle.as_str(),
            models.middle.as_str(),
        ],
    );
    write_synthetic_transcript(
        &fixture,
        "session-corrective-transcript-default-only",
        &[models.target.as_str(), "<synthetic>"],
    );
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .corrective_apply_command(Some(&backup_dir), true)
        .output()
        .unwrap();

    assert_success(&output);
    assert_quick_check_ok(&fixture.state_path);
    let after = snapshot(&fixture.state_path);
    assert_eq!(
        after.chains["chain-corrective-transcript-middle"].model_name,
        models.middle
    );
    assert_eq!(
        after.chains["chain-corrective-db-last"].model_name,
        models.last
    );
    assert_eq!(
        after.chains["chain-corrective-transcript-default-only"].model_name,
        models.target
    );
    assert_eq!(corrective_preimage_row_count(&fixture.state_path), 2);
    assert_eq!(
        corrective_preimage_models(&fixture.state_path, "chain-corrective-transcript-middle"),
        (models.target.clone(), models.middle.clone())
    );
    assert_eq!(
        corrective_preimage_evidence_source(
            &fixture.state_path,
            "chain-corrective-transcript-middle"
        ),
        "transcript"
    );
    assert_eq!(
        corrective_preimage_evidence_source(&fixture.state_path, "chain-corrective-db-last"),
        "original-preimage-db-evidence"
    );
    let report = read_report_from_output(&output);
    assert_report_count(&report, "corrective_chain_model_updates_to_apply", 2);
    assert_report_count(&report, "corrective_chain_model_updates_applied", 2);
    assert_report_count(&report, "evidence_source transcript", 1);
    assert_report_count(&report, "evidence_source original-preimage-db-evidence", 1);

    let second = fixture
        .corrective_apply_command(Some(&backup_dir), true)
        .output()
        .unwrap();

    assert_success(&second);
    let second_report = read_report_from_output(&second);
    assert_report_count(&second_report, "corrective_chain_model_updates_to_apply", 0);
    assert_report_count(&second_report, "corrective_chain_model_updates_applied", 0);
    assert_eq!(corrective_preimage_row_count(&fixture.state_path), 2);

    let rollback = fixture.corrective_rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_quick_check_ok(&fixture.state_path);
    let after_rollback = snapshot(&fixture.state_path);
    assert_eq!(
        after_rollback.chains["chain-corrective-transcript-middle"].model_name,
        models.target
    );
    assert_eq!(
        after_rollback.chains["chain-corrective-db-last"].model_name,
        models.target
    );
    let rollback_report = read_report_from_output(&rollback);
    assert_corrective_rollback_report(&rollback_report, 2);
}

#[test]
fn s11_m2c_corrective_preimage_absent_fallback_requires_single_different_inventory_model() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    seed_corrective_fallback_fixture(&fixture, &models);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .corrective_apply_command(Some(&backup_dir), true)
        .output()
        .unwrap();

    assert_success(&output);
    let after = snapshot(&fixture.state_path);
    assert_eq!(
        after.chains["chain-fallback-single"].model_name,
        models.middle
    );
    assert_eq!(
        after.chains["chain-fallback-none"].model_name,
        models.target
    );
    assert_eq!(
        after.chains["chain-fallback-conflicting"].model_name,
        models.target
    );
    assert_eq!(
        after.chains["chain-fallback-out-of-inventory"].model_name,
        models.target
    );
    let report = read_report_from_output(&output);
    assert_corrective_apply_report(&report, &models.middle, 1, 1);
}

#[test]
fn s11_m2c_corrective_dry_run_proves_on_copy_without_live_mutation() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    seed_corrective_primary_fixture(&fixture, &models);
    let live_before = snapshot(&fixture.state_path);
    let hash_before = file_hash(&fixture.state_path);

    let output = fixture.corrective_dry_run_command().output().unwrap();

    assert_success(&output);
    assert_eq!(file_hash(&fixture.state_path), hash_before);
    assert_eq!(snapshot(&fixture.state_path), live_before);
    let state_copy = find_artifact(&fixture.scratch_dir, "state-copy.db");
    let rollback_copy = find_artifact(&fixture.scratch_dir, "rollback-copy.db");
    let copy_after = snapshot(&state_copy);
    assert_eq!(
        copy_after.chains["chain-corrective-middle"].model_name,
        models.middle
    );
    let rollback_after = snapshot(&rollback_copy);
    assert_eq!(
        rollback_after.chains["chain-corrective-middle"].model_name,
        models.target
    );
    let report = read_report(&fixture.scratch_dir);
    assert!(
        report.contains("s11_m2c_model_corrective_preimage"),
        "report missing corrective preimage table: {report}"
    );
    assert!(
        report.contains(&models.middle),
        "report missing grouped inferred model: {report}"
    );
    assert_report_context_count(
        &report,
        &FIRST_FORWARD_PHASE,
        "corrective_chain_model_updates_to_apply",
        1,
    );
    assert_report_context_count(
        &report,
        &AFTER_IDEMPOTENCE_PHASE,
        "corrective_chain_model_updates_to_apply",
        0,
    );
    assert_report_context_count(
        &report,
        &ROLLBACK_COPY_PHASE,
        "corrective_chain_model_rollback_mismatches",
        0,
    );
}

#[test]
fn s11_m2c_corrective_live_mutation_gates_refuse_without_backup_or_confirmation() {
    let missing_confirm = Fixture::new();
    let models = missing_confirm.write_s11_m2c_scope_configs();
    seed_corrective_primary_fixture(&missing_confirm, &models);
    let before_missing_confirm = snapshot(&missing_confirm.state_path);
    let backup_dir = missing_confirm.backup_dir();

    let output = missing_confirm
        .corrective_apply_command(Some(&backup_dir), false)
        .output()
        .unwrap();

    assert_failure(&output);
    assert!(combined_output(&output).contains("--confirm-mutate-live-db"));
    assert_eq!(
        snapshot(&missing_confirm.state_path),
        before_missing_confirm
    );

    let missing_backup = Fixture::new();
    let models = missing_backup.write_s11_m2c_scope_configs();
    seed_corrective_primary_fixture(&missing_backup, &models);
    let before_missing_backup = snapshot(&missing_backup.state_path);

    let output = missing_backup
        .corrective_apply_command(None, true)
        .output()
        .unwrap();

    assert_failure(&output);
    assert!(combined_output(&output).contains("--backup-dir"));
    assert_eq!(snapshot(&missing_backup.state_path), before_missing_backup);

    let rollback_without_confirm = Fixture::new();
    let models = rollback_without_confirm.write_s11_m2c_scope_configs();
    seed_corrective_primary_fixture(&rollback_without_confirm, &models);
    let backup_dir = rollback_without_confirm.backup_dir();
    assert_success(
        &rollback_without_confirm
            .corrective_apply_command(Some(&backup_dir), true)
            .output()
            .unwrap(),
    );
    let before_rollback_refusal = snapshot(&rollback_without_confirm.state_path);

    let output = rollback_without_confirm
        .corrective_rollback_command(false)
        .output()
        .unwrap();

    assert_failure(&output);
    assert!(combined_output(&output).contains("--confirm-mutate-live-db"));
    assert_eq!(
        snapshot(&rollback_without_confirm.state_path),
        before_rollback_refusal
    );
}

#[test]
fn s11_m2c_corrective_rollback_drift_fails_closed() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    seed_corrective_primary_fixture(&fixture, &models);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .corrective_apply_command(Some(&backup_dir), true)
            .output()
            .unwrap(),
    );
    fixture
        .conn()
        .execute(
            "UPDATE session_chains SET model_name = ?1 WHERE chain_id = 'chain-corrective-middle'",
            [&models.last],
        )
        .unwrap();

    let rollback = fixture.corrective_rollback_command(true).output().unwrap();

    assert_failure(&rollback);
    let combined = combined_output(&rollback) + &read_report_if_present(&fixture.scratch_dir);
    assert!(
        combined.contains("drift"),
        "rollback drift missing: {combined}"
    );
    assert_eq!(
        snapshot(&fixture.state_path).chains["chain-corrective-middle"].model_name,
        models.last
    );
    assert_eq!(corrective_preimage_row_count(&fixture.state_path), 1);
}

#[test]
fn s11_m2c_scope_valid_chain_merge_rolls_back_precisely() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    let conn = fixture.conn();
    seed_chain(&conn, "chain-valid-merge", &models.middle);
    seed_segment_with_started_at(
        &conn,
        "chain-valid-merge",
        &canonical_account(),
        "session-valid-merge",
        "2026-06-20T10:00:00Z",
        Some("2026-06-20T10:04:00Z"),
        Some("turn-valid-merge-r"),
        "initial",
    );
    seed_segment_with_started_at(
        &conn,
        "chain-valid-merge",
        &unregistered_account_a(),
        "session-valid-merge",
        "2026-06-20T10:05:00Z",
        None,
        Some("turn-valid-merge-t"),
        "manual",
    );
    seed_turn(
        &conn,
        &canonical_account(),
        "session-valid-merge",
        "turn-valid-merge-r",
        "assistant",
    );
    seed_turn(
        &conn,
        &unregistered_account_a(),
        "session-valid-merge",
        "turn-valid-merge-t",
        "user",
    );
    drop(conn);
    let before = snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_quick_check_ok(&fixture.state_path);
    let after = snapshot(&fixture.state_path);
    assert_eq!(after.chains["chain-valid-merge"].model_name, models.middle);
    let merged_segments: Vec<_> = after
        .segments
        .iter()
        .filter(|segment| {
            segment.chain_id == "chain-valid-merge"
                && segment.provider_name == canonical_account()
                && segment.session_id == "session-valid-merge"
        })
        .collect();
    assert_eq!(
        merged_segments.len(),
        1,
        "valid-chain remap collision was not merged: {merged_segments:?}"
    );

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_quick_check_ok(&fixture.state_path);
    assert_eq!(snapshot(&fixture.state_path), before);
}

#[test]
fn s11_m2c_scope_reapply_is_clean_noop_with_valid_models() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    seed_s11_m2c_scope_population(&fixture, &models);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    let after_first_apply = snapshot(&fixture.state_path);

    let second = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&second);
    assert_quick_check_ok(&fixture.state_path);
    assert_eq!(snapshot(&fixture.state_path), after_first_apply);
    let report = read_report_from_output(&second);
    assert_report_count(&report, "chain_model_updates_to_apply", 0);
    assert_report_count(&report, "segment_provider_updates_to_apply", 0);
    assert_report_count(&report, "turn_provider_updates_to_apply", 0);
}

#[test]
fn s11_m2c_resume_backfilled_orphan_reconciles_invocations_and_resolves_provider_ref() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    fixture.write_s11_m2c_local_shadow_model();
    seed_resume_orphaned_chain_with_shadow_invocations(&fixture);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_provider_ref_resume(
        &fixture,
        RESUME_CHAIN_ORPHAN,
        &models.target,
        &canonical_account(),
    );
    assert_invocations_reconciled(
        &fixture,
        "session-resume-orphan",
        &models.target,
        &canonical_account(),
    );
    assert_no_external_identity_errors(&output);
}

#[test]
fn s11_m2c_resume_rotation_remapped_chain_reconciles_invocations_and_resolves_provider_ref() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    fixture.write_s11_m2c_local_shadow_model();
    seed_resume_rotation_chain_with_shadow_invocation(&fixture);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_provider_ref_resume(
        &fixture,
        RESUME_CHAIN_ROTATION,
        &models.target,
        &canonical_account(),
    );
    assert_invocations_reconciled(
        &fixture,
        "session-resume-rotation",
        &models.target,
        &canonical_account(),
    );
    assert_no_external_identity_errors(&output);
}

#[test]
fn s11_m2c_resume_valid_provider_ref_chain_preserves_aligned_invocations() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    seed_resume_valid_chain_with_aligned_invocation(&fixture, &models);
    let before_chain_model = snapshot(&fixture.state_path)
        .chains
        .get(RESUME_CHAIN_VALID)
        .unwrap()
        .model_name
        .clone();
    assert_eq!(before_chain_model, models.middle);
    let before = invocation_snapshot_for_session(&fixture.state_path, "session-resume-valid");
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_provider_ref_resume(
        &fixture,
        RESUME_CHAIN_VALID,
        &models.middle,
        &accepted_account(),
    );
    let after_chain_model = snapshot(&fixture.state_path)
        .chains
        .get(RESUME_CHAIN_VALID)
        .unwrap()
        .model_name
        .clone();
    assert_eq!(after_chain_model, before_chain_model);
    assert_eq!(after_chain_model, models.middle);
    assert_invocations_preserved(&fixture, "session-resume-valid", &before);
    let report = read_report_from_output(&output);
    assert_report_count(&report, "invocation_identity_updates_to_apply", 0);
    assert_no_external_identity_errors(&output);
}

#[test]
fn s11_m2c_resume_no_payload_repl_skips_default_provider_ref_migration() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    fixture.rewrite_scope_provider_entries_with_resume();
    seed_resume_repl_orphaned_provider_ref_chain(&fixture);
    let backup_dir = fixture.backup_dir();
    let apply = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();
    assert_success(&apply);
    assert_provider_ref_resume(
        &fixture,
        RESUME_CHAIN_REPL,
        &models.target,
        &canonical_account(),
    );
    fixture.copy_state_to_default_runtime_path();

    let output = fixture
        .repl_resume_command(RESUME_CHAIN_REPL)
        .output()
        .unwrap();

    assert_no_external_identity_errors(&output);
    assert_success(&output);
}

#[test]
fn s11_m2c_provider_ref_repl_launches_existing_session_unbounded() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    let projects_dir = fixture.dir.path().join("repl-provider-projects");
    let argv_path = fixture.dir.path().join("repl-bound-argv.txt");
    let pwd_path = fixture.dir.path().join("repl-bound-pwd.txt");
    let resolved_launch_cwd = fixture.dir.path().join("resolved-repl-launch-cwd");
    fs::create_dir_all(&resolved_launch_cwd).unwrap();
    let recorder = fixture.recording_script_with_pwd("repl-bound-provider", &argv_path, &pwd_path);
    fixture.rewrite_scope_provider_entries_with_resume_storage(&recorder, &projects_dir);
    seed_provider_ref_boundary_resume_chain(&fixture, &models.target);
    let (source_path, _boundary_line, _pre_boundary_line, _post_boundary_line) =
        stage_provider_ref_boundary_jsonl(
            &projects_dir,
            &resolved_launch_cwd,
            "session-resume-repl",
        );
    let transcript_dir = projects_dir.join(claude_project_dir_name(&resolved_launch_cwd));
    let original_source = fs::read(&source_path).unwrap();
    let before_files = transcript_file_snapshot(&transcript_dir);
    seed_mailbox(
        &fixture.default_runtime_mailbox_path(),
        "session-resume-repl",
        Some(resolved_launch_cwd.to_str().unwrap()),
    );
    let runtime_state = fixture.copy_state_to_default_runtime_path();
    let before_segments = full_snapshot(&runtime_state).segments;

    let output = fixture
        .repl_resume_command(RESUME_CHAIN_REPL)
        .output()
        .unwrap();

    assert_no_external_identity_errors(&output);
    assert_success(&output);
    let argv = fs::read_to_string(&argv_path).unwrap();
    let argv_lines: Vec<_> = argv.lines().collect();
    let resume_flag = argv_lines
        .iter()
        .position(|line| *line == "--resume")
        .unwrap_or_else(|| panic!("REPL launch did not receive configured resume flag: {argv}"));
    assert_eq!(
        argv_lines.get(resume_flag + 1).copied(),
        Some("session-resume-repl"),
        "provider-ref REPL resume must pass the existing provider session id unbounded"
    );
    assert_eq!(
        fs::read_to_string(&pwd_path).unwrap().trim(),
        fixture.dir.path().to_string_lossy().as_ref(),
        "provider-ref REPL skip must not compute a migration/bounding launch cwd"
    );
    assert_eq!(fs::read(&source_path).unwrap(), original_source);
    assert_eq!(
        transcript_file_snapshot(&transcript_dir),
        before_files,
        "provider-ref REPL resume must not create a fresh JSONL or temp file"
    );
    assert_eq!(
        full_snapshot(&runtime_state).segments,
        before_segments,
        "provider-ref REPL resume must not close/open chain segments"
    );
}

#[test]
fn s11_m2c_provider_ref_repl_no_boundary_launches_original_session() {
    assert_repl_non_rotated_provider_ref_resume(ProviderRefNonRotatedCase::NoBoundary);
}

#[test]
fn s11_m2c_provider_ref_repl_boundary_not_found_launches_original_session() {
    assert_repl_non_rotated_provider_ref_resume(ProviderRefNonRotatedCase::BoundaryNotFound);
}

#[test]
fn s11_m2c_provider_ref_repl_already_bounded_launches_original_session() {
    assert_repl_non_rotated_provider_ref_resume(ProviderRefNonRotatedCase::AlreadyBounded);
}

fn assert_repl_non_rotated_provider_ref_resume(case: ProviderRefNonRotatedCase) {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    let projects_dir = fixture
        .dir
        .path()
        .join(format!("repl-{}-projects", case.label()));
    let argv_path = fixture
        .dir
        .path()
        .join(format!("repl-{}-argv.txt", case.label()));
    let pwd_path = fixture
        .dir
        .path()
        .join(format!("repl-{}-pwd.txt", case.label()));
    let resolved_launch_cwd = fixture
        .dir
        .path()
        .join(format!("resolved-repl-{}-cwd", case.label()));
    fs::create_dir_all(&resolved_launch_cwd).unwrap();
    let recorder = fixture.recording_script_with_pwd(
        &format!("repl-{}-provider", case.label()),
        &argv_path,
        &pwd_path,
    );
    fixture.rewrite_scope_provider_entries_with_resume_storage(&recorder, &projects_dir);
    let source_path = match case {
        ProviderRefNonRotatedCase::NoBoundary => {
            seed_provider_ref_no_boundary_resume_chain(&fixture, &models.target);
            stage_provider_ref_jsonl_without_boundary(
                &projects_dir,
                &resolved_launch_cwd,
                "session-resume-repl",
            )
        }
        ProviderRefNonRotatedCase::BoundaryNotFound => {
            seed_provider_ref_boundary_resume_chain(&fixture, &models.target);
            stage_provider_ref_jsonl_without_boundary(
                &projects_dir,
                &resolved_launch_cwd,
                "session-resume-repl",
            )
        }
        ProviderRefNonRotatedCase::AlreadyBounded => {
            seed_provider_ref_boundary_resume_chain(&fixture, &models.target);
            stage_provider_ref_boundary_at_head_jsonl(
                &projects_dir,
                &resolved_launch_cwd,
                "session-resume-repl",
            )
        }
    };
    let transcript_dir = projects_dir.join(claude_project_dir_name(&resolved_launch_cwd));
    let original_source = fs::read(&source_path).unwrap();
    let before_files = transcript_file_snapshot(&transcript_dir);
    seed_mailbox(
        &fixture.default_runtime_mailbox_path(),
        "session-resume-repl",
        Some(resolved_launch_cwd.to_str().unwrap()),
    );
    let runtime_state = fixture.copy_state_to_default_runtime_path();
    let before_segments = full_snapshot(&runtime_state).segments;

    let output = fixture
        .repl_resume_command(RESUME_CHAIN_REPL)
        .output()
        .unwrap();

    assert_no_external_identity_errors(&output);
    assert_success(&output);
    let argv = fs::read_to_string(&argv_path).unwrap();
    let argv_lines: Vec<_> = argv.lines().collect();
    let resume_flag = argv_lines
        .iter()
        .position(|line| *line == "--resume")
        .unwrap_or_else(|| panic!("REPL launch did not receive configured resume flag: {argv}"));
    assert_eq!(
        argv_lines.get(resume_flag + 1).copied(),
        Some("session-resume-repl"),
        "non-rotated provider-ref REPL resume must keep the original provider session id for {case:?}"
    );
    assert_eq!(
        fs::read_to_string(&pwd_path).unwrap().trim(),
        fixture.dir.path().to_string_lossy().as_ref(),
        "non-rotated provider-ref REPL resume must keep the pre-resume2 skip cwd for {case:?}"
    );
    assert_eq!(fs::read(&source_path).unwrap(), original_source);
    assert_eq!(
        transcript_file_snapshot(&transcript_dir),
        before_files,
        "non-rotated provider-ref REPL resume must not create a fresh JSONL or temp file for {case:?}"
    );
    assert_eq!(
        full_snapshot(&runtime_state).segments,
        before_segments,
        "non-rotated provider-ref REPL resume must not close/open chain segments for {case:?}"
    );
}

#[test]
fn s11_m2c_provider_ref_repl_model_provider_mismatch_rejects_before_default_migration() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    let projects_dir = fixture.dir.path().join("repl-provider-projects");
    let argv_path = fixture.dir.path().join("repl-mismatch-argv.txt");
    let recorder = fixture.recording_script("repl-mismatch-provider", &argv_path);
    fixture.rewrite_scope_provider_entries_with_resume_storage(&recorder, &projects_dir);
    fixture.append_provider_entry(
        &models.non_family_account,
        &fixture.success_script("repl-mismatch-non-family-provider"),
    );
    seed_provider_ref_boundary_resume_chain(&fixture, &models.target);
    let (source_path, _boundary_line, _pre_boundary_line, _post_boundary_line) =
        stage_provider_ref_boundary_jsonl(&projects_dir, fixture.dir.path(), "session-resume-repl");
    let transcript_dir = projects_dir.join(claude_project_dir_name(fixture.dir.path()));
    let original_source = fs::read(&source_path).unwrap();
    let before_files = transcript_file_snapshot(&transcript_dir);
    let runtime_state = fixture.copy_state_to_default_runtime_path();
    let before_state = full_snapshot(&runtime_state);

    let output = fixture
        .repl_resume_command_with_model(RESUME_CHAIN_REPL, Some(&models.non_family_model))
        .output()
        .unwrap();

    assert_failure(&output);
    let combined = combined_output(&output);
    assert!(
        combined.contains(&format!(
            "session {RESUME_CHAIN_REPL} belongs to provider {}, which is not in model {}'s provider pool",
            canonical_account(),
            models.non_family_model
        )),
        "{combined}"
    );
    assert_no_external_identity_errors(&output);
    assert!(
        !argv_path.exists(),
        "provider command must not launch after provider-ref mismatch: {}",
        combined
    );
    assert_eq!(fs::read(&source_path).unwrap(), original_source);
    assert_eq!(transcript_file_snapshot(&transcript_dir), before_files);
    assert_eq!(full_snapshot(&runtime_state), before_state);
}

#[test]
fn s11_m2c_resume_invocation_preimage_rolls_back_exactly() {
    let fixture = Fixture::new();
    fixture.write_s11_m2c_scope_configs();
    fixture.write_s11_m2c_local_shadow_model();
    seed_resume_orphaned_chain_with_shadow_invocations(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert!(
        invocation_preimage_count(&fixture.state_path) > 0,
        "forward migration did not record invocation preimage rows"
    );

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_eq!(full_snapshot(&fixture.state_path), before);
    let report = read_report_from_output(&rollback);
    assert_report_context_count(&report, &ROLLBACK_COPY_PHASE, "invocation_mismatch", 0);
}

#[test]
fn s11_m2c_resume_zero_residual_includes_migrated_invocations() {
    let fixture = Fixture::new();
    fixture.write_s11_m2c_scope_configs();
    fixture.write_s11_m2c_local_shadow_model();
    seed_resume_orphaned_chain_with_shadow_invocations(&fixture);
    seed_resume_rotation_chain_with_shadow_invocation(&fixture);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_live_counts_and_zero_residual(&fixture.state_path);
    assert_eq!(
        residual_noncanonical_rotation_invocation_owner_count(&fixture.state_path),
        0
    );
    assert_eq!(
        residual_stale_invocation_model_count(&fixture.state_path),
        0
    );
}

#[test]
fn s11_m2c_resume_invocation_updates_are_idempotent_and_reported() {
    let fixture = Fixture::new();
    fixture.write_s11_m2c_scope_configs();
    fixture.write_s11_m2c_local_shadow_model();
    seed_resume_orphaned_chain_with_shadow_invocations(&fixture);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    let after_first = full_snapshot(&fixture.state_path);

    let second = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&second);
    assert_eq!(full_snapshot(&fixture.state_path), after_first);
    let report = read_report_from_output(&second);
    assert_report_count(&report, "invocation_identity_updates_to_apply", 0);
}

#[test]
fn s11_m2c_resume_stale_invocation_preimage_reapply_reconciles_and_counts() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    let invocation_id = seed_resume_reapply_reconciliation_fixture(&fixture, &models);
    let stale_provider = unregistered_account_family(2);
    {
        let conn = Connection::open(&fixture.state_path).unwrap();
        seed_stale_invocation_preimage_row(&conn, invocation_id, &models.middle, &stale_provider);
    }
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_provider_ref_resume(
        &fixture,
        RESUME_CHAIN_STALE_PREIMAGE,
        &models.middle,
        &canonical_account(),
    );
    assert_invocations_reconciled(
        &fixture,
        RESUME_STALE_PREIMAGE_SESSION,
        &models.middle,
        &canonical_account(),
    );
    assert_eq!(
        invocation_preimage_count_for_identity(
            &fixture.state_path,
            invocation_id,
            &models.middle,
            &models.middle,
            &stale_provider,
            &canonical_account(),
        ),
        1,
        "fresh non-no-op invocation preimage row was not recorded"
    );
    let conn = Connection::open(&fixture.state_path).unwrap();
    let planned = last_run_count(&conn, "invocation_identity_updates_to_apply");
    let applied = applied_invocation_count(&conn);
    assert_eq!(planned, 1);
    assert_eq!(applied, planned);
    assert_no_external_identity_errors(&output);
}

#[test]
fn s11_m2c_resume_clean_double_apply_invocation_reconciliation_is_noop() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    seed_resume_reapply_reconciliation_fixture(&fixture, &models);
    let backup_dir = fixture.backup_dir();
    let first = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();
    assert_success(&first);
    assert_invocations_reconciled(
        &fixture,
        RESUME_STALE_PREIMAGE_SESSION,
        &models.middle,
        &canonical_account(),
    );
    let after_first = full_snapshot(&fixture.state_path);

    let second = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&second);
    assert_eq!(full_snapshot(&fixture.state_path), after_first);
    let report = read_report_from_output(&second);
    assert_report_count(&report, "invocation_identity_updates_to_apply", 0);
}

#[test]
fn s11_m2c_resume_stale_check_preimage_reapply_reconciles_and_rebuilds() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    let invocation_id = seed_resume_reapply_reconciliation_fixture(&fixture, &models);
    let stale_provider = unregistered_account_family(2);
    {
        let conn = Connection::open(&fixture.state_path).unwrap();
        seed_old_check_invocation_preimage_table(&conn);
    }
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_success(&output);
    assert_invocations_reconciled(
        &fixture,
        RESUME_STALE_PREIMAGE_SESSION,
        &models.middle,
        &canonical_account(),
    );
    assert_eq!(
        invocation_preimage_count_for_identity(
            &fixture.state_path,
            invocation_id,
            &models.middle,
            &models.middle,
            &stale_provider,
            &canonical_account(),
        ),
        1,
        "rebuilt preimage table did not record an invocation row"
    );
    let conn = Connection::open(&fixture.state_path).unwrap();
    let planned = last_run_count(&conn, "invocation_identity_updates_to_apply");
    let applied = applied_invocation_count(&conn);
    assert_eq!(planned, 1);
    assert_eq!(applied, planned);
    assert_no_external_identity_errors(&output);
    let preimage_sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 's11_wu4_restore_session_ownership_preimage'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        preimage_sql.contains("'invocation'"),
        "rebuilt preimage CHECK does not admit invocation rows: {preimage_sql}"
    );
}

#[test]
fn s11_m2c_resume_rollback_drops_preimage_table() {
    let fixture = Fixture::new();
    let models = fixture.write_s11_m2c_scope_configs();
    seed_resume_reapply_reconciliation_fixture(&fixture, &models);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_success(&rollback);
    assert_eq!(full_snapshot(&fixture.state_path), before);
    let conn = Connection::open(&fixture.state_path).unwrap();
    assert!(
        !table_exists(&conn, "s11_wu4_restore_session_ownership_preimage"),
        "rollback should drop the session-ownership preimage table after verified restore"
    );
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn live_apply_and_rollback_commands_are_guarded_to_temp_state_db_and_redirected_env() {
    let fixture = Fixture::new();
    let backup_dir = fixture.backup_dir();
    let apply = fixture.apply_command(Some(&backup_dir), true, false, false);
    assert_live_command_guard(&apply, &fixture);
    let rollback = fixture.rollback_command(true);
    assert_live_command_guard(&rollback, &fixture);
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

fn provider_token_number(index: u8) -> String {
    ["cla", "ude", &index.to_string()].concat()
}

fn unregistered_account_family(index: u8) -> String {
    format!("acct-unreg-{}", provider_token_number(index))
}

fn seed_s11_m2c_scope_population(fixture: &Fixture, models: &S11M2cScopeModels) {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-valid-mmm", &models.middle);
    seed_segment(
        &conn,
        "chain-valid-mmm",
        &accepted_account(),
        "session-valid-mmm",
        None,
        Some("turn-valid-mmm"),
        "initial",
    );
    seed_turn(
        &conn,
        &accepted_account(),
        "session-valid-mmm",
        "turn-valid-mmm",
        "assistant",
    );

    seed_chain(&conn, "chain-valid-zzz", &models.last);
    seed_segment(
        &conn,
        "chain-valid-zzz",
        &accepted_account(),
        "session-valid-zzz",
        None,
        Some("turn-valid-zzz"),
        "initial",
    );
    seed_turn(
        &conn,
        &accepted_account(),
        "session-valid-zzz",
        "turn-valid-zzz",
        "assistant",
    );

    seed_chain(&conn, "chain-target", &models.target);
    seed_segment(
        &conn,
        "chain-target",
        &accepted_account(),
        "session-target",
        None,
        Some("turn-target"),
        "initial",
    );
    seed_turn(
        &conn,
        &accepted_account(),
        "session-target",
        "turn-target",
        "assistant",
    );

    seed_chain(&conn, "chain-orphan", "<unknown>");
    seed_segment(
        &conn,
        "chain-orphan",
        &canonical_account(),
        "session-orphan",
        None,
        Some("turn-orphan"),
        "initial",
    );
    seed_turn(
        &conn,
        &canonical_account(),
        "session-orphan",
        "turn-orphan",
        "assistant",
    );

    seed_chain(&conn, "chain-orphan-rot", "<unknown>");
    seed_segment(
        &conn,
        "chain-orphan-rot",
        &unregistered_account_a(),
        "session-orphan-rot",
        None,
        Some("turn-orphan-rot"),
        "manual",
    );
    seed_turn(
        &conn,
        &unregistered_account_a(),
        "session-orphan-rot",
        "turn-orphan-rot",
        "user",
    );

    seed_chain(&conn, "chain-valid-rot", &models.middle);
    seed_segment(
        &conn,
        "chain-valid-rot",
        &unregistered_account_a(),
        "session-valid-rot",
        None,
        Some("turn-valid-rot"),
        "manual",
    );
    seed_turn(
        &conn,
        &unregistered_account_a(),
        "session-valid-rot",
        "turn-valid-rot",
        "user",
    );

    seed_chain(&conn, "chain-gpt", &models.non_family_model);
    seed_segment(
        &conn,
        "chain-gpt",
        &models.non_family_account,
        "session-gpt",
        None,
        Some("turn-gpt"),
        "initial",
    );
    seed_turn(
        &conn,
        &models.non_family_account,
        "session-gpt",
        "turn-gpt",
        "assistant",
    );
}

fn seed_inference_orphan_chain(
    conn: &Connection,
    chain_id: &str,
    session_id: &str,
    provider_name: &str,
) {
    let turn_id = format!("turn-{session_id}");
    seed_chain(conn, chain_id, "<unknown>");
    seed_segment(
        conn,
        chain_id,
        provider_name,
        session_id,
        None,
        Some(&turn_id),
        "initial",
    );
    seed_turn(conn, provider_name, session_id, &turn_id, "assistant");
}

fn seed_corrective_primary_fixture(fixture: &Fixture, models: &S11M2cScopeModels) {
    let conn = fixture.conn();
    seed_corrective_chain(
        &conn,
        "chain-corrective-middle",
        "session-corrective-middle",
        &models.target,
        "<unknown>",
        &models.target,
        &canonical_account(),
    );
    seed_invocation(
        &conn,
        "71000000-0000-4000-8000-000000000001",
        &models.middle,
        &canonical_account(),
        "session-corrective-middle",
        "succeeded",
        "2026-06-20T10:01:00Z",
    );
    seed_invocation(
        &conn,
        "71000000-0000-4000-8000-000000000002",
        &models.middle,
        &canonical_account(),
        "session-corrective-middle",
        "succeeded",
        "2026-06-20T10:02:00Z",
    );

    seed_corrective_chain(
        &conn,
        "chain-corrective-original-real",
        "session-corrective-original-real",
        &models.target,
        &models.middle,
        &models.target,
        &canonical_account(),
    );
    seed_invocation(
        &conn,
        "72000000-0000-4000-8000-000000000001",
        &models.last,
        &canonical_account(),
        "session-corrective-original-real",
        "succeeded",
        "2026-06-20T10:03:00Z",
    );

    seed_corrective_chain(
        &conn,
        "chain-corrective-no-different-evidence",
        "session-corrective-no-different-evidence",
        &models.target,
        "<unknown>",
        &models.target,
        &canonical_account(),
    );
    seed_invocation(
        &conn,
        "73000000-0000-4000-8000-000000000001",
        &models.target,
        &canonical_account(),
        "session-corrective-no-different-evidence",
        "succeeded",
        "2026-06-20T10:04:00Z",
    );
    seed_invocation(
        &conn,
        "73000000-0000-4000-8000-000000000002",
        "<unknown>",
        &canonical_account(),
        "session-corrective-no-different-evidence",
        "failed",
        "2026-06-20T10:05:00Z",
    );

    seed_corrective_chain(
        &conn,
        "chain-corrective-non-family",
        "session-corrective-non-family",
        &models.non_family_model,
        "<unknown>",
        &models.target,
        &models.non_family_account,
    );
    seed_invocation(
        &conn,
        "74000000-0000-4000-8000-000000000001",
        &models.last,
        &models.non_family_account,
        "session-corrective-non-family",
        "succeeded",
        "2026-06-20T10:06:00Z",
    );
}

fn seed_corrective_fallback_fixture(fixture: &Fixture, models: &S11M2cScopeModels) {
    let conn = fixture.conn();
    seed_current_default_chain(
        &conn,
        "chain-fallback-single",
        "session-fallback-single",
        &models.target,
        &canonical_account(),
    );
    seed_invocation(
        &conn,
        "81000000-0000-4000-8000-000000000001",
        &models.middle,
        &canonical_account(),
        "session-fallback-single",
        "succeeded",
        "2026-06-20T10:01:00Z",
    );

    seed_current_default_chain(
        &conn,
        "chain-fallback-none",
        "session-fallback-none",
        &models.target,
        &canonical_account(),
    );

    seed_current_default_chain(
        &conn,
        "chain-fallback-conflicting",
        "session-fallback-conflicting",
        &models.target,
        &canonical_account(),
    );
    seed_invocation(
        &conn,
        "82000000-0000-4000-8000-000000000001",
        &models.middle,
        &canonical_account(),
        "session-fallback-conflicting",
        "succeeded",
        "2026-06-20T10:02:00Z",
    );
    seed_invocation(
        &conn,
        "82000000-0000-4000-8000-000000000002",
        &models.last,
        &canonical_account(),
        "session-fallback-conflicting",
        "succeeded",
        "2026-06-20T10:03:00Z",
    );

    seed_current_default_chain(
        &conn,
        "chain-fallback-out-of-inventory",
        "session-fallback-out-of-inventory",
        &models.target,
        &canonical_account(),
    );
    seed_invocation(
        &conn,
        "83000000-0000-4000-8000-000000000001",
        &source_model_name(),
        &canonical_account(),
        "session-fallback-out-of-inventory",
        "succeeded",
        "2026-06-20T10:04:00Z",
    );
}

fn write_synthetic_transcript(fixture: &Fixture, session_id: &str, models: &[&str]) -> PathBuf {
    let project_dir = fixture.transcript_projects_dir().join("synthetic-project");
    fs::create_dir_all(&project_dir).unwrap();
    let body = models
        .iter()
        .map(|model| format!(r#"{{"type":"assistant","message":{{"model":{model:?}}}}}"#))
        .collect::<Vec<_>>()
        .join("\n");
    let path = project_dir.join(format!("{session_id}.jsonl"));
    fs::write(&path, format!("{body}\n")).unwrap();
    path
}

fn seed_current_default_chain(
    conn: &Connection,
    chain_id: &str,
    session_id: &str,
    model_name: &str,
    provider_name: &str,
) {
    let turn_id = format!("turn-{session_id}");
    seed_chain(conn, chain_id, model_name);
    seed_segment(
        conn,
        chain_id,
        provider_name,
        session_id,
        None,
        Some(&turn_id),
        "initial",
    );
    seed_turn(conn, provider_name, session_id, &turn_id, "assistant");
}

#[allow(clippy::too_many_arguments)]
fn seed_corrective_chain(
    conn: &Connection,
    chain_id: &str,
    session_id: &str,
    current_model_name: &str,
    original_old_model_name: &str,
    original_new_model_name: &str,
    provider_name: &str,
) {
    seed_current_default_chain(
        conn,
        chain_id,
        session_id,
        current_model_name,
        provider_name,
    );
    ensure_original_preimage_table(conn);
    conn.execute(
        "INSERT INTO s11_wu4_restore_session_ownership_preimage
         (migration_id, entity_kind, row_pk, chain_id, old_model_name, new_model_name)
         VALUES (?1, 'chain', ?2, ?2, ?3, ?4)",
        params![
            SESSION_OWNERSHIP_MIGRATION_ID,
            chain_id,
            original_old_model_name,
            original_new_model_name,
        ],
    )
    .unwrap();
}

fn ensure_original_preimage_table(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS s11_wu4_restore_session_ownership_preimage (
            migration_id TEXT NOT NULL,
            entity_kind TEXT NOT NULL CHECK(entity_kind IN ('chain', 'segment', 'turn', 'invocation', 'segment_delete', 'turn_delete', 'segment_merge_survivor')),
            row_pk TEXT NOT NULL,
            chain_id TEXT,
            segment_id INTEGER,
            turn_row_id INTEGER,
            old_model_name TEXT,
            new_model_name TEXT,
            old_provider_name TEXT,
            new_provider_name TEXT,
            session_id TEXT,
            segment_started_at TEXT,
            segment_ended_at TEXT,
            segment_last_turn_id TEXT,
            segment_transition_reason TEXT,
            turn_id TEXT,
            turn_timestamp TEXT,
            turn_role TEXT,
            turn_parent_turn_id TEXT,
            turn_is_sidechain INTEGER,
            turn_is_compaction_boundary INTEGER,
            turn_source_file TEXT,
            turn_ingested_at TEXT,
            turn_body TEXT,
            new_started_at TEXT,
            new_ended_at TEXT,
            new_last_turn_id TEXT,
            new_transition_reason TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY (migration_id, entity_kind, row_pk)
        );",
    )
    .unwrap();
}

fn assert_chain_preimage_new_model(path: &Path, chain_id: &str, expected_new_model: &str) {
    let conn = Connection::open(path).unwrap();
    let actual: String = conn
        .query_row(
            "SELECT new_model_name
             FROM s11_wu4_restore_session_ownership_preimage
             WHERE entity_kind = 'chain' AND chain_id = ?1",
            [chain_id],
            |row| row.get(0),
        )
        .unwrap_or_else(|err| panic!("missing chain preimage for {chain_id}: {err}"));
    assert_eq!(actual, expected_new_model);
}

fn pragma_user_version(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn corrective_preimage_row_count(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    if !table_exists(&conn, "s11_m2c_model_corrective_preimage") {
        return 0;
    }
    conn.query_row(
        "SELECT COUNT(*) FROM s11_m2c_model_corrective_preimage",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn corrective_preimage_models(path: &Path, chain_id: &str) -> (String, String) {
    let conn = Connection::open(path).unwrap();
    conn.query_row(
        "SELECT old_model_name, new_model_name
         FROM s11_m2c_model_corrective_preimage
         WHERE chain_id = ?1",
        [chain_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap_or_else(|err| panic!("missing corrective preimage for {chain_id}: {err}"))
}

fn corrective_preimage_evidence_source(path: &Path, chain_id: &str) -> String {
    let conn = Connection::open(path).unwrap();
    conn.query_row(
        "SELECT evidence_source
         FROM s11_m2c_model_corrective_preimage
         WHERE chain_id = ?1",
        [chain_id],
        |row| row.get(0),
    )
    .unwrap_or_else(|err| panic!("missing corrective preimage evidence for {chain_id}: {err}"))
}

fn corrective_residual_default_count(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_m2c_model_corrective_preimage p
         JOIN session_chains c ON c.chain_id = p.chain_id
         WHERE c.model_name = p.old_model_name",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn assert_corrective_apply_report(
    report: &str,
    expected_inferred_model: &str,
    expected_planned: i64,
    expected_applied: i64,
) {
    assert!(
        report.contains("preimage_table") && report.contains("s11_m2c_model_corrective_preimage"),
        "report missing corrective preimage identity: {report}"
    );
    assert_report_count(
        report,
        "corrective_chain_model_updates_to_apply",
        expected_planned,
    );
    assert_report_count(
        report,
        "corrective_chain_model_updates_applied",
        expected_applied,
    );
    assert_report_count(report, "corrective_preimage_rows", expected_applied.max(1));
    assert!(
        report.contains("new_model_name") && report.contains(expected_inferred_model),
        "report missing inferred-model grouping for {expected_inferred_model}: {report}"
    );
    assert!(
        report.contains("quick_check") && report.contains("ok"),
        "report missing quick_check ok: {report}"
    );
    assert!(
        report.contains("user_version"),
        "report missing user_version: {report}"
    );
}

fn assert_corrective_rollback_report(report: &str, expected_restored: i64) {
    assert!(
        report.contains("preimage_table") && report.contains("s11_m2c_model_corrective_preimage"),
        "rollback report missing corrective preimage identity: {report}"
    );
    assert!(
        report.contains("restored_model_semantics") && report.contains("backfill_default"),
        "rollback report missing default-restoration semantics: {report}"
    );
    assert_report_count(
        report,
        "corrective_chain_model_updates_restored",
        expected_restored,
    );
    assert_report_count(report, "corrective_chain_model_rollback_mismatches", 0);
}

fn fake_contract_provider_script() -> String {
    r#"#!/usr/bin/env python3
import json
import sys

CONTRACT = "oulipoly.provider/v1"
request = json.loads(sys.stdin.read() or "{}")
response = {
    "contract": request.get("contract", CONTRACT),
    "request_id": request.get("request_id", "s11-m2b-provider-proof"),
    "ok": True,
    "result": {
        "provider_id": "s11-m2b-proof-provider",
        "display_name": "S11 M2b Proof Provider",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {
            "launch": False,
            "policy": False,
            "quota": False,
            "session": False,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        },
    },
}
json.dump(response, sys.stdout)
sys.stdout.write("\n")
"#
    .to_string()
}

fn assert_live_command_guard(cmd: &Command, fixture: &Fixture) {
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let state_db_index = args
        .iter()
        .position(|arg| arg == "--state-db")
        .unwrap_or_else(|| panic!("live command missing --state-db: {args:?}"));
    let state_db_arg = args
        .get(state_db_index + 1)
        .unwrap_or_else(|| panic!("live command missing --state-db value: {args:?}"));
    assert_eq!(Path::new(state_db_arg), fixture.state_path.as_path());
    assert!(
        Path::new(state_db_arg).starts_with(fixture.dir.path()),
        "state DB escaped temp fixture: {state_db_arg}"
    );

    assert_command_env_eq(cmd, "XDG_CONFIG_HOME", &fixture.config_dir);
    assert_command_env_eq(cmd, "XDG_DATA_HOME", &fixture.dir.path().join("xdg-data"));
    assert_command_env_eq(
        cmd,
        "OULIPOLY_DATA_DIR",
        &fixture.dir.path().join("xdg-data/oulipoly-agent-runner"),
    );
    let path = command_env_value(cmd, "PATH")
        .unwrap_or_else(|| panic!("PATH not set on guarded command"))
        .unwrap_or_else(|| panic!("PATH was removed on guarded command"));
    let path_entries: Vec<_> = std::env::split_paths(&path).collect();
    assert_eq!(path_entries.first(), Some(&fixture.provider_bin_dir));
}

fn assert_command_env_eq(cmd: &Command, key: &str, expected: &Path) {
    let value = command_env_value(cmd, key)
        .unwrap_or_else(|| panic!("{key} not set on command"))
        .unwrap_or_else(|| panic!("{key} was removed on command"));
    assert_eq!(Path::new(&value), expected);
}

fn command_env_value(cmd: &Command, key: &str) -> Option<Option<std::ffi::OsString>> {
    cmd.get_envs()
        .find(|(candidate, _)| candidate.to_string_lossy() == key)
        .map(|(_, value)| value.map(|value| value.to_os_string()))
}

fn output_path_value(output: &Output, key: &str) -> PathBuf {
    let prefix = format!("{key}=");
    combined_output(output)
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&prefix))
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("output missing {prefix}: {}", combined_output(output)))
}

fn read_report_from_output(output: &Output) -> String {
    fs::read_to_string(output_path_value(output, "report")).unwrap()
}

fn assert_quick_check_ok(path: &Path) {
    let conn = Connection::open(path).unwrap();
    let quick_check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        quick_check,
        "ok",
        "quick_check failed for {}",
        path.display()
    );
}

fn directory_is_empty(path: &Path) -> bool {
    fs::read_dir(path).unwrap().next().is_none()
}

fn assert_live_counts_and_zero_residual(path: &Path) {
    let conn = Connection::open(path).unwrap();
    let chain_planned = last_run_count(&conn, "chain_model_updates_to_apply");
    let segment_planned = last_run_count(&conn, "segment_provider_updates_to_apply");
    let turn_planned = last_run_count(&conn, "turn_provider_updates_to_apply");
    let invocation_planned = last_run_count(&conn, "invocation_identity_updates_to_apply");
    assert_eq!(chain_planned, applied_chain_count(&conn));
    assert_eq!(segment_planned, applied_segment_count(&conn));
    assert_eq!(turn_planned, applied_turn_count(&conn));
    assert_eq!(invocation_planned, applied_invocation_count(&conn));
    assert_eq!(residual_old_owner_count(&conn), 0);
    assert_eq!(residual_noncanonical_rotation_segment_owner_count(&conn), 0);
    assert_eq!(residual_noncanonical_rotation_turn_owner_count(&conn), 0);
    assert_eq!(
        residual_noncanonical_rotation_invocation_owner_count(path),
        0
    );
    assert_eq!(residual_stale_invocation_model_count(path), 0);
    assert!(preimage_row_count(&conn) > 0, "preimage table is empty");
}

fn last_run_count(conn: &Connection, key: &str) -> i64 {
    conn.query_row(
        "SELECT value FROM s11_wu4_last_run_counts WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
    .unwrap_or_else(|err| panic!("missing last-run count {key}: {err}"))
}

fn applied_chain_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_chains c ON c.chain_id = p.chain_id
         WHERE p.entity_kind = 'chain'
           AND p.old_model_name <> p.new_model_name
           AND c.model_name = p.new_model_name",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn applied_segment_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_chain_segments s ON s.id = p.segment_id
         WHERE p.entity_kind = 'segment'
           AND p.old_provider_name <> p.new_provider_name
           AND s.provider_name = p.new_provider_name",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn applied_turn_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_turns t ON t.id = p.turn_row_id
         WHERE p.entity_kind = 'turn'
           AND p.old_provider_name <> p.new_provider_name
           AND t.provider_name = p.new_provider_name",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn applied_invocation_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage p
         JOIN invocations i ON i.id = CAST(p.row_pk AS INTEGER)
         WHERE p.entity_kind = 'invocation'
           AND (p.old_model_name <> p.new_model_name OR p.old_provider_name <> p.new_provider_name)
           AND i.model_name = p.new_model_name
           AND i.provider_name IS p.new_provider_name",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn residual_old_owner_count(conn: &Connection) -> i64 {
    let chains: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM s11_wu4_restore_session_ownership_preimage p
             JOIN session_chains c ON c.chain_id = p.chain_id
             WHERE p.entity_kind = 'chain'
               AND p.old_model_name <> p.new_model_name
               AND c.model_name = p.old_model_name",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let segments: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM s11_wu4_restore_session_ownership_preimage p
             JOIN session_chain_segments s ON s.id = p.segment_id
             WHERE p.entity_kind = 'segment'
               AND p.old_provider_name <> p.new_provider_name
               AND s.provider_name = p.old_provider_name",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let turns: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM s11_wu4_restore_session_ownership_preimage p
             JOIN session_turns t ON t.id = p.turn_row_id
             WHERE p.entity_kind = 'turn'
               AND p.old_provider_name <> p.new_provider_name
               AND t.provider_name = p.old_provider_name",
            [],
            |row| row.get(0),
        )
        .unwrap();
    chains + segments + turns
}

fn residual_noncanonical_rotation_segment_owner_count(conn: &Connection) -> i64 {
    let pattern = format!("%{}%", provider_token());
    conn.query_row(
        "SELECT COUNT(*)
         FROM session_chain_segments s
         JOIN session_chains c ON c.chain_id = s.chain_id
         WHERE c.model_name = ?4
           AND s.provider_name LIKE ?1
           AND s.provider_name <> ?2
           AND s.provider_name <> ?3",
        params![
            pattern,
            canonical_account(),
            accepted_account(),
            target_model_name()
        ],
        |row| row.get(0),
    )
    .unwrap()
}

fn residual_noncanonical_rotation_turn_owner_count(conn: &Connection) -> i64 {
    let pattern = format!("%{}%", provider_token());
    conn.query_row(
        "SELECT COUNT(*)
         FROM session_turns t
         WHERE t.provider_name LIKE ?1
           AND t.provider_name <> ?2
           AND t.provider_name <> ?3
           AND EXISTS (
               SELECT 1
               FROM session_chain_segments s
               JOIN session_chains c ON c.chain_id = s.chain_id
               WHERE s.session_id = t.session_id
                 AND c.model_name = ?4
           )",
        params![
            pattern,
            canonical_account(),
            accepted_account(),
            target_model_name()
        ],
        |row| row.get(0),
    )
    .unwrap()
}

fn residual_noncanonical_rotation_invocation_owner_count(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    let pattern = format!("%{}%", provider_token());
    conn.query_row(
        "SELECT COUNT(*)
         FROM invocations i
         WHERE i.provider_name LIKE ?1
           AND i.provider_name <> ?2
           AND i.provider_name <> ?3
           AND EXISTS (
               SELECT 1
               FROM session_chain_segments s
               JOIN session_chains c ON c.chain_id = s.chain_id
               WHERE s.session_id = COALESCE(i.provider_session_id, i.session_id)
                 AND c.model_name = ?4
           )",
        params![
            pattern,
            canonical_account(),
            accepted_account(),
            target_model_name()
        ],
        |row| row.get(0),
    )
    .unwrap()
}

fn residual_stale_invocation_model_count(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.query_row(
        "SELECT COUNT(*)
         FROM invocations i
         JOIN session_chain_segments s ON s.session_id = COALESCE(i.provider_session_id, i.session_id)
         JOIN session_chains c ON c.chain_id = s.chain_id
         WHERE c.model_name = ?1
           AND i.model_name <> ?1
           AND i.model_name <> '<unknown>'",
        [target_model_name()],
        |row| row.get(0),
    )
    .unwrap()
}

fn preimage_row_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM s11_wu4_restore_session_ownership_preimage",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn provider_toml_entry(name: &str, command: &Path) -> String {
    format!(
        "[{name}]\ncommand = {:?}\nargs = []\nprompt_mode = \"arg\"\n\n",
        command.to_string_lossy()
    )
}

fn provider_toml_entry_with_session_storage(
    name: &str,
    command: &Path,
    projects_dir: &Path,
) -> String {
    format!(
        "[{name}]\ncommand = {:?}\nargs = []\nprompt_mode = \"arg\"\n\n[{name}.session_storage]\nkind = {:?}\nprojects_dir = {:?}\n\n",
        command.to_string_lossy(),
        format!("{}_code", provider_token()),
        projects_dir.to_string_lossy()
    )
}

fn provider_toml_entry_with_resume(name: &str, command: &Path) -> String {
    format!(
        "[{name}]\ncommand = {:?}\nargs = []\ninteractive_args = []\nprompt_mode = \"arg\"\n\n[{name}.resume]\nkind = \"flag\"\nflag = \"--resume\"\n\n",
        command.to_string_lossy()
    )
}

fn provider_toml_entry_with_resume_storage(
    name: &str,
    command: &Path,
    projects_dir: &Path,
) -> String {
    format!(
        "[{name}]\ncommand = {:?}\nargs = []\ninteractive_args = []\nprompt_mode = \"arg\"\n\n[{name}.resume]\nkind = \"flag\"\nflag = \"--resume\"\n\n[{name}.session_storage]\nkind = \"claude_code\"\nprojects_dir = {:?}\n\n",
        command.to_string_lossy(),
        projects_dir.to_string_lossy()
    )
}

fn claude_project_dir_name(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| match c {
            '/' | '\\' => '-',
            c if (c.is_ascii() && c.is_alphanumeric()) || c == '-' => c,
            _ => '-',
        })
        .collect()
}

fn transcript_file_snapshot(dir: &Path) -> Vec<(String, String)> {
    let mut files = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read_to_string(entry.path()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    files.sort();
    files
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

#[allow(clippy::too_many_arguments)]
fn seed_segment_with_started_at(
    conn: &Connection,
    chain_id: &str,
    provider_name: &str,
    session_id: &str,
    started_at: &str,
    ended_at: Option<&str>,
    last_turn_id: Option<&str>,
    transition_reason: &str,
) -> i64 {
    conn.execute(
        "INSERT INTO session_chain_segments
         (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[allow(clippy::too_many_arguments)]
fn seed_turn_full(
    conn: &Connection,
    provider_name: &str,
    session_id: &str,
    turn_id: &str,
    timestamp: &str,
    role: &str,
    parent_turn_id: Option<&str>,
    is_sidechain: i64,
    is_compaction_boundary: i64,
    source_file: &str,
    ingested_at: &str,
    body: Option<&str>,
) -> i64 {
    conn.execute(
        "INSERT INTO session_turns
         (provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
          is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            provider_name,
            session_id,
            turn_id,
            timestamp,
            role,
            parent_turn_id,
            is_sidechain,
            is_compaction_boundary,
            source_file,
            ingested_at,
            body,
        ],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn seed_invocation(
    conn: &Connection,
    invocation_uuid: &str,
    model_name: &str,
    provider_name: &str,
    session_id: &str,
    status: &str,
    created_at: &str,
) -> i64 {
    conn.execute(
        "INSERT INTO invocations
         (invocation_uuid, model_name, provider_name, provider_index, status, success, exit_code,
          session_id, provider_session_id, created_at, finished_at)
         VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?7, ?8, ?8)",
        params![
            invocation_uuid,
            model_name,
            provider_name,
            status,
            if status == "succeeded" {
                Some(1_i64)
            } else {
                None
            },
            if status == "succeeded" {
                Some(0_i64)
            } else {
                None
            },
            session_id,
            created_at,
        ],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn seed_three_segment_collision(fixture: &Fixture) {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-merge-three", &source_model_name());
    seed_segment_with_started_at(
        &conn,
        "chain-merge-three",
        &unregistered_account_a(),
        "session-merge",
        "2026-06-20T10:00:00Z",
        Some("2026-06-20T10:04:00Z"),
        Some("turn-merge-first"),
        "initial",
    );
    seed_segment_with_started_at(
        &conn,
        "chain-merge-three",
        &canonical_account(),
        "session-merge",
        "2026-06-20T10:05:00Z",
        Some("2026-06-20T10:09:00Z"),
        Some("turn-merge-middle"),
        "manual",
    );
    seed_segment_with_started_at(
        &conn,
        "chain-merge-three",
        &unregistered_account_family(2),
        "session-merge",
        "2026-06-20T10:10:00Z",
        None,
        Some("turn-merge-last"),
        "exhausted",
    );
    seed_segment_with_started_at(
        &conn,
        "chain-merge-three",
        &unregistered_account_family(3),
        "session-unrelated",
        "2026-06-20T11:00:00Z",
        Some("2026-06-20T11:05:00Z"),
        Some("turn-unrelated"),
        "imported",
    );
    for (provider, session_id, turn_id) in [
        (
            unregistered_account_a(),
            "session-merge",
            "turn-merge-first",
        ),
        (canonical_account(), "session-merge", "turn-merge-middle"),
        (
            unregistered_account_family(2),
            "session-merge",
            "turn-merge-last",
        ),
        (
            unregistered_account_family(3),
            "session-unrelated",
            "turn-unrelated",
        ),
    ] {
        seed_turn(&conn, &provider, session_id, turn_id, "assistant");
    }
    seed_mailbox(&fixture.mailbox_path, "session-merge", None);
    seed_mailbox(&fixture.mailbox_path, "session-unrelated", None);
}

fn seed_open_survivor_collision(fixture: &Fixture) -> (i64, i64) {
    let conn = fixture.conn();
    seed_chain(
        &conn,
        "chain-open-survivor-regression",
        &source_model_name(),
    );
    let open_id = seed_segment_with_started_at(
        &conn,
        "chain-open-survivor-regression",
        &canonical_account(),
        "session-open-survivor-regression",
        "2026-06-20T10:00:00Z",
        None,
        Some("turn-open-active"),
        "manual",
    );
    let closed_later_id = seed_segment_with_started_at(
        &conn,
        "chain-open-survivor-regression",
        &unregistered_account_family(2),
        "session-open-survivor-regression",
        "2026-06-20T10:05:00Z",
        Some("2026-06-20T10:06:00Z"),
        Some("turn-closed-later-tail"),
        "quota_threshold",
    );
    seed_turn_full(
        &conn,
        &canonical_account(),
        "session-open-survivor-regression",
        "turn-open-active",
        "2026-06-20T10:01:00Z",
        "assistant",
        None,
        0,
        0,
        "open-survivor.jsonl",
        "2026-06-20T10:01:00Z",
        Some("open active body"),
    );
    seed_turn_full(
        &conn,
        &unregistered_account_family(2),
        "session-open-survivor-regression",
        "turn-closed-later-tail",
        "2026-06-20T10:06:00Z",
        "assistant",
        Some("turn-open-active"),
        0,
        0,
        "open-survivor.jsonl",
        "2026-06-20T10:06:00Z",
        Some("later closed tail body"),
    );
    seed_mailbox(
        &fixture.mailbox_path,
        "session-open-survivor-regression",
        None,
    );
    (open_id, closed_later_id)
}

fn seed_turn_dedup_collision(fixture: &Fixture) -> TurnRowSnapshot {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-turn-dedup", &source_model_name());
    seed_segment_with_started_at(
        &conn,
        "chain-turn-dedup",
        &unregistered_account_a(),
        "session-dedup",
        "2026-06-20T12:00:00Z",
        Some("2026-06-20T12:04:00Z"),
        Some("turn-dedup"),
        "manual",
    );
    seed_segment_with_started_at(
        &conn,
        "chain-turn-dedup",
        &canonical_account(),
        "session-dedup",
        "2026-06-20T12:05:00Z",
        None,
        Some("turn-dedup"),
        "exhausted",
    );
    let common_body = "identical-dedup-body";
    let winner_id = seed_turn_full(
        &conn,
        &unregistered_account_a(),
        "session-dedup",
        "turn-dedup",
        "2026-06-20T12:01:00Z",
        "assistant",
        Some("turn-parent"),
        1,
        0,
        "dedup.jsonl",
        "2026-06-20T12:02:00Z",
        Some(common_body),
    );
    seed_turn_full(
        &conn,
        &canonical_account(),
        "session-dedup",
        "turn-dedup",
        "2026-06-20T12:01:00Z",
        "assistant",
        Some("turn-parent"),
        1,
        0,
        "dedup.jsonl",
        "2026-06-20T12:02:00Z",
        Some(common_body),
    );
    seed_mailbox(&fixture.mailbox_path, "session-dedup", None);
    query_turns_with_id(&conn)
        .into_iter()
        .find(|turn| turn.id == winner_id)
        .unwrap()
}

fn seed_divergent_turn_collision(fixture: &Fixture) {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-divergent-turn", &source_model_name());
    seed_segment_with_started_at(
        &conn,
        "chain-divergent-turn",
        &unregistered_account_a(),
        "session-divergent",
        "2026-06-20T13:00:00Z",
        Some("2026-06-20T13:04:00Z"),
        Some("turn-divergent"),
        "manual",
    );
    seed_segment_with_started_at(
        &conn,
        "chain-divergent-turn",
        &canonical_account(),
        "session-divergent",
        "2026-06-20T13:05:00Z",
        None,
        Some("turn-divergent"),
        "exhausted",
    );
    seed_turn_full(
        &conn,
        &unregistered_account_a(),
        "session-divergent",
        "turn-divergent",
        "2026-06-20T13:01:00Z",
        "assistant",
        Some("parent-left"),
        1,
        0,
        "divergent.jsonl",
        "2026-06-20T13:02:00Z",
        Some("left-body"),
    );
    seed_turn_full(
        &conn,
        &canonical_account(),
        "session-divergent",
        "turn-divergent",
        "2026-06-20T13:01:00Z",
        "assistant",
        Some("parent-right"),
        0,
        1,
        "divergent.jsonl",
        "2026-06-20T13:03:00Z",
        Some("right-body"),
    );
    seed_mailbox(&fixture.mailbox_path, "session-divergent", None);
}

fn seed_ingested_at_only_turn_collision(fixture: &Fixture) -> TurnRowSnapshot {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-ingested-at-metadata", &source_model_name());
    let providers = [
        unregistered_account_a(),
        canonical_account(),
        unregistered_account_family(2),
    ];
    for (index, provider) in providers.iter().enumerate() {
        seed_segment_with_started_at(
            &conn,
            "chain-ingested-at-metadata",
            provider,
            "session-ingested-at-metadata",
            &format!("2026-06-20T16:0{index}:00Z"),
            if index == providers.len() - 1 {
                None
            } else {
                Some("2026-06-20T16:09:00Z")
            },
            Some("turn-ingested-at-metadata"),
            if index == providers.len() - 1 {
                "exhausted"
            } else {
                "manual"
            },
        );
    }
    let mut winner_id = None;
    for (index, provider) in providers.iter().enumerate() {
        let row_id = seed_turn_full(
            &conn,
            provider,
            "session-ingested-at-metadata",
            "turn-ingested-at-metadata",
            "2026-06-20T16:01:00Z",
            "assistant",
            Some("turn-ingested-parent"),
            1,
            0,
            "metadata-ingested.jsonl",
            &format!("2026-06-20T16:1{index}:00Z"),
            Some("metadata-identical-body"),
        );
        winner_id.get_or_insert(row_id);
    }
    seed_mailbox(&fixture.mailbox_path, "session-ingested-at-metadata", None);
    query_turns_with_id(&conn)
        .into_iter()
        .find(|turn| turn.id == winner_id.unwrap())
        .unwrap()
}

fn seed_parent_turn_id_only_turn_collision(fixture: &Fixture) -> TurnRowSnapshot {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-parent-metadata", &source_model_name());
    seed_segment_with_started_at(
        &conn,
        "chain-parent-metadata",
        &unregistered_account_a(),
        "session-parent-metadata",
        "2026-06-20T17:00:00Z",
        Some("2026-06-20T17:04:00Z"),
        Some("turn-parent-metadata"),
        "manual",
    );
    seed_segment_with_started_at(
        &conn,
        "chain-parent-metadata",
        &canonical_account(),
        "session-parent-metadata",
        "2026-06-20T17:05:00Z",
        None,
        Some("turn-parent-metadata"),
        "exhausted",
    );
    let winner_id = seed_turn_full(
        &conn,
        &unregistered_account_a(),
        "session-parent-metadata",
        "turn-parent-metadata",
        "2026-06-20T17:01:00Z",
        "assistant",
        Some("winner-parent"),
        0,
        1,
        "metadata-parent.jsonl",
        "2026-06-20T17:02:00Z",
        Some("metadata-identical-body"),
    );
    seed_turn_full(
        &conn,
        &canonical_account(),
        "session-parent-metadata",
        "turn-parent-metadata",
        "2026-06-20T17:01:00Z",
        "assistant",
        Some("loser-parent"),
        0,
        1,
        "metadata-parent.jsonl",
        "2026-06-20T17:02:00Z",
        Some("metadata-identical-body"),
    );
    seed_mailbox(&fixture.mailbox_path, "session-parent-metadata", None);
    query_turns_with_id(&conn)
        .into_iter()
        .find(|turn| turn.id == winner_id)
        .unwrap()
}

fn assert_intrinsic_divergence_aborts_apply_without_mutation(field: &str) {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_single_intrinsic_field_divergence(&fixture, field);
    let before = snapshot(&fixture.state_path);
    let hash_before = file_hash(&fixture.state_path);
    let backup_dir = fixture.backup_dir();

    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();

    assert_failure(&output);
    assert_failure_mentions(&output, &fixture.scratch_dir, "divergent");
    assert_eq!(
        file_hash(&fixture.state_path),
        hash_before,
        "live DB hash changed"
    );
    assert_eq!(
        snapshot(&fixture.state_path),
        before,
        "live DB rows changed"
    );
    assert_copied_db_unchanged_if_present(&fixture.scratch_dir, &before);
}

fn seed_single_intrinsic_field_divergence(fixture: &Fixture, field: &str) {
    let conn = fixture.conn();
    let chain_id = format!("chain-{field}-intrinsic");
    let session_id = format!("session-{field}-intrinsic");
    let turn_id = format!("turn-{field}-intrinsic");
    seed_chain(&conn, &chain_id, &source_model_name());
    seed_segment_with_started_at(
        &conn,
        &chain_id,
        &unregistered_account_a(),
        &session_id,
        "2026-06-20T18:00:00Z",
        Some("2026-06-20T18:04:00Z"),
        Some(&turn_id),
        "manual",
    );
    seed_segment_with_started_at(
        &conn,
        &chain_id,
        &canonical_account(),
        &session_id,
        "2026-06-20T18:05:00Z",
        None,
        Some(&turn_id),
        "exhausted",
    );
    let (left_timestamp, right_timestamp) = if field == "timestamp" {
        ("2026-06-20T18:01:00Z", "2026-06-20T18:02:00Z")
    } else {
        ("2026-06-20T18:01:00Z", "2026-06-20T18:01:00Z")
    };
    let (left_role, right_role) = if field == "role" {
        ("assistant", "user")
    } else {
        ("assistant", "assistant")
    };
    let (left_body, right_body) = if field == "body" {
        ("left-intrinsic-body", "right-intrinsic-body")
    } else {
        ("shared-intrinsic-body", "shared-intrinsic-body")
    };
    for (provider, timestamp, role, body) in [
        (
            unregistered_account_a(),
            left_timestamp,
            left_role,
            left_body,
        ),
        (canonical_account(), right_timestamp, right_role, right_body),
    ] {
        seed_turn_full(
            &conn,
            &provider,
            &session_id,
            &turn_id,
            timestamp,
            role,
            Some("shared-parent"),
            1,
            0,
            "intrinsic-divergence.jsonl",
            "2026-06-20T18:03:00Z",
            Some(body),
        );
    }
    seed_mailbox(&fixture.mailbox_path, &session_id, None);
}

fn seed_large_rotation_collision(fixture: &Fixture) {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-large-rotation", &source_model_name());
    let providers = [
        unregistered_account_a(),
        unregistered_account_family(2),
        canonical_account(),
        unregistered_account_family(3),
    ];
    for (index, provider) in providers.iter().enumerate() {
        seed_segment_with_started_at(
            &conn,
            "chain-large-rotation",
            provider,
            "session-large",
            &format!("2026-06-20T14:0{index}:00Z"),
            if index == providers.len() - 1 {
                None
            } else {
                Some("2026-06-20T14:09:00Z")
            },
            Some("turn-large-3"),
            if index == providers.len() - 1 {
                "exhausted"
            } else {
                "manual"
            },
        );
    }
    for turn_id in ["turn-large-1", "turn-large-2", "turn-large-3"] {
        for provider in &providers {
            seed_turn_full(
                &conn,
                provider,
                "session-large",
                turn_id,
                "2026-06-20T14:01:00Z",
                "assistant",
                None,
                0,
                0,
                "large.jsonl",
                "2026-06-20T14:02:00Z",
                Some(&format!("body-{turn_id}")),
            );
        }
    }
    seed_turn_full(
        &conn,
        &unregistered_account_family(2),
        "session-large",
        "turn-large-unique",
        "2026-06-20T14:03:00Z",
        "user",
        Some("turn-large-3"),
        0,
        0,
        "large-unique.jsonl",
        "2026-06-20T14:04:00Z",
        Some("body-turn-large-unique"),
    );
    seed_chain(&conn, "chain-large-unrelated", &source_model_name());
    seed_segment_with_started_at(
        &conn,
        "chain-large-unrelated",
        &unregistered_account_family(4),
        "session-large-unrelated",
        "2026-06-20T15:00:00Z",
        None,
        Some("turn-large-unrelated"),
        "initial",
    );
    seed_turn(
        &conn,
        &unregistered_account_family(4),
        "session-large-unrelated",
        "turn-large-unrelated",
        "assistant",
    );
    seed_mailbox(&fixture.mailbox_path, "session-large", None);
    seed_mailbox(&fixture.mailbox_path, "session-large-unrelated", None);
}

fn seed_merged_away_rotation_turns(fixture: &Fixture) -> Vec<&'static str> {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-turns-merged-away", &source_model_name());
    seed_segment_with_started_at(
        &conn,
        "chain-turns-merged-away",
        &canonical_account(),
        "session-turns-merged-away",
        "2026-06-20T19:00:00Z",
        None,
        Some("turn-current-survivor"),
        "exhausted",
    );
    seed_turn(
        &conn,
        &canonical_account(),
        "session-turns-merged-away",
        "turn-current-survivor",
        "assistant",
    );
    let historical_provider = unregistered_account_family(4);
    let turn_ids = vec!["turn-historical-one", "turn-historical-two"];
    for turn_id in &turn_ids {
        seed_turn(
            &conn,
            &historical_provider,
            "session-turns-merged-away",
            turn_id,
            "user",
        );
    }
    seed_mailbox(&fixture.mailbox_path, "session-turns-merged-away", None);
    turn_ids
}

fn seed_merged_away_identical_turn_collision(fixture: &Fixture) -> TurnRowSnapshot {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-turns-identical", &source_model_name());
    seed_segment_with_started_at(
        &conn,
        "chain-turns-identical",
        &canonical_account(),
        "session-turns-identical",
        "2026-06-20T20:00:00Z",
        None,
        Some("turn-identical"),
        "exhausted",
    );
    let winner_id = seed_turn_full(
        &conn,
        &canonical_account(),
        "session-turns-identical",
        "turn-identical",
        "2026-06-20T20:01:00Z",
        "assistant",
        Some("turn-identical-parent"),
        0,
        1,
        "widened-identical.jsonl",
        "2026-06-20T20:02:00Z",
        Some("widened-identical-body"),
    );
    seed_turn_full(
        &conn,
        &unregistered_account_family(5),
        "session-turns-identical",
        "turn-identical",
        "2026-06-20T20:01:00Z",
        "assistant",
        Some("turn-identical-parent"),
        0,
        1,
        "widened-identical.jsonl",
        "2026-06-20T20:03:00Z",
        Some("widened-identical-body"),
    );
    seed_mailbox(&fixture.mailbox_path, "session-turns-identical", None);
    query_turns_with_id(&conn)
        .into_iter()
        .find(|turn| turn.id == winner_id)
        .unwrap()
}

fn seed_merged_away_divergent_turn_collision(fixture: &Fixture) {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-turns-divergent", &source_model_name());
    seed_segment_with_started_at(
        &conn,
        "chain-turns-divergent",
        &canonical_account(),
        "session-turns-divergent",
        "2026-06-20T21:00:00Z",
        None,
        Some("turn-widened-divergent"),
        "exhausted",
    );
    seed_turn_full(
        &conn,
        &canonical_account(),
        "session-turns-divergent",
        "turn-widened-divergent",
        "2026-06-20T21:01:00Z",
        "assistant",
        Some("turn-divergent-parent"),
        0,
        0,
        "widened-divergent.jsonl",
        "2026-06-20T21:02:00Z",
        Some("right-widened-body"),
    );
    seed_turn_full(
        &conn,
        &unregistered_account_family(6),
        "session-turns-divergent",
        "turn-widened-divergent",
        "2026-06-20T21:01:00Z",
        "assistant",
        Some("turn-divergent-parent"),
        0,
        0,
        "widened-divergent.jsonl",
        "2026-06-20T21:03:00Z",
        Some("left-widened-body"),
    );
    seed_mailbox(&fixture.mailbox_path, "session-turns-divergent", None);
}

fn seed_widened_turn_comprehensive_population(fixture: &Fixture) -> WidenedComprehensiveSessions {
    let conn = fixture.conn();
    let sessions = WidenedComprehensiveSessions {
        merged_session: "session-comprehensive-merged",
        orphan_session: "session-comprehensive-orphan",
        rotation_session: "session-comprehensive-rotation",
        accepted_session: "session-comprehensive-accepted",
    };

    seed_chain(&conn, "chain-comprehensive-merged", &source_model_name());
    seed_segment(
        &conn,
        "chain-comprehensive-merged",
        &canonical_account(),
        sessions.merged_session,
        None,
        Some("turn-comprehensive-current"),
        "initial",
    );
    seed_turn(
        &conn,
        &unregistered_account_family(4),
        sessions.merged_session,
        "turn-comprehensive-historical",
        "user",
    );

    seed_chain(&conn, "chain-comprehensive-orphan", "<unknown>");
    seed_segment(
        &conn,
        "chain-comprehensive-orphan",
        &canonical_account(),
        sessions.orphan_session,
        None,
        Some("turn-comprehensive-orphan"),
        "initial",
    );
    seed_turn(
        &conn,
        &unregistered_account_family(5),
        sessions.orphan_session,
        "turn-comprehensive-orphan",
        "assistant",
    );

    seed_chain(&conn, "chain-comprehensive-rotation", &source_model_name());
    seed_segment(
        &conn,
        "chain-comprehensive-rotation",
        &unregistered_account_a(),
        sessions.rotation_session,
        None,
        Some("turn-comprehensive-rotation"),
        "manual",
    );
    seed_turn(
        &conn,
        &unregistered_account_a(),
        sessions.rotation_session,
        "turn-comprehensive-rotation",
        "assistant",
    );

    seed_chain(&conn, "chain-comprehensive-accepted", &source_model_name());
    seed_segment(
        &conn,
        "chain-comprehensive-accepted",
        &canonical_account(),
        sessions.accepted_session,
        None,
        Some("turn-comprehensive-accepted"),
        "initial",
    );
    seed_turn(
        &conn,
        &accepted_account(),
        sessions.accepted_session,
        "turn-comprehensive-accepted",
        "assistant",
    );

    for session_id in [
        sessions.merged_session,
        sessions.orphan_session,
        sessions.rotation_session,
        sessions.accepted_session,
    ] {
        seed_mailbox(&fixture.mailbox_path, session_id, None);
    }
    sessions
}

fn seed_accepted_inventory_turn(fixture: &Fixture) {
    let conn = fixture.conn();
    seed_chain(&conn, "chain-turns-accepted", &source_model_name());
    seed_segment(
        &conn,
        "chain-turns-accepted",
        &canonical_account(),
        "session-turns-accepted",
        None,
        Some("turn-accepted-inventory"),
        "initial",
    );
    seed_turn(
        &conn,
        &accepted_account(),
        "session-turns-accepted",
        "turn-accepted-inventory",
        "assistant",
    );
    seed_mailbox(&fixture.mailbox_path, "session-turns-accepted", None);
}

fn seed_widened_rollback_population(fixture: &Fixture) {
    seed_merged_away_rotation_turns(fixture);
    seed_merged_away_identical_turn_collision(fixture);
}

fn apply_widened_fixture_snapshot_and_counts() -> (FullOwnershipSnapshot, BTreeMap<String, i64>) {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_widened_rollback_population(&fixture);
    let backup_dir = fixture.backup_dir();
    let output = fixture
        .apply_command(Some(&backup_dir), true, false, false)
        .output()
        .unwrap();
    assert_success(&output);
    let conn = Connection::open(&fixture.state_path).unwrap();
    let counts = last_run_counts_for_keys(
        &conn,
        &[
            "chain_model_updates_to_apply",
            "segment_provider_updates_to_apply",
            "turn_provider_updates_to_apply",
            "invocation_identity_updates_to_apply",
            "segment_rows_merged_away",
            "turn_rows_deduped_away",
            "post_apply_turn_collision_count",
        ],
    );
    (full_snapshot(&fixture.state_path), counts)
}

fn last_run_counts_for_keys(conn: &Connection, keys: &[&str]) -> BTreeMap<String, i64> {
    keys.iter()
        .map(|key| ((*key).to_string(), last_run_count(conn, key)))
        .collect()
}

fn seed_scale_smoke_population(
    fixture: &Fixture,
    chain_count: usize,
    dup_turns_per_chain: usize,
    unique_old_turns_per_chain: usize,
) {
    let mut conn = fixture.conn();
    let tx = conn.transaction().unwrap();
    let source_model = source_model_name();
    let canonical = canonical_account();
    let unregistered = unregistered_account_a();
    {
        let mut insert_chain = tx
            .prepare(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, ?2, ?2, ?3)",
            )
            .unwrap();
        let mut insert_segment = tx
            .prepare(
                "INSERT INTO session_chain_segments
                 (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id,
                  transition_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .unwrap();
        let mut insert_turn = tx
            .prepare(
                "INSERT INTO session_turns
                 (provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
                  is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )
            .unwrap();

        for chain_index in 0..chain_count {
            let chain_id = format!("chain-scale-{chain_index:04}");
            let session_id = format!("session-scale-{chain_index:04}");
            let last_turn_id = format!(
                "turn-scale-{chain_index:04}-dup-{:02}",
                dup_turns_per_chain - 1
            );
            insert_chain
                .execute(params![chain_id, FIXED_TS, source_model])
                .unwrap();
            insert_segment
                .execute(params![
                    chain_id,
                    unregistered,
                    session_id,
                    "2026-06-20T10:00:00Z",
                    Some("2026-06-20T10:04:00Z"),
                    last_turn_id,
                    "manual"
                ])
                .unwrap();
            insert_segment
                .execute(params![
                    chain_id,
                    canonical,
                    session_id,
                    "2026-06-20T10:05:00Z",
                    None::<&str>,
                    last_turn_id,
                    "exhausted"
                ])
                .unwrap();

            for turn_index in 0..dup_turns_per_chain {
                let turn_id = format!("turn-scale-{chain_index:04}-dup-{turn_index:02}");
                for provider in [&unregistered, &canonical] {
                    insert_turn
                        .execute(params![
                            provider,
                            session_id,
                            turn_id,
                            "2026-06-20T10:01:00Z",
                            "assistant",
                            Some("scale-parent"),
                            0,
                            0,
                            "scale.jsonl",
                            "2026-06-20T10:02:00Z",
                            Some(format!("body-{turn_id}"))
                        ])
                        .unwrap();
                }
            }

            for turn_index in 0..unique_old_turns_per_chain {
                let turn_id = format!("turn-scale-{chain_index:04}-unique-{turn_index:02}");
                insert_turn
                    .execute(params![
                        unregistered,
                        session_id,
                        turn_id,
                        "2026-06-20T10:03:00Z",
                        "user",
                        Some("scale-parent"),
                        0,
                        0,
                        "scale-unique.jsonl",
                        "2026-06-20T10:04:00Z",
                        Some(format!("body-{turn_id}"))
                    ])
                    .unwrap();
            }
        }
    }
    tx.commit().unwrap();
}

fn occupy_segment_id(path: &Path, id: i64) {
    let conn = Connection::open(path).unwrap();
    seed_chain(&conn, "chain-occupied-segment", &target_model_name());
    conn.execute(
        "INSERT INTO session_chain_segments
         (id, chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
         VALUES (?1, 'chain-occupied-segment', ?2, 'session-occupied-segment', ?3, NULL,
                 'turn-occupied-segment', 'manual')",
        params![id, accepted_account(), FIXED_TS],
    )
    .unwrap();
}

fn occupy_turn_id(path: &Path, id: i64) {
    let conn = Connection::open(path).unwrap();
    conn.execute(
        "INSERT INTO session_turns
         (id, provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, body)
         VALUES (?1, ?2, 'session-occupied-turn', 'turn-occupied-turn', ?3, 'assistant',
                 'occupied.jsonl', ?3, 'occupied-body')",
        params![id, accepted_account(), FIXED_TS],
    )
    .unwrap();
}

fn delete_segment_id(path: &Path, id: i64) {
    let conn = Connection::open(path).unwrap();
    let deleted = conn
        .execute("DELETE FROM session_chain_segments WHERE id = ?1", [id])
        .unwrap();
    assert_eq!(deleted, 1, "fixture did not delete survivor segment {id}");
}

fn assert_merge_survivor_identity_drift_detected(column: &str) {
    let fixture = Fixture::new();
    fixture.write_target_config();
    seed_three_segment_collision(&fixture);
    let before = full_snapshot(&fixture.state_path);
    let backup_dir = fixture.backup_dir();
    assert_success(
        &fixture
            .apply_command(Some(&backup_dir), true, false, false)
            .output()
            .unwrap(),
    );
    let survivor_id = latest_segment_id(&before, "chain-merge-three", "session-merge");
    drift_merge_survivor_identity(&fixture.state_path, survivor_id, column);
    let drifted = full_snapshot(&fixture.state_path);

    let rollback = fixture.rollback_command(true).output().unwrap();

    assert_failure(&rollback);
    assert_failure_mentions(
        &rollback,
        fixture.dir.path(),
        "segment merge survivor drift before rollback",
    );
    assert_eq!(
        full_snapshot(&fixture.state_path),
        drifted,
        "rollback mutated after survivor {column} drift"
    );
}

fn drift_merge_survivor_identity(path: &Path, survivor_id: i64, column: &str) {
    let conn = Connection::open(path).unwrap();
    match column {
        "chain_id" => {
            seed_chain(&conn, "chain-survivor-drift", &target_model_name());
            conn.execute(
                "UPDATE session_chain_segments SET chain_id = 'chain-survivor-drift' WHERE id = ?1",
                [survivor_id],
            )
            .unwrap();
        }
        "session_id" => {
            conn.execute(
                "UPDATE session_chain_segments SET session_id = 'session-survivor-drift' WHERE id = ?1",
                [survivor_id],
            )
            .unwrap();
        }
        other => panic!("unsupported survivor identity drift column {other}"),
    }
}

fn seed_mailbox(path: &Path, session_id: &str, cwd: Option<&str>) {
    let mut db = MailboxDb::open(path).unwrap();
    db.wake_sessions()
        .upsert_session_metadata(SessionMetadataUpsert {
            session_id,
            mode: "pty_interactive",
            invocation_uuid: None,
            provider_name: Some(&canonical_account()),
            model_name: Some(&target_model_name()),
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
        invocations: query_invocations(&conn),
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
    let body_expr = if table_has_column(conn, "session_turns", "body") {
        "body"
    } else {
        "NULL"
    };
    let mut stmt = conn
        .prepare(&format!(
            "SELECT provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
                    is_sidechain, is_compaction_boundary, {source_file_expr}, ingested_at, {body_expr}
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

fn query_invocations(conn: &Connection) -> Vec<InvocationSnapshot> {
    let mut stmt = conn
        .prepare(
            "SELECT id, model_name, provider_name, status
             FROM invocations
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok(InvocationSnapshot {
            id: row.get(0)?,
            model_name: row.get(1)?,
            provider_name: row.get(2)?,
            status: row.get(3)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn full_snapshot(path: &Path) -> FullOwnershipSnapshot {
    let conn = Connection::open(path).unwrap();
    FullOwnershipSnapshot {
        chains: query_chains(&conn),
        segments: query_segments_with_id(&conn),
        turns: query_turns_with_id(&conn),
        invocations: query_invocations(&conn),
    }
}

fn query_segments_with_id(conn: &Connection) -> Vec<SegmentRowSnapshot> {
    let mut stmt = conn
        .prepare(
            "SELECT id, chain_id, provider_name, session_id, started_at, ended_at, last_turn_id,
                    transition_reason
             FROM session_chain_segments
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok(SegmentRowSnapshot {
            id: row.get(0)?,
            chain_id: row.get(1)?,
            provider_name: row.get(2)?,
            session_id: row.get(3)?,
            started_at: row.get(4)?,
            ended_at: row.get(5)?,
            last_turn_id: row.get(6)?,
            transition_reason: row.get(7)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn query_turns_with_id(conn: &Connection) -> Vec<TurnRowSnapshot> {
    let source_file_expr = if table_has_column(conn, "session_turns", "source_file") {
        "source_file"
    } else {
        "NULL"
    };
    let body_expr = if table_has_column(conn, "session_turns", "body") {
        "body"
    } else {
        "NULL"
    };
    let mut stmt = conn
        .prepare(&format!(
            "SELECT id, provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
                    is_sidechain, is_compaction_boundary, {source_file_expr}, ingested_at, {body_expr}
             FROM session_turns
             ORDER BY id"
        ))
        .unwrap();
    stmt.query_map([], |row| {
        Ok(TurnRowSnapshot {
            id: row.get(0)?,
            provider_name: row.get(1)?,
            session_id: row.get(2)?,
            turn_id: row.get(3)?,
            timestamp: row.get(4)?,
            role: row.get(5)?,
            parent_turn_id: row.get(6)?,
            is_sidechain: row.get(7)?,
            is_compaction_boundary: row.get(8)?,
            source_file: row.get(9)?,
            ingested_at: row.get(10)?,
            body: row.get(11)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn assert_zero_collision_counts(conn: &Connection) {
    let segment_collisions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (
                 SELECT chain_id, provider_name, session_id
                 FROM session_chain_segments
                 GROUP BY chain_id, provider_name, session_id
                 HAVING COUNT(*) > 1
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let turn_collisions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (
                 SELECT provider_name, session_id, turn_id
                 FROM session_turns
                 GROUP BY provider_name, session_id, turn_id
                 HAVING COUNT(*) > 1
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(segment_collisions, 0, "segment collision groups remain");
    assert_eq!(turn_collisions, 0, "turn collision groups remain");
}

fn latest_segment_id(snapshot: &FullOwnershipSnapshot, chain_id: &str, session_id: &str) -> i64 {
    snapshot
        .segments
        .iter()
        .filter(|segment| segment.chain_id == chain_id && segment.session_id == session_id)
        .max_by(|left, right| {
            left.started_at
                .cmp(&right.started_at)
                .then_with(|| left.id.cmp(&right.id))
        })
        .map(|segment| segment.id)
        .unwrap_or_else(|| panic!("missing segment group {chain_id}/{session_id}"))
}

fn min_turn_id(snapshot: &FullOwnershipSnapshot, session_id: &str, turn_id: &str) -> i64 {
    snapshot
        .turns
        .iter()
        .filter(|turn| turn.session_id == session_id && turn.turn_id == turn_id)
        .map(|turn| turn.id)
        .min()
        .unwrap_or_else(|| panic!("missing turn group {session_id}/{turn_id}"))
}

fn single_turn_by_key<'a>(
    snapshot: &'a FullOwnershipSnapshot,
    session_id: &str,
    turn_id: &str,
) -> &'a TurnRowSnapshot {
    let matching: Vec<_> = snapshot
        .turns
        .iter()
        .filter(|turn| turn.session_id == session_id && turn.turn_id == turn_id)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one turn for {session_id}/{turn_id}: {matching:?}"
    );
    matching[0]
}

fn planned_merged_segment_count(
    snapshot: &FullOwnershipSnapshot,
    chain_id: &str,
    session_id: &str,
) -> i64 {
    let group_count = snapshot
        .segments
        .iter()
        .filter(|segment| segment.chain_id == chain_id && segment.session_id == session_id)
        .count();
    assert!(
        group_count > 0,
        "missing segment group {chain_id}/{session_id}"
    );
    group_count.saturating_sub(1) as i64
}

fn planned_dedup_loser_ids(
    snapshot: &FullOwnershipSnapshot,
    session_id: &str,
    turn_ids: &[&str],
) -> Vec<i64> {
    let mut loser_ids = Vec::new();
    for turn_id in turn_ids {
        let mut group_ids: Vec<_> = snapshot
            .turns
            .iter()
            .filter(|turn| turn.session_id == session_id && turn.turn_id == *turn_id)
            .map(|turn| turn.id)
            .collect();
        assert!(
            !group_ids.is_empty(),
            "missing turn group {session_id}/{turn_id}"
        );
        group_ids.sort_unstable();
        loser_ids.extend(group_ids.into_iter().skip(1));
    }
    loser_ids
}

fn assert_preserved_non_dedup_turns_by_id(
    before: &FullOwnershipSnapshot,
    after: &FullOwnershipSnapshot,
    dedup_loser_ids: &[i64],
) {
    assert_eq!(
        before.turns.len() as i64 - after.turns.len() as i64,
        dedup_loser_ids.len() as i64,
        "turn row delta must equal only planned dedup losers"
    );
    let after_by_id: BTreeMap<_, _> = after.turns.iter().map(|turn| (turn.id, turn)).collect();
    for before_turn in &before.turns {
        if dedup_loser_ids.contains(&before_turn.id) {
            continue;
        }
        let after_turn = after_by_id
            .get(&before_turn.id)
            .unwrap_or_else(|| panic!("non-dedup turn id {} was dropped", before_turn.id));
        assert_eq!(after_turn.id, before_turn.id);
        assert_same_turn_content(after_turn, before_turn);
    }
}

fn first_deleted_segment_id(before: &FullOwnershipSnapshot, after: &FullOwnershipSnapshot) -> i64 {
    before
        .segments
        .iter()
        .find(|segment| {
            !after
                .segments
                .iter()
                .any(|candidate| candidate.id == segment.id)
        })
        .map(|segment| segment.id)
        .unwrap_or_else(|| panic!("no deleted segment id found"))
}

fn first_deleted_turn_id(before: &FullOwnershipSnapshot, after: &FullOwnershipSnapshot) -> i64 {
    before
        .turns
        .iter()
        .find(|turn| !after.turns.iter().any(|candidate| candidate.id == turn.id))
        .map(|turn| turn.id)
        .unwrap_or_else(|| panic!("no deleted turn id found"))
}

fn assert_same_turn_content(actual: &TurnRowSnapshot, expected: &TurnRowSnapshot) {
    assert_eq!(actual.session_id, expected.session_id);
    assert_eq!(actual.turn_id, expected.turn_id);
    assert_eq!(actual.timestamp, expected.timestamp);
    assert_eq!(actual.role, expected.role);
    assert_eq!(actual.parent_turn_id, expected.parent_turn_id);
    assert_eq!(actual.is_sidechain, expected.is_sidechain);
    assert_eq!(
        actual.is_compaction_boundary,
        expected.is_compaction_boundary
    );
    assert_eq!(actual.source_file, expected.source_file);
    assert_eq!(actual.ingested_at, expected.ingested_at);
    assert_eq!(actual.body, expected.body);
}

fn assert_large_unrelated_rows_preserved_with_expected_remap(
    before: &FullOwnershipSnapshot,
    after: &FullOwnershipSnapshot,
) {
    let before_chain = before
        .chains
        .get("chain-large-unrelated")
        .unwrap_or_else(|| panic!("missing unrelated chain before migration"));
    let after_chain = after
        .chains
        .get("chain-large-unrelated")
        .unwrap_or_else(|| panic!("missing unrelated chain after migration"));
    assert_eq!(after_chain.model_name, target_model_name());
    assert_eq!(after_chain.created_at, before_chain.created_at);
    assert_eq!(after_chain.last_used_at, before_chain.last_used_at);

    let before_segment = before
        .segments
        .iter()
        .find(|segment| {
            segment.chain_id == "chain-large-unrelated"
                && segment.session_id == "session-large-unrelated"
        })
        .unwrap_or_else(|| panic!("missing unrelated segment before migration"));
    let after_segment = after
        .segments
        .iter()
        .find(|segment| segment.id == before_segment.id)
        .unwrap_or_else(|| panic!("unrelated segment id {} was dropped", before_segment.id));
    assert_eq!(after_segment.provider_name, canonical_account());
    assert_eq!(after_segment.chain_id, before_segment.chain_id);
    assert_eq!(after_segment.session_id, before_segment.session_id);
    assert_eq!(after_segment.started_at, before_segment.started_at);
    assert_eq!(after_segment.ended_at, before_segment.ended_at);
    assert_eq!(after_segment.last_turn_id, before_segment.last_turn_id);
    assert_eq!(
        after_segment.transition_reason,
        before_segment.transition_reason
    );

    let before_turn = before
        .turns
        .iter()
        .find(|turn| turn.turn_id == "turn-large-unrelated")
        .unwrap_or_else(|| panic!("missing unrelated turn before migration"));
    let after_turn = after
        .turns
        .iter()
        .find(|turn| turn.id == before_turn.id)
        .unwrap_or_else(|| panic!("unrelated turn id {} was dropped", before_turn.id));
    assert_eq!(after_turn.provider_name, canonical_account());
    assert_same_turn_content(after_turn, before_turn);
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

fn assert_turn_owner(
    snapshot: &OwnershipSnapshot,
    expected_session: &str,
    expected_turn: &str,
    expected_provider: &str,
) {
    let turn = snapshot
        .turns
        .iter()
        .find(|turn| turn.session_id == expected_session && turn.turn_id == expected_turn)
        .unwrap_or_else(|| panic!("missing turn {expected_session}/{expected_turn}"));
    assert_eq!(turn.provider_name, expected_provider, "{expected_turn}");
    assert_eq!(turn.session_id, expected_session, "{expected_turn}");
}

fn assert_turns_owned_by(
    snapshot: &FullOwnershipSnapshot,
    expected_session: &str,
    expected_turns: &[&str],
    expected_provider: &str,
) {
    for expected_turn in expected_turns {
        let turn = snapshot
            .turns
            .iter()
            .find(|turn| turn.session_id == expected_session && turn.turn_id == *expected_turn)
            .unwrap_or_else(|| panic!("missing turn {expected_session}/{expected_turn}"));
        assert_eq!(turn.provider_name, expected_provider, "{expected_turn}");
        assert_eq!(turn.session_id, expected_session, "{expected_turn}");
    }
}

fn assert_no_noncanonical_rotation_turns_in_sessions(
    snapshot: &FullOwnershipSnapshot,
    session_ids: &[&str],
) {
    let residual: Vec<_> = snapshot
        .turns
        .iter()
        .filter(|turn| session_ids.contains(&turn.session_id.as_str()))
        .filter(|turn| is_noncanonical_rotation_provider(&turn.provider_name))
        .collect();
    assert!(
        residual.is_empty(),
        "non-canonical rotation turn owners remain: {residual:?}"
    );
}

fn is_noncanonical_rotation_provider(provider_name: &str) -> bool {
    provider_name.contains(&provider_token())
        && provider_name != canonical_account()
        && provider_name != accepted_account()
}

fn assert_surviving_session_ids_preserved(
    before: &FullOwnershipSnapshot,
    after: &FullOwnershipSnapshot,
) {
    let after_segments_by_id: BTreeMap<_, _> = after
        .segments
        .iter()
        .map(|segment| (segment.id, segment))
        .collect();
    for before_segment in &before.segments {
        if let Some(after_segment) = after_segments_by_id.get(&before_segment.id) {
            assert_eq!(after_segment.session_id, before_segment.session_id);
        }
    }

    let after_turns_by_id: BTreeMap<_, _> =
        after.turns.iter().map(|turn| (turn.id, turn)).collect();
    for before_turn in &before.turns {
        if let Some(after_turn) = after_turns_by_id.get(&before_turn.id) {
            assert_eq!(after_turn.session_id, before_turn.session_id);
        }
    }
}

fn assert_preserved_chain_segment_session(
    before: &OwnershipSnapshot,
    after: &OwnershipSnapshot,
    chain_id: &str,
    expected_session: &str,
) {
    assert!(
        before
            .segments
            .iter()
            .any(|segment| segment.chain_id == chain_id && segment.session_id == expected_session),
        "missing pre-apply segment {chain_id}/{expected_session}"
    );
    assert!(
        after
            .segments
            .iter()
            .any(|segment| segment.chain_id == chain_id && segment.session_id == expected_session),
        "missing post-apply segment {chain_id}/{expected_session}"
    );
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
    assert_report_context_count(
        report,
        &FIRST_FORWARD_PHASE,
        "invocation_identity_updates_to_apply",
        0,
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
    assert_report_context_count(
        report,
        &AFTER_IDEMPOTENCE_PHASE,
        "invocation_identity_updates_to_apply",
        0,
    );
}

fn assert_rollback_report_counts(report: &str) {
    assert_report_truthy(report, &ROLLBACK_COPY_PHASE, "restored");
    assert_report_context_count(report, &ROLLBACK_COPY_PHASE, "chain_mismatch", 0);
    assert_report_context_count(report, &ROLLBACK_COPY_PHASE, "segment_mismatch", 0);
    assert_report_context_count(report, &ROLLBACK_COPY_PHASE, "turn_mismatch", 0);
    assert_report_context_count(report, &ROLLBACK_COPY_PHASE, "invocation_mismatch", 0);
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

fn assert_report_truthy_or_zero(report: &str, key: &str) {
    let normalized_key = normalize_report_text(key);
    let line = report
        .lines()
        .find(|line| normalize_report_text(line).contains(&normalized_key))
        .unwrap_or_else(|| panic!("report missing {key}: {report}"));
    let value = line
        .split_once(':')
        .or_else(|| line.split_once('='))
        .map(|(_, value)| value)
        .unwrap_or(line);
    let normalized_value = normalize_report_text(value);
    let truthy = normalized_value
        .split_whitespace()
        .any(|part| part == "true" || part == "yes");
    let zero = extract_last_i64(value) == Some(0);
    assert!(
        truthy || zero,
        "report field {key} was not truthy/zero: {line}\n{report}"
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

fn assert_failure_does_not_mention(output: &Output, scratch_dir: &Path, needle: &str) {
    let combined = combined_output(output) + &read_report_if_present(scratch_dir);
    assert!(
        !combined.contains(needle),
        "failure unexpectedly mentioned {needle}: {combined}"
    );
}

fn assert_failure_mentions_survivor_missing_or_absent(output: &Output, scratch_dir: &Path) {
    let combined = combined_output(output) + &read_report_if_present(scratch_dir);
    assert!(
        combined.contains("survivor")
            && (combined.contains("missing") || combined.contains("absent")),
        "failure did not mention survivor missing/absent: {combined}"
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
