use super::*;
use chrono::Utc;
use oulipoly_config::{ProviderEntry, ResumeKind, ResumeStrategy};
use std::os::unix::fs::PermissionsExt;

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

fn session_checked_transcript_script(session_id: &str, jsonl_path: &std::path::Path) -> String {
    format!(
        "test \"$SESSION_ID\" = '{}' || exit 7\nprintf '{}\\n'",
        session_id,
        jsonl_path.display()
    )
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
