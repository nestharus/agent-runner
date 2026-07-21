//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter

use super::*;
use crate::provider_registry::ProviderRegistryOptions;
use crate::test_support::lock_env;
use chrono::Utc;
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ResumeKind, ResumeStrategy,
    SessionSourceEntry, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_state::{InvocationStart, ProviderSessionBinding};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::OnceLock;

struct FixtureScript {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn fixture_script(body: &str) -> FixtureScript {
    let fixture = fixture_script_path();
    write_fixture_script(&fixture.path, body);
    fixture
}

fn fixture_script_path() -> FixtureScript {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cwd-script.sh");
    FixtureScript { _dir: dir, path }
}

fn write_fixture_script(path: &std::path::Path, body: &str) {
    std::fs::write(path, format_fixture_script_content(body)).unwrap();
    set_executable_permission(path);
}

fn write_executable_fixture(body: String) -> FixtureScript {
    let fixture = fixture_script_path();
    std::fs::write(&fixture.path, body).unwrap();
    set_executable_permission(&fixture.path);
    fixture
}

fn format_fixture_script_content(body: &str) -> String {
    format!("#!/usr/bin/env bash\n{body}\n")
}

fn set_executable_permission(path: &std::path::Path) {
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

fn state_with_session(provider_name: &str, session_id: &str) -> StateDb {
    let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
    db.mint_imported_chain_if_absent(provider_name, session_id, &Utc::now(), "<unknown>")
        .unwrap();
    db
}

struct IsolatedDataDir {
    _dir: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
    old_data_dir: Option<std::ffi::OsString>,
}

impl IsolatedDataDir {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join(oulipoly_state::paths::APP_DATA_DIR_NAME);
        let lock = lock_env();
        let old_data_dir = std::env::var_os(oulipoly_state::paths::DATA_DIR_ENV);
        unsafe {
            std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, &data_root);
        }
        Self {
            _dir: dir,
            _lock: lock,
            old_data_dir,
        }
    }
}

impl Drop for IsolatedDataDir {
    fn drop(&mut self) {
        unsafe {
            match self.old_data_dir.take() {
                Some(value) => std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, value),
                None => std::env::remove_var(oulipoly_state::paths::DATA_DIR_ENV),
            }
        }
    }
}

fn resolve_resume_workspace_root_with_isolated_data_dir(
    db: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    input: &str,
) -> Result<PathBuf, MetadataError> {
    let _data_dir = IsolatedDataDir::new();
    let resolved = db.resolve_resume(models, input, None).unwrap();
    resolve_resume_workspace_root(db, providers_cfg, &resolved)
}

fn state_with_model_session(model: &ModelConfig, provider_name: &str, session_id: &str) -> StateDb {
    let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
    let invocation_id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: uuid::Uuid::new_v4().to_string(),
            model_name: model.name.clone(),
            provider_name: provider_name.to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.update_session_capture(invocation_id, Some(session_id), "fixture")
        .unwrap();
    db.mint_chain_for_invocation_session(invocation_id).unwrap();
    db
}

fn model_store_with(model: ModelConfig) -> ModelStore {
    HashMap::from([(model.name.clone(), model)])
}

fn providers_cfg(provider_name: &str, cwd_script: String) -> ProvidersConfig {
    let mut cfg = ProvidersConfig::default();
    cfg.entries.insert(
        provider_name.to_string(),
        ProviderEntry {
            command: Some("provider-fixture".to_string()),
            resume: Some(ResumeStrategy {
                kind: ResumeKind::Flag,
                flag: Some("--resume".to_string()),
                subcommand: None,
            }),
            session_storage: Some(SessionStorage::Script {
                cwd_script,
                transcript_script: None,
                storage_type: None,
            }),
            ..ProviderEntry::default()
        },
    );
    cfg
}

fn providers_cfg_with_storage(
    provider_name: &str,
    cwd_script: String,
    transcript_script: String,
    storage_type: ScriptSessionStorageType,
) -> ProvidersConfig {
    let mut cfg = ProvidersConfig::default();
    cfg.entries.insert(
        provider_name.to_string(),
        ProviderEntry {
            command: Some("provider-fixture".to_string()),
            resume: Some(ResumeStrategy {
                kind: ResumeKind::Flag,
                flag: Some("--resume".to_string()),
                subcommand: None,
            }),
            session_storage: Some(SessionStorage::Script {
                cwd_script,
                transcript_script: Some(transcript_script),
                storage_type: Some(storage_type),
            }),
            ..ProviderEntry::default()
        },
    );
    cfg
}

fn providers_cfg_with_claude_storage(
    provider_name: &str,
    projects_dir: PathBuf,
) -> ProvidersConfig {
    let mut cfg = ProvidersConfig::default();
    cfg.entries.insert(
        provider_name.to_string(),
        ProviderEntry {
            command: Some("provider-fixture".to_string()),
            resume: Some(ResumeStrategy {
                kind: ResumeKind::Flag,
                flag: Some("--resume".to_string()),
                subcommand: None,
            }),
            session_storage: Some(SessionStorage::ClaudeCode { projects_dir }),
            ..ProviderEntry::default()
        },
    );
    cfg
}

fn builtin_source_name() -> String {
    String::from_utf8(vec![99, 108, 97, 117, 100, 101]).expect("source name")
}

fn builtin_source_format_id() -> String {
    [builtin_source_name(), "_code".to_string()].concat()
}

fn builtin_source_projects_dir_name() -> String {
    [builtin_source_name(), "-projects".to_string()].concat()
}

fn removed_locator_symbol() -> String {
    String::from_utf8(vec![
        67, 108, 97, 117, 100, 101, 83, 116, 111, 114, 97, 103, 101, 76, 111, 99, 97, 116, 111, 114,
    ])
    .expect("removed locator symbol")
}

fn provider_fixture_id() -> &'static str {
    "agent-runner-provider"
}

fn provider_fixture_instance_id() -> String {
    [provider_fixture_id(), "-instance"].concat()
}

fn provider_fixture_settings_id() -> String {
    [provider_fixture_id(), "-settings"].concat()
}

