use super::*;
use chrono::Utc;
use oulipoly_config::{ProviderEntry, ResumeKind, ResumeStrategy, SessionSourceEntry};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::OnceLock;

struct FixtureScript {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn fixture_script(body: &str) -> FixtureScript {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cwd-script.sh");
    std::fs::write(&path, format_fixture_script_content(body)).unwrap();
    set_executable_permission(&path);
    FixtureScript { _dir: dir, path }
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
        let scripts_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("scripts");
        let existing_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(scripts_dir).chain(std::env::split_paths(&existing_path)),
        )
        .unwrap();
        unsafe {
            std::env::set_var("PATH", path);
        }
    });
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

    let resolved =
        resolve_resume_workspace_root(&db, &ModelStore::new(), &cfg, session_id).unwrap();

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

    let err = resolve_resume_workspace_root(&db, &ModelStore::new(), &cfg, session_id).unwrap_err();

    assert_reason_contains(&err, "cwd_script_not_found");
}

#[test]
fn resolve_resume_workspace_root_reports_malformed_cwd_script_json() {
    let script = fixture_script("printf 'not-json\\n'");
    let provider_name = "provider";
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let db = state_with_session(provider_name, session_id);
    let cfg = providers_cfg(provider_name, script.path.display().to_string());

    let err = resolve_resume_workspace_root(&db, &ModelStore::new(), &cfg, session_id).unwrap_err();

    assert_reason_contains(&err, "cwd_script_malformed_json");
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
    resolve_resume_workspace_root(&db, &ModelStore::new(), &cfg, session_id).unwrap_err()
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
    let provider_name = "claude".to_string();
    let projects_dir = dir.path().join(case_name);
    let workspace = dir.path().join(format!("{case_name}-workspace"));
    let jsonl_path = stage_claude_transcript(&projects_dir, &workspace, session_id, case_name);
    let locator_script =
        fixture_script(&session_checked_transcript_script(session_id, &jsonl_path));
    let sessions_cfg = sessions_cfg_with_locator(
        &provider_name,
        locator_script.path.display().to_string(),
        dir.path().join("registry-state"),
    );

    PrivateLayoutCase {
        _dir: dir,
        _locator_script: locator_script,
        provider_name,
        session_id: session_id.to_string(),
        storage: SessionStorage::ClaudeCode { projects_dir },
        sessions_cfg,
    }
}

fn codex_private_layout_case(
    case_name: &str,
    session_id: &str,
    nested_components: &[&str],
) -> PrivateLayoutCase {
    let dir = tempfile::tempdir().unwrap();
    let provider_name = "codex".to_string();
    let sessions_dir = dir.path().join(case_name);
    let rollout_dir = nested_components
        .iter()
        .fold(sessions_dir.clone(), |path, component| path.join(component));
    let workspace = dir.path().join(format!("{case_name}-workspace"));
    std::fs::create_dir_all(&workspace).unwrap();
    let jsonl_path = stage_codex_rollout(&rollout_dir, &workspace, session_id, case_name);
    let locator_script =
        fixture_script(&session_checked_transcript_script(session_id, &jsonl_path));
    let sessions_cfg = sessions_cfg_with_locator(
        &provider_name,
        locator_script.path.display().to_string(),
        dir.path().join("registry-state"),
    );

    PrivateLayoutCase {
        _dir: dir,
        _locator_script: locator_script,
        provider_name,
        session_id: session_id.to_string(),
        storage: SessionStorage::Codex { sessions_dir },
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
    let provider_cases = cases
        .iter()
        .filter(|case| case.provider_name == provider_name)
        .collect::<Vec<_>>();
    let pre_refactor_private_layout_resolvable_count = provider_cases
        .iter()
        .filter(|case| resolve_case_from_private_layout(case).is_ok())
        .count();
    let registry_entry_count = provider_cases
        .iter()
        .map(|case| {
            let registry =
                discover_transcript_locator_registry(&case.provider_name, Some(&case.storage))
                    .unwrap();
            assert_eq!(
                registry.entry_count(),
                registry.iter().count(),
                "{fixture_label} back-population registry inspection APIs should agree"
            );
            registry.entry_count()
        })
        .sum::<usize>();

    assert_eq!(
        pre_refactor_private_layout_resolvable_count,
        provider_cases.len(),
        "{fixture_label} fixture setup should represent only private-layout transcripts today's fallback resolves"
    );
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
    let transcript_dir = projects_dir.join(claude_project_dir_name(workspace));
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
    let jsonl_path = rollout_dir.join(format!("rollout-{case_name}-{session_id}.jsonl"));
    std::fs::write(
        &jsonl_path,
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"{}\"}}}}\n",
            workspace.display()
        ),
    )
    .unwrap();
    jsonl_path
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