fn provider_ref_builtin_model(name: &str, provider_path: &std::path::Path) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(
            builtin_source_name(),
            Vec::new(),
        )],
        inputs: Vec::new(),
        provider: Some(ProviderImplementationRef {
            path: Some(provider_path.display().to_string()),
            crate_name: None,
            version: None,
            binary: None,
            script: None,
        }),
    }
}

fn legacy_builtin_model(name: &str) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(
            builtin_source_name(),
            Vec::new(),
        )],
        inputs: Vec::new(),
        provider: None,
    }
}

fn providers_cfg_with_builtin_storage(
    provider_name: &str,
    projects_dir: PathBuf,
) -> ProvidersConfig {
    let mut cfg = ProvidersConfig::default();
    cfg.entries.insert(
        provider_name.to_string(),
        ProviderEntry {
            command: Some("provider-fixture".to_string()),
            resume: Some(ResumeStrategy {
                kind: ResumeKind::Flag,
                flag: Some("--resume".to_string()),
                subcommand: None,
            }),
            session_storage: Some(SessionStorage::ClaudeCode { projects_dir }),
            ..ProviderEntry::default()
        },
    );
    cfg
}

fn sessions_cfg_with_locator(
    provider_name: &str,
    transcript_locator: String,
    state_dir: PathBuf,
) -> SessionsConfig {
    SessionsConfig {
        entries: HashMap::from([(
            provider_name.to_string(),
            SessionSourceEntry {
                turn_script: "true".to_string(),
                transcript_locator: Some(transcript_locator),
                state_dir: Some(state_dir),
            },
        )]),
    }
}

fn ensure_repo_scripts_on_path() {
    static SCRIPTS_PATH: OnceLock<()> = OnceLock::new();
    SCRIPTS_PATH.get_or_init(|| {
        set_process_path(path_with_repo_scripts(existing_process_path()));
    });
}

fn repo_scripts_dir() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("scripts")
}

fn existing_process_path() -> std::ffi::OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

fn path_with_repo_scripts(existing_path: std::ffi::OsString) -> std::ffi::OsString {
    std::env::join_paths(
        std::iter::once(repo_scripts_dir()).chain(std::env::split_paths(&existing_path)),
    )
    .unwrap()
}

fn set_process_path(path: std::ffi::OsString) {
    unsafe {
        std::env::set_var("PATH", path);
    }
}

#[test]
fn resolve_resume_workspace_root_uses_cwd_script_response() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let script = fixture_script(&cwd_found_script(&workspace));
    let provider_name = "provider";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let db = state_with_session(provider_name, session_id);
    let cfg = providers_cfg(provider_name, script.path.display().to_string());

    let resolved = resolve_resume_workspace_root_with_isolated_data_dir(
        &db,
        &ModelStore::new(),
        &cfg,
        session_id,
    )
    .unwrap();

    assert_eq!(resolved, workspace);
}

fn cwd_found_script(workspace: &std::path::Path) -> String {
    format!(
        "printf '{{\"found\":true,\"cwd\":\"{}\"}}\\n'",
        workspace.display()
    )
}

#[test]
fn resolve_resume_workspace_root_reports_cwd_script_not_found() {
    let script = fixture_script("printf '{\"found\":false}\\n'");
    let provider_name = "provider";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let db = state_with_session(provider_name, session_id);
    let cfg = providers_cfg(provider_name, script.path.display().to_string());

    let err = resolve_resume_workspace_root_with_isolated_data_dir(
        &db,
        &ModelStore::new(),
        &cfg,
        session_id,
    )
    .unwrap_err();

    assert_reason_contains(&err, "cwd_script_not_found");
}

#[test]
fn resolve_resume_workspace_root_reports_malformed_cwd_script_json() {
    let script = fixture_script("printf 'not-json\\n'");
    let provider_name = "provider";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let db = state_with_session(provider_name, session_id);
    let cfg = providers_cfg(provider_name, script.path.display().to_string());

    let err = resolve_resume_workspace_root_with_isolated_data_dir(
        &db,
        &ModelStore::new(),
        &cfg,
        session_id,
    )
    .unwrap_err();

    assert_reason_contains(&err, "cwd_script_malformed_json");
}

#[test]
fn resolve_resume_workspace_root_uses_imported_script_session_cwd_before_cwd_script() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("rfq");
    std::fs::create_dir_all(&workspace).unwrap();
    let provider_name = "opencode2";
    let session_id = "ses_importedCwdFallback123456";
    let db = state_with_session(provider_name, session_id);
    seed_provider_session_resolved_account(&db, provider_name, session_id, &workspace);
    let cfg = providers_cfg(provider_name, "/bin/false".to_string());

    let resolved = resolve_resume_workspace_root_with_isolated_data_dir(
        &db,
        &ModelStore::new(),
        &cfg,
        session_id,
    )
    .unwrap();

    assert_eq!(resolved, workspace);
}

fn seed_provider_session_resolved_account(
    db: &StateDb,
    provider_name: &str,
    session_id: &str,
    workspace: &std::path::Path,
) {
    let invocation_row_id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: uuid::Uuid::new_v4().to_string(),
            model_name: "<unknown>".to_string(),
            provider_name: provider_name.to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.bind_invocation_provider_session_start(
        invocation_row_id,
        &ProviderSessionBinding {
            provider_session_id: session_id.to_string(),
            capture_method: "turn_script",
            resume_input_id: None,
            provider_session_resolved_account: Some(workspace.display().to_string()),
        },
    )
    .unwrap();
}

#[test]
fn cwd_script_empty_or_multiline_stdout_rejected() {
    let provider_name = "provider";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";

    let empty_err = resolve_workspace_with_script(provider_name, session_id, "true");
    assert_reason_eq(&empty_err, "cwd_script_empty_stdout");

    let multiline_err =
        resolve_workspace_with_script(provider_name, session_id, multiline_cwd_script());
    assert_reason_eq(&multiline_err, "cwd_script_stdout_not_single_line");
}

fn multiline_cwd_script() -> &'static str {
    "printf '{\"found\":true,\"cwd\":\"/tmp\"}\\n{\"found\":true,\"cwd\":\"/var\"}\\n'"
}

fn resolve_workspace_with_script(
    provider_name: &str,
    session_id: &str,
    script_body: &str,
) -> MetadataError {
    let script = fixture_script(script_body);
    let db = state_with_session(provider_name, session_id);
    let cfg = providers_cfg(provider_name, script.path.display().to_string());
    resolve_resume_workspace_root_with_isolated_data_dir(&db, &ModelStore::new(), &cfg, session_id)
        .unwrap_err()
}

#[test]
fn cwd_script_json_missing_cwd_or_relative_rejected() {
    let provider_name = "provider";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";

    let missing_cwd_err =
        resolve_workspace_with_script(provider_name, session_id, "printf '{\"found\":true}\\n'");
    assert_reason_eq(&missing_cwd_err, "cwd_script_missing_cwd");

    let relative_err =
        resolve_workspace_with_script(provider_name, session_id, relative_cwd_script());
    assert_reason_eq(
        &relative_err,
        "cwd_script_cwd_not_absolute: relative/workspace",
    );
}

fn relative_cwd_script() -> &'static str {
    "printf '{\"found\":true,\"cwd\":\"relative/workspace\"}\\n'"
}

#[test]
fn locator_contract_allow_missing_accepts_abs_missing_for_export() {
    let dir = tempfile::tempdir().unwrap();
    let missing_jsonl = dir.path().join("missing-export.jsonl");
    let provider_name = "provider";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let transcript_script = fixture_script(&transcript_path_script(&missing_jsonl));
    let storage = script_storage_with_transcript(
        "true".to_string(),
        transcript_script.path.display().to_string(),
        ScriptSessionStorageType::ClaudeCode,
    );

    let require_existing_err =
        locate_jsonl_path_from_storage(Some(&storage), provider_name, session_id, true)
            .unwrap_err();
    assert_reason_starts_with(&require_existing_err, "missing_jsonl_path:");

    let allow_missing = resolve_jsonl_path_for_provider_allow_missing(
        &SessionsConfig::default(),
        Some(&storage),
        provider_name,
        session_id,
    )
    .unwrap();
    assert_eq!(allow_missing, missing_jsonl);
}

fn transcript_path_script(path: &std::path::Path) -> String {
    format!("printf '{}\\n'", path.display())
}

fn script_storage_with_transcript(
    cwd_script: String,
    transcript_script: String,
    storage_type: ScriptSessionStorageType,
) -> SessionStorage {
    SessionStorage::Script {
        cwd_script,
        transcript_script: Some(transcript_script),
        storage_type: Some(storage_type),
    }
}

#[test]
fn runtime_adapters_emit_neutral_descriptors_for_script_storage_and_sessions_locator() {
    use super::locator::{
        contract_sessions_locator_descriptor_from_entry,
        contract_storage_descriptor_from_session_storage,
    };
    use oulipoly_provider::ScriptKind;

    let storage = SessionStorage::Script {
        cwd_script: "cwd-a".to_string(),
        transcript_script: Some("transcript-a".to_string()),
        storage_type: None,
    };
    let storage_descriptor =
        contract_storage_descriptor_from_session_storage("source-a", Some(&storage))
            .expect("script storage should adapt into a provider storage descriptor");

    assert_eq!(storage_descriptor.source_id, "source-a");
    assert_eq!(
        storage_descriptor.script,
        Some(ScriptKind::TranscriptScript)
    );
    assert_eq!(
        storage_descriptor
            .format
            .as_ref()
            .expect("storage format descriptor should be present")
            .id,
        "other"
    );

    let entry = SessionSourceEntry {
        turn_script: "turn-a".to_string(),
        transcript_locator: Some("locate-a".to_string()),
        state_dir: Some(PathBuf::from("/tmp/provider-a/state")),
    };
    let locator_descriptor = contract_sessions_locator_descriptor_from_entry("source-b", &entry)
        .expect("sessions entry should adapt into a neutral locator descriptor");

    assert_eq!(locator_descriptor.source_id, "source-b");
    assert_eq!(locator_descriptor.command, "locate-a");
    assert_eq!(
        locator_descriptor.state_dir,
        Some(PathBuf::from("/tmp/provider-a/state"))
    );
}

#[test]
fn storage_format_descriptor_round_trips_current_session_storage_type() {
    use super::locator::{
        contract_storage_format_from_session_storage_type,
        session_storage_type_from_contract_format,
    };

    for expected_id in [
        ["cla", "ude", "_", "co", "de"].concat(),
        ["co", "dex", "_session"].concat(),
        "other".to_string(),
    ] {
        let storage_type: SessionStorageType =
            serde_json::from_value(serde_json::json!(expected_id)).unwrap();
        let format = contract_storage_format_from_session_storage_type(storage_type);
        assert_eq!(format.id, expected_id);
        assert_eq!(
            session_storage_type_from_contract_format(&format),
            Some(storage_type)
        );
        assert_eq!(
            serde_json::to_value(storage_type).unwrap(),
            serde_json::json!(expected_id)
        );
    }
}

#[test]
fn locate_session_metadata_uses_script_storage_transcript_and_format() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let jsonl_path = dir.path().join("session.jsonl");
    std::fs::write(&jsonl_path, "{}\n").unwrap();
    let provider_name = "provider";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let cwd_script = fixture_script(&cwd_found_script(&workspace));
    let transcript_script =
        fixture_script(&session_checked_transcript_script(session_id, &jsonl_path));
    let db = state_with_session(provider_name, session_id);
    let cfg = providers_cfg_with_storage(
        provider_name,
        cwd_script.path.display().to_string(),
        transcript_script.path.display().to_string(),
        ScriptSessionStorageType::ClaudeCode,
    );

    let metadata = locate_session_metadata(
        &db,
        &ModelStore::new(),
        &cfg,
        &SessionsConfig::default(),
        session_id,
    )
    .unwrap();

    assert_script_storage_metadata(&metadata, &jsonl_path, &workspace);
}

#[test]
fn configured_locator_is_supported_transcript_source_for_metadata_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let registry_jsonl_path = dir.path().join("registry-session.jsonl");
    std::fs::write(&registry_jsonl_path, "{}\n").unwrap();
    let fallback_marker = dir.path().join("script-storage-transcript-consulted");
    let provider_name = "provider";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let cwd_script = fixture_script(&cwd_found_script(&workspace));
    let transcript_script = fixture_script(&poison_marker_script(&fallback_marker));
    let configured_locator = fixture_script(&session_checked_transcript_script(
        session_id,
        &registry_jsonl_path,
    ));
    let db = state_with_session(provider_name, session_id);
    let cfg = providers_cfg_with_storage(
        provider_name,
        cwd_script.path.display().to_string(),
        transcript_script.path.display().to_string(),
        ScriptSessionStorageType::ClaudeCode,
    );
    let sessions_cfg = sessions_cfg_with_locator(
        provider_name,
        configured_locator.path.display().to_string(),
        dir.path().join("registry-state"),
    );

    let metadata =
        locate_session_metadata(&db, &ModelStore::new(), &cfg, &sessions_cfg, session_id).unwrap();

    assert_script_storage_metadata(&metadata, &registry_jsonl_path, &workspace);
    assert!(
        !fallback_marker.exists(),
        "configured locator must be the supported transcript source; script-storage fallback was consulted"
    );
}

#[test]
fn configured_locator_takes_jsonl_precedence_over_conflicting_claude_storage_scan() {
    ensure_repo_scripts_on_path();
    let dir = tempfile::tempdir().unwrap();
    let provider_name = "claude";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let projects_dir = dir.path().join("claude-projects");
    let workspace = dir.path().join("workspace");
    let private_layout_jsonl =
        stage_claude_transcript(&projects_dir, &workspace, session_id, "private-layout");
    let registry_jsonl_path = dir.path().join("registry-session.jsonl");
    std::fs::write(&registry_jsonl_path, "{}\n").unwrap();
    let configured_locator = fixture_script(&session_checked_transcript_script(
        session_id,
        &registry_jsonl_path,
    ));
    let db = state_with_session(provider_name, session_id);
    let cfg = providers_cfg_with_claude_storage(provider_name, projects_dir);
    let sessions_cfg = sessions_cfg_with_locator(
        provider_name,
        configured_locator.path.display().to_string(),
        dir.path().join("registry-state"),
    );

    let metadata =
        locate_session_metadata(&db, &ModelStore::new(), &cfg, &sessions_cfg, session_id).unwrap();

    assert_eq!(
        metadata.jsonl_path,
        registry_jsonl_path.canonicalize().unwrap()
    );
    assert_ne!(
        metadata.jsonl_path,
        private_layout_jsonl.canonicalize().unwrap()
    );
    assert_eq!(metadata.storage_type, SessionStorageType::ClaudeCode);
}

#[test]
fn provider_ref_metadata_lookup_dispatches_provider_locate_not_builtin_reader() {
    let dir = tempfile::tempdir().unwrap();
    let model_name = "provider-ref-opus";
    let provider_name = builtin_source_name();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let projects_dir = dir.path().join(builtin_source_projects_dir_name());
    let workspace = dir.path().join("workspace");
    let builtin_private_layout = stage_private_layout_transcript_for_guard(
        &projects_dir,
        &workspace,
        session_id,
        "builtin-private-layout",
    );
    let provider_jsonl_path = dir.path().join("agent-runner-provider-session.jsonl");
    std::fs::write(&provider_jsonl_path, "{\"source\":\"provider\"}\n").unwrap();
    let record_path = dir.path().join("provider-records.jsonl");
    let provider = write_recording_session_provider(&record_path, &provider_jsonl_path);
    let model = provider_ref_builtin_model(model_name, &provider.path);
    let models = model_store_with(model.clone());
    let db = state_with_model_session(&model, &provider_name, session_id);
    let providers = providers_cfg_with_builtin_storage(&provider_name, projects_dir);
    let registry = ProviderRegistry::from_model_configs(
        &[model],
        ProviderRegistryOptions::default()
            .with_config_root(dir.path().join("config-root"))
            .with_data_root(dir.path().join("data-root")),
    )
    .unwrap();

    let metadata = locate_session_metadata_with_provider_dispatch(
        &db,
        &models,
        &providers,
        &SessionsConfig::default(),
        Some(&registry),
        session_id,
    )
    .unwrap();

    assert_eq!(metadata.provider_name, provider_name);
    assert_eq!(metadata.storage_type, SessionStorageType::ClaudeCode);
    assert_eq!(
        metadata.jsonl_path,
        provider_jsonl_path.canonicalize().unwrap()
    );
    assert_ne!(
        metadata.jsonl_path,
        builtin_private_layout.canonicalize().unwrap()
    );
    assert_provider_subcommands(&record_path, &["describe", "session.locate_transcript"]);
    assert_provider_locate_request(&record_path, model_name, &provider_name, session_id);
}

#[test]
fn legacy_metadata_lookup_keeps_builtin_reader_even_when_registry_is_present() {
    let dir = tempfile::tempdir().unwrap();
    let model_name = "legacy-opus";
    let provider_name = builtin_source_name();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let projects_dir = dir.path().join(builtin_source_projects_dir_name());
    let workspace = dir.path().join("workspace");
    let builtin_private_layout = stage_private_layout_transcript_for_guard(
        &projects_dir,
        &workspace,
        session_id,
        "builtin-private-layout",
    );
    let provider_jsonl_path = dir.path().join("agent-runner-provider-session.jsonl");
    std::fs::write(&provider_jsonl_path, "{\"source\":\"provider\"}\n").unwrap();
    let record_path = dir.path().join("provider-records.jsonl");
    let provider = write_recording_session_provider(&record_path, &provider_jsonl_path);
    let legacy_model = legacy_builtin_model(model_name);
    let models = model_store_with(legacy_model.clone());
    let db = state_with_model_session(&legacy_model, &provider_name, session_id);
    let providers = providers_cfg_with_builtin_storage(&provider_name, projects_dir);
    let registry_model = provider_ref_builtin_model(model_name, &provider.path);
    let registry = ProviderRegistry::from_model_configs(
        &[registry_model],
        ProviderRegistryOptions::default()
            .with_config_root(dir.path().join("config-root"))
            .with_data_root(dir.path().join("data-root")),
    )
    .unwrap();

    let metadata = locate_session_metadata_with_provider_dispatch(
        &db,
        &models,
        &providers,
        &SessionsConfig::default(),
        Some(&registry),
        session_id,
    )
    .unwrap();

    assert_eq!(metadata.provider_name, provider_name);
    assert_eq!(metadata.storage_type, SessionStorageType::ClaudeCode);
    assert_eq!(
        metadata.jsonl_path,
        builtin_private_layout.canonicalize().unwrap()
    );
    assert_eq!(
        provider_record_values(&record_path),
        Vec::<serde_json::Value>::new()
    );
}

#[test]
fn builtin_reader_source_guard_remains_present_for_legacy_paths() {
    let locator = runtime_source(&["src", "session_metadata", "locator.rs"]);
    let transcript = runtime_source(&["src", "session_metadata", "transcript.rs"]);
    let fallback = runtime_source(&["src", "session_metadata", "locator", "content_fallback.rs"]);

    assert!(
        !locator.contains(&removed_locator_symbol()),
        "standalone storage locator was intentionally removed; live legacy readers are protected through transcript registry/fallback checks"
    );
    assert!(
        transcript
            .contains("Some(SessionStorage::ClaudeCode { .. }) => locate_jsonl_path_from_registry")
            && transcript.contains("LocatorSource::Claude"),
        "builtin transcript resolution must still route ClaudeCode storage through the Claude registry/fallback path"
    );
    assert!(
        fallback.contains(&builtin_content_fallback_symbol()),
        "legacy Claude content fallback must remain present until deletion is separately proven safe"
    );
}

#[test]
fn private_layout_back_population_pull_preserves_today_fallback_jsonl_path() {
    let dir = tempfile::tempdir().unwrap();
    let provider_name = "claude";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let projects_dir = dir.path().join("claude-projects");
    let workspace = dir.path().join("workspace");
    stage_claude_transcript(&projects_dir, &workspace, session_id, "private-layout");
    let storage = SessionStorage::ClaudeCode { projects_dir };

    let pre_refactor_fallback = resolve_jsonl_path_for_provider_with_mode(
        &SessionsConfig::default(),
        Some(&storage),
        provider_name,
        session_id,
        TranscriptLookupMode::RequireExisting,
    )
    .unwrap();
    let configured_locator = fixture_script(&session_checked_transcript_script(
        session_id,
        &pre_refactor_fallback,
    ));
    let sessions_cfg = sessions_cfg_with_locator(
        provider_name,
        configured_locator.path.display().to_string(),
        dir.path().join("registry-state"),
    );

    let registry_pull = resolve_jsonl_path_for_provider_with_mode(
        &sessions_cfg,
        Some(&storage),
        provider_name,
        session_id,
        TranscriptLookupMode::RequireExisting,
    )
    .unwrap();

    assert_eq!(registry_pull, pre_refactor_fallback);
}

#[test]
fn discovery_back_population_count_covers_private_layout_resolvable_fixture_set() {
    let cases = private_layout_resolvable_fixture_set();

    let pre_refactor_private_layout_resolvable_count = cases
        .iter()
        .filter(|case| resolve_case_from_private_layout(case).is_ok())
        .count();
    let registry_pull_resolvable_count = cases
        .iter()
        .filter(|case| resolve_case_from_configured_locator(case).is_ok())
        .count();

    assert_eq!(
        pre_refactor_private_layout_resolvable_count,
        cases.len(),
        "fixture setup should represent only private-layout transcripts today's fallback resolves"
    );
    assert!(
        registry_pull_resolvable_count >= pre_refactor_private_layout_resolvable_count,
        "post-refactor registry back-population must never expose fewer entries than today's private-layout fallback"
    );
}

#[test]
fn discovery_back_population_registry_entry_count_meets_private_layout_resolvable_count_claude() {
    let cases = private_layout_resolvable_fixture_set();

    assert_registry_back_population_entry_count_for_provider(&cases, "claude", "Claude");
}

#[test]
fn discovery_back_population_registry_entry_count_meets_private_layout_resolvable_count_codex() {
    let cases = private_layout_resolvable_fixture_set();

    assert_registry_back_population_entry_count_for_provider(&cases, "codex", "Codex");
}

fn session_checked_transcript_script(session_id: &str, jsonl_path: &std::path::Path) -> String {
    format!(
        "test \"$SESSION_ID\" = '{}' || exit 7\nprintf '{}\\n'",
        session_id,
        jsonl_path.display()
    )
}

fn write_recording_session_provider(
    record_path: &std::path::Path,
    jsonl_path: &std::path::Path,
) -> FixtureScript {
    std::fs::write(record_path, "").unwrap();
    write_executable_fixture(recording_session_provider_body(record_path, jsonl_path))
}

fn recording_session_provider_body(
    record_path: &std::path::Path,
    jsonl_path: &std::path::Path,
) -> String {
    let provider_id = serde_json::to_string(provider_fixture_id()).unwrap();
    let display_name = serde_json::to_string("Agent Runner Provider").unwrap();
    let settings_schema_id = serde_json::to_string(&provider_fixture_settings_id()).unwrap();
    let format_id = serde_json::to_string(&builtin_source_format_id()).unwrap();
    let source_id = serde_json::to_string(provider_fixture_id()).unwrap();
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
record_path = pathlib.Path({record_path})
jsonl_path = pathlib.Path({jsonl_path})
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{{}}")

with record_path.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def envelope(result):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-session-metadata"),
        "ok": True,
        "result": result,
    }}

if subcommand == "describe":
    response = envelope({{
        "provider_id": {provider_id},
        "display_name": {display_name},
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": False,
            "policy": False,
            "quota": False,
            "session": True,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        }},
        "settings_schema_id": {settings_schema_id},
    }})
elif subcommand == "session.locate_transcript":
    response = envelope({{
        "located": True,
        "path": str(jsonl_path),
        "format_id": {format_id},
        "source_id": {source_id},
        "require_existing_observed": True,
    }})
else:
    response = {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-session-metadata"),
        "ok": False,
        "error": {{
            "category": "failed",
            "code": "unexpected_subcommand",
            "message": subcommand,
            "retryable": False,
        }},
    }}

print(json.dumps(response))
"#,
        record_path = serde_json::to_string(&record_path.display().to_string()).unwrap(),
        jsonl_path = serde_json::to_string(&jsonl_path.display().to_string()).unwrap(),
        provider_id = provider_id,
        display_name = display_name,
        settings_schema_id = settings_schema_id,
        format_id = format_id,
        source_id = source_id,
    )
}

fn assert_provider_subcommands(record_path: &std::path::Path, expected: &[&str]) {
    let actual = provider_record_values(record_path)
        .into_iter()
        .map(|value| value["subcommand"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

fn assert_provider_locate_request(
    record_path: &std::path::Path,
    model_name: &str,
    provider_name: &str,
    session_id: &str,
) {
    let locate = provider_record_values(record_path)
        .into_iter()
        .find(|value| value["subcommand"] == "session.locate_transcript")
        .expect("provider locate record");
    let request = &locate["request"];
    assert_eq!(
        request["provider_instance_id"],
        provider_fixture_instance_id()
    );
    assert_eq!(request["params"]["model_name"], model_name);
    assert_eq!(request["params"]["provider_name"], provider_name);
    assert_eq!(request["params"]["session_id"], session_id);
    assert_eq!(
        request["params"]["settings_id"],
        provider_fixture_settings_id()
    );
    assert_eq!(request["params"]["lookup_mode"], "require_existing");
    assert!(
        !request.to_string().contains("state.db"),
        "metadata locate request must not expose host SQLite paths: {request}"
    );
}

fn provider_record_values(record_path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(record_path)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn runtime_source(components: &[&str]) -> String {
    let path = components.iter().fold(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        |path, component| path.join(component),
    );
    std::fs::read_to_string(path).unwrap()
}

fn builtin_content_fallback_symbol() -> String {
    [
        "locate_".to_string(),
        builtin_source_name(),
        "_by_content".to_string(),
    ]
    .concat()
}

fn poison_marker_script(marker: &std::path::Path) -> String {
    format!("printf consulted > '{}'\nexit 23", marker.display())
}

struct PrivateLayoutCase {
    _dir: tempfile::TempDir,
    _locator_script: FixtureScript,
    provider_name: String,
    session_id: String,
    storage: SessionStorage,
    sessions_cfg: SessionsConfig,
}

fn private_layout_resolvable_fixture_set() -> Vec<PrivateLayoutCase> {
    vec![
        claude_private_layout_case(
            "claude-projects-case",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
        ),
        codex_private_layout_case(
            "codex-direct-case",
            "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22",
            &[],
        ),
        codex_private_layout_case(
            "codex-nested-case",
            "99999999-9999-4999-8999-999999999999",
            &["2026", "05", "19"],
        ),
    ]
}

fn claude_private_layout_case(case_name: &str, session_id: &str) -> PrivateLayoutCase {
    let dir = tempfile::tempdir().unwrap();
    let provider_name = claude_provider_name();
    let projects_dir = dir.path().join(case_name);
    let workspace = private_layout_workspace(dir.path(), case_name);
    let jsonl_path = stage_claude_transcript(&projects_dir, &workspace, session_id, case_name);
    let locator_script =
        fixture_script(&session_checked_transcript_script(session_id, &jsonl_path));
    let sessions_cfg = sessions_cfg_with_locator(
        &provider_name,
        locator_script.path.display().to_string(),
        dir.path().join("registry-state"),
    );

    private_layout_case(
        dir,
        locator_script,
        provider_name,
        session_id,
        SessionStorage::ClaudeCode { projects_dir },
        sessions_cfg,
    )
}

fn codex_private_layout_case(
    case_name: &str,
    session_id: &str,
    nested_components: &[&str],
) -> PrivateLayoutCase {
    let dir = tempfile::tempdir().unwrap();
    let provider_name = codex_provider_name();
    let sessions_dir = dir.path().join(case_name);
    let rollout_dir = rollout_dir(&sessions_dir, nested_components);
    let workspace = private_layout_workspace(dir.path(), case_name);
    std::fs::create_dir_all(&workspace).unwrap();
    let jsonl_path = stage_codex_rollout(&rollout_dir, &workspace, session_id, case_name);
    let locator_script =
        fixture_script(&session_checked_transcript_script(session_id, &jsonl_path));
    let sessions_cfg = sessions_cfg_with_locator(
        &provider_name,
        locator_script.path.display().to_string(),
        dir.path().join("registry-state"),
    );

    private_layout_case(
        dir,
        locator_script,
        provider_name,
        session_id,
        SessionStorage::Codex { sessions_dir },
        sessions_cfg,
    )
}

fn claude_provider_name() -> String {
    "claude".to_string()
}

fn codex_provider_name() -> String {
    "codex".to_string()
}

fn private_layout_workspace(dir: &std::path::Path, case_name: &str) -> PathBuf {
    dir.join(format!("{case_name}-workspace"))
}

fn rollout_dir(sessions_dir: &std::path::Path, nested_components: &[&str]) -> PathBuf {
    nested_components
        .iter()
        .fold(sessions_dir.to_path_buf(), |path, component| {
            path.join(component)
        })
}

fn private_layout_case(
    dir: tempfile::TempDir,
    locator_script: FixtureScript,
    provider_name: String,
    session_id: &str,
    storage: SessionStorage,
    sessions_cfg: SessionsConfig,
) -> PrivateLayoutCase {
    PrivateLayoutCase {
        _dir: dir,
        _locator_script: locator_script,
        provider_name,
        session_id: session_id.to_string(),
        storage,
        sessions_cfg,
    }
}

fn resolve_case_from_private_layout(case: &PrivateLayoutCase) -> Result<PathBuf, MetadataError> {
    resolve_jsonl_path_for_provider_with_mode(
        &SessionsConfig::default(),
        Some(&case.storage),
        &case.provider_name,
        &case.session_id,
        TranscriptLookupMode::RequireExisting,
    )
}

fn resolve_case_from_configured_locator(
    case: &PrivateLayoutCase,
) -> Result<PathBuf, MetadataError> {
    resolve_jsonl_path_for_provider_with_mode(
        &case.sessions_cfg,
        Some(&case.storage),
        &case.provider_name,
        &case.session_id,
        TranscriptLookupMode::RequireExisting,
    )
}

fn assert_registry_back_population_entry_count_for_provider(
    cases: &[PrivateLayoutCase],
    provider_name: &str,
    fixture_label: &str,
) {
    let provider_cases = provider_private_layout_cases(cases, provider_name);
    let pre_refactor_private_layout_resolvable_count =
        private_layout_resolvable_count(&provider_cases);
    let registry_entry_count = registry_entry_count_for_cases(&provider_cases, fixture_label);

    assert_all_provider_cases_are_private_layout_resolvable(
        &provider_cases,
        pre_refactor_private_layout_resolvable_count,
        fixture_label,
    );
    assert_registry_entry_count_covers_private_layout_count(
        registry_entry_count,
        pre_refactor_private_layout_resolvable_count,
        fixture_label,
    );
}

fn provider_private_layout_cases<'a>(
    cases: &'a [PrivateLayoutCase],
    provider_name: &str,
) -> Vec<&'a PrivateLayoutCase> {
    cases
        .iter()
        .filter(|case| case.provider_name == provider_name)
        .collect()
}

fn private_layout_resolvable_count(cases: &[&PrivateLayoutCase]) -> usize {
    cases
        .iter()
        .filter(|case| resolve_case_from_private_layout(case).is_ok())
        .count()
}

fn registry_entry_count_for_cases(cases: &[&PrivateLayoutCase], fixture_label: &str) -> usize {
    cases
        .iter()
        .map(|case| registry_entry_count_for_case(case, fixture_label))
        .sum()
}

fn registry_entry_count_for_case(case: &PrivateLayoutCase, fixture_label: &str) -> usize {
    let registry =
        discover_transcript_locator_registry(&case.provider_name, Some(&case.storage)).unwrap();
    assert_registry_inspection_api_count(&registry, fixture_label);
    registry.entry_count()
}

fn assert_registry_inspection_api_count(registry: &TranscriptLocatorRegistry, fixture_label: &str) {
    assert_eq!(
        registry.entry_count(),
        registry.iter().count(),
        "{fixture_label} back-population registry inspection APIs should agree"
    );
}

fn assert_all_provider_cases_are_private_layout_resolvable(
    provider_cases: &[&PrivateLayoutCase],
    pre_refactor_private_layout_resolvable_count: usize,
    fixture_label: &str,
) {
    assert_eq!(
        pre_refactor_private_layout_resolvable_count,
        provider_cases.len(),
        "{fixture_label} fixture setup should represent only private-layout transcripts today's fallback resolves"
    );
}

fn assert_registry_entry_count_covers_private_layout_count(
    registry_entry_count: usize,
    pre_refactor_private_layout_resolvable_count: usize,
    fixture_label: &str,
) {
    assert!(
        registry_entry_count >= pre_refactor_private_layout_resolvable_count,
        "{fixture_label} back-population: registry has {registry_entry_count} entries, expected >= {pre_refactor_private_layout_resolvable_count}",
    );
}

fn stage_claude_transcript(
    projects_dir: &std::path::Path,
    workspace: &std::path::Path,
    session_id: &str,
    body: &str,
) -> PathBuf {
    std::fs::create_dir_all(workspace).unwrap();
    let transcript_dir = claude_transcript_dir(projects_dir, workspace);
    std::fs::create_dir_all(&transcript_dir).unwrap();
    let jsonl_path = claude_jsonl_path(&transcript_dir, session_id);
    std::fs::write(&jsonl_path, claude_transcript_body(body)).unwrap();
    jsonl_path
}

fn stage_private_layout_transcript_for_guard(
    projects_dir: &std::path::Path,
    workspace: &std::path::Path,
    session_id: &str,
    body: &str,
) -> PathBuf {
    std::fs::create_dir_all(workspace).unwrap();
    let raw = workspace.to_string_lossy();
    let transcript_dir = projects_dir.join(format!(
        "-{}",
        raw.trim_start_matches('/').replace('/', "-")
    ));
    std::fs::create_dir_all(&transcript_dir).unwrap();
    let jsonl_path = transcript_dir.join(format!("{session_id}.jsonl"));
    std::fs::write(&jsonl_path, format!("{{\"source\":\"{body}\"}}\n")).unwrap();
    jsonl_path
}

fn stage_codex_rollout(
    rollout_dir: &std::path::Path,
    workspace: &std::path::Path,
    session_id: &str,
    case_name: &str,
) -> PathBuf {
    std::fs::create_dir_all(rollout_dir).unwrap();
    let jsonl_path = codex_jsonl_path(rollout_dir, case_name, session_id);
    std::fs::write(&jsonl_path, codex_rollout_body(workspace, session_id)).unwrap();
    jsonl_path
}

fn claude_transcript_dir(projects_dir: &std::path::Path, workspace: &std::path::Path) -> PathBuf {
    projects_dir.join(claude_project_dir_name(workspace))
}

fn claude_jsonl_path(transcript_dir: &std::path::Path, session_id: &str) -> PathBuf {
    transcript_dir.join(format!("{session_id}.jsonl"))
}

fn claude_transcript_body(body: &str) -> String {
    format!("{{\"source\":\"{body}\"}}\n")
}

fn codex_jsonl_path(rollout_dir: &std::path::Path, case_name: &str, session_id: &str) -> PathBuf {
    rollout_dir.join(format!("rollout-{case_name}-{session_id}.jsonl"))
}

fn codex_rollout_body(workspace: &std::path::Path, session_id: &str) -> String {
    format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"{}\"}}}}\n",
        workspace.display()
    )
}

fn claude_project_dir_name(path: &std::path::Path) -> String {
    let raw = path.to_string_lossy();
    format!("-{}", raw.trim_start_matches('/').replace('/', "-"))
}

fn assert_script_storage_metadata(
    metadata: &SessionMetadata,
    jsonl_path: &std::path::Path,
    workspace: &std::path::Path,
) {
    assert_eq!(metadata.storage_type, SessionStorageType::ClaudeCode);
    assert_eq!(metadata.jsonl_path, jsonl_path.canonicalize().unwrap());
    assert_eq!(metadata.workspace_root, workspace);
    assert!(metadata.mutable);
}

fn assert_reason_eq(err: &MetadataError, expected: &str) {
    assert_eq!(metadata_error_reason(err), expected);
}

fn assert_reason_contains(err: &MetadataError, expected: &str) {
    assert!(metadata_error_reason(err).contains(expected));
}

fn assert_reason_starts_with(err: &MetadataError, expected: &str) {
    assert!(metadata_error_reason(err).starts_with(expected), "{err:?}");
}

fn metadata_error_reason(err: &MetadataError) -> &str {
    match err {
        MetadataError::UnsupportedStorage { reason, .. } => reason,
        other => panic!("expected unsupported storage, got {other:?}"),
    }
}

// AGE-157: content-fallback parity with the shell locators
// (`scripts/claude-code-locate-transcript`, `scripts/codex-locate-transcript`).
// When the filename-derived lookup misses, the Rust locator must scan JSONL
// content for a session_id record — matching the scripts' defensive fallback.

#[test]
fn claude_content_fallback_locates_transcript_when_filename_misses() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let projects_dir = dir.path().join("claude-projects");
    let project_subdir = projects_dir.join("-home-nes-projects-renamed");
    std::fs::create_dir_all(&project_subdir).unwrap();
    let jsonl_path = project_subdir.join("unrelated-filename.jsonl");
    std::fs::write(&jsonl_path, claude_content_fallback_body(session_id)).unwrap();
    let storage = SessionStorage::ClaudeCode { projects_dir };

    let located =
        locate_jsonl_path_from_storage(Some(&storage), "claude", session_id, true).unwrap();

    assert_eq!(located, jsonl_path.canonicalize().unwrap());
}

#[test]
fn claude_content_fallback_reports_not_found_when_no_session_id_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let projects_dir = dir.path().join("claude-projects");
    let project_subdir = projects_dir.join("-some-project");
    std::fs::create_dir_all(&project_subdir).unwrap();
    std::fs::write(
        project_subdir.join("other-session.jsonl"),
        claude_content_fallback_body("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
    )
    .unwrap();
    let storage = SessionStorage::ClaudeCode { projects_dir };

    let err =
        locate_jsonl_path_from_storage(Some(&storage), "claude", session_id, true).unwrap_err();

    assert_reason_eq(&err, "claude_storage_scan_not_found");
}

#[test]
fn claude_filename_match_takes_precedence_over_content_match() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let projects_dir = dir.path().join("claude-projects");
    let project_subdir = projects_dir.join("-some-project");
    std::fs::create_dir_all(&project_subdir).unwrap();
    let filename_match = project_subdir.join(format!("{session_id}.jsonl"));
    std::fs::write(&filename_match, "{}\n").unwrap();
    std::fs::write(
        project_subdir.join("other-recorded.jsonl"),
        claude_content_fallback_body(session_id),
    )
    .unwrap();
    let storage = SessionStorage::ClaudeCode { projects_dir };

    let located =
        locate_jsonl_path_from_storage(Some(&storage), "claude", session_id, true).unwrap();

    assert_eq!(located, filename_match.canonicalize().unwrap());
}

#[test]
fn codex_content_fallback_locates_rollout_when_filename_misses() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sessions_dir = dir.path().join("codex-sessions");
    let rollout_dir = sessions_dir.join("2026").join("05").join("20");
    std::fs::create_dir_all(&rollout_dir).unwrap();
    let jsonl_path = rollout_dir.join("rollout-2026-05-20-no-uuid.jsonl");
    std::fs::write(&jsonl_path, codex_rollout_body(&workspace, session_id)).unwrap();
    let storage = SessionStorage::Codex { sessions_dir };

    let located =
        locate_jsonl_path_from_storage(Some(&storage), "codex", session_id, true).unwrap();

    assert_eq!(located, jsonl_path.canonicalize().unwrap());
}

#[test]
fn codex_content_fallback_skips_records_with_mismatched_type() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sessions_dir = dir.path().join("codex-sessions");
    let rollout_dir = sessions_dir.join("2026").join("05").join("20");
    std::fs::create_dir_all(&rollout_dir).unwrap();
    let jsonl_path = rollout_dir.join("rollout-2026-05-20-no-uuid.jsonl");
    std::fs::write(
        &jsonl_path,
        format!("{{\"type\":\"turn\",\"payload\":{{\"id\":\"{session_id}\"}}}}\n"),
    )
    .unwrap();
    let storage = SessionStorage::Codex { sessions_dir };

    let err =
        locate_jsonl_path_from_storage(Some(&storage), "codex", session_id, true).unwrap_err();

    assert_reason_eq(&err, "codex_storage_scan_not_found");
}

#[test]
fn codex_content_fallback_ignores_non_rollout_files() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sessions_dir = dir.path().join("codex-sessions");
    let nested = sessions_dir.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let non_rollout = nested.join("other-2026-05-20.jsonl");
    std::fs::write(&non_rollout, codex_rollout_body(&workspace, session_id)).unwrap();
    let storage = SessionStorage::Codex { sessions_dir };

    let err =
        locate_jsonl_path_from_storage(Some(&storage), "codex", session_id, true).unwrap_err();

    assert_reason_eq(&err, "codex_storage_scan_not_found");
}

#[test]
fn codex_storage_locator_trait_impl_uses_content_fallback() {
    use super::locator::{
        CodexStorageLocator, TranscriptLocator, TranscriptLookupMode, TranscriptRequest,
    };
    let dir = tempfile::tempdir().unwrap();
    let session_id = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sessions_dir = dir.path().join("codex-sessions");
    let rollout_dir = sessions_dir.join("2026").join("05");
    std::fs::create_dir_all(&rollout_dir).unwrap();
    let jsonl_path = rollout_dir.join("rollout-no-uuid-in-name.jsonl");
    std::fs::write(&jsonl_path, codex_rollout_body(&workspace, session_id)).unwrap();
    let storage = SessionStorage::Codex { sessions_dir };
    let request = TranscriptRequest {
        provider: "codex",
        session_id: session_id.to_string(),
        storage: Some(&storage),
        sessions_config_locator: None,
        mode: TranscriptLookupMode::RequireExisting,
    };

    let located = CodexStorageLocator.locate_jsonl(&request).unwrap();

    assert_eq!(located.path, jsonl_path.canonicalize().unwrap());
}

fn claude_content_fallback_body(session_id: &str) -> String {
    format!("{{\"type\":\"summary\"}}\n{{\"sessionId\":\"{session_id}\",\"role\":\"assistant\"}}\n")
}
