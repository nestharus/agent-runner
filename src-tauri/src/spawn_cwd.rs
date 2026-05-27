//! ## Declared roles
//!
//! - `accessor`: `read_current_dir`, `effective_resume_spawn_cwd` (calls the
//!   `resolve_resume_workspace_root` session-metadata accessor).
//! - `mapper`: `effective_spawn_cwd`, `join_relative_cwd`,
//!   `format_current_dir_error`, `format_resume_spawn_cwd_fallback_warning`.
//! - `formatter`: resume cwd fallback warning emission.
//! - `orchestration`: fallback sequencing for resume spawn cwd resolution.
//! - `validator`: colocated cwd behavior/source-shape tests.

use std::path::{Path, PathBuf};

use oulipoly_config::ProvidersConfig;
use oulipoly_runtime::session_metadata::{MetadataError, resolve_resume_workspace_root};
use oulipoly_state::StateDb;

pub(crate) fn effective_spawn_cwd(working_dir: Option<&Path>) -> Result<PathBuf, String> {
    match working_dir {
        Some(dir) if dir.is_absolute() => Ok(dir.to_path_buf()),
        Some(dir) => Ok(join_relative_cwd(read_current_dir()?, dir)),
        None => read_current_dir(),
    }
}

fn read_current_dir() -> Result<PathBuf, String> {
    std::env::current_dir().map_err(format_current_dir_error)
}

fn join_relative_cwd(current_dir: PathBuf, relative: &Path) -> PathBuf {
    current_dir.join(relative)
}

fn format_current_dir_error(error: std::io::Error) -> String {
    format!("Failed to resolve current directory: {error}")
}

pub(crate) fn effective_resume_spawn_cwd(
    state: &StateDb,
    models: &oulipoly_state::ModelStore,
    providers_cfg: &ProvidersConfig,
    _sessions_cfg: &oulipoly_config::SessionsConfig,
    resume_input: &str,
    working_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let fallback = effective_spawn_cwd(working_dir)?;
    resolve_resume_spawn_cwd_or_fallback(
        resume_workspace_root(state, models, providers_cfg, resume_input),
        resume_input,
        fallback,
    )
}

fn resume_workspace_root(
    state: &StateDb,
    models: &oulipoly_state::ModelStore,
    providers_cfg: &ProvidersConfig,
    resume_input: &str,
) -> Result<PathBuf, MetadataError> {
    resolve_resume_workspace_root(state, models, providers_cfg, resume_input)
}

fn resume_spawn_cwd_fallback(
    resume_input: &str,
    err: &MetadataError,
    fallback: PathBuf,
) -> Result<PathBuf, String> {
    emit_resume_spawn_cwd_fallback_warning(resume_input, err, &fallback);
    Ok(resume_spawn_cwd_fallback_path(fallback))
}

fn resolve_resume_spawn_cwd_or_fallback(
    workspace_root: Result<PathBuf, MetadataError>,
    resume_input: &str,
    fallback: PathBuf,
) -> Result<PathBuf, String> {
    match workspace_root {
        Ok(workspace_root) => Ok(workspace_root),
        Err(err) => resume_spawn_cwd_fallback(resume_input, &err, fallback),
    }
}

fn resume_spawn_cwd_fallback_path(fallback: PathBuf) -> PathBuf {
    fallback
}

fn emit_resume_spawn_cwd_fallback_warning(
    resume_input: &str,
    err: &MetadataError,
    fallback: &Path,
) {
    eprintln!(
        "{}",
        format_resume_spawn_cwd_fallback_warning(resume_input, err, fallback)
    );
}

fn format_resume_spawn_cwd_fallback_warning(
    resume_input: &str,
    err: &MetadataError,
    fallback: &Path,
) -> String {
    format!(
        "[resume] warning: could not resolve original cwd for {resume_input}: {}; using {}",
        crate::commands::session_locate_export::metadata_error_message(err),
        fallback.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Mutex, OnceLock};

    fn cwd_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn providers_cfg_with_cwd_script(
        provider_name: &str,
        cwd_script: impl Into<String>,
    ) -> ProvidersConfig {
        let mut cfg = ProvidersConfig::default();
        cfg.entries.insert(
            provider_name.to_string(),
            oulipoly_config::ProviderEntry {
                command: Some(provider_name.to_string()),
                session_storage: Some(oulipoly_config::SessionStorage::Script {
                    cwd_script: cwd_script.into(),
                    transcript_script: None,
                    storage_type: None,
                }),
                resume: Some(oulipoly_config::ResumeStrategy {
                    kind: oulipoly_config::ResumeKind::Flag,
                    flag: Some("--resume".to_string()),
                    subcommand: None,
                }),
                ..oulipoly_config::ProviderEntry::default()
            },
        );
        cfg
    }

    fn imported_chain_state(db_path: &Path, provider_name: &str, session_id: &str) -> StateDb {
        let state = StateDb::open(db_path).unwrap();
        let started_at = chrono::DateTime::parse_from_rfc3339("2026-04-17T08:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        state
            .mint_imported_chain_if_absent(provider_name, session_id, &started_at, "<unknown>")
            .unwrap();
        state
    }

    fn with_deleted_current_dir(test: impl FnOnce()) {
        let _guard = cwd_lock().lock().unwrap();
        let original = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        drop(dir);

        let result = catch_unwind(AssertUnwindSafe(test));

        std::env::set_current_dir(original).unwrap();
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    // Characterization test for AGE-8 — pins current behavior of effective_spawn_cwd CLI adapter.
    #[test]
    fn effective_spawn_cwd_keeps_absolute_paths_and_absolutizes_relative_paths() {
        let _guard = cwd_lock().lock().unwrap();
        let cwd = std::env::current_dir().unwrap();

        assert_eq!(
            effective_spawn_cwd(Some(Path::new("/tmp/age8-project"))).unwrap(),
            PathBuf::from("/tmp/age8-project")
        );
        assert_eq!(
            effective_spawn_cwd(Some(Path::new("relative-project"))).unwrap(),
            cwd.join("relative-project")
        );
        assert_eq!(effective_spawn_cwd(None).unwrap(), cwd);
    }

    // Characterization test for AGE-8 — pins current behavior of effective_spawn_cwd error handling.
    #[cfg(unix)]
    #[test]
    fn effective_spawn_cwd_reports_current_dir_resolution_errors() {
        with_deleted_current_dir(|| {
            let err = effective_spawn_cwd(Some(Path::new("relative-project"))).unwrap_err();
            assert!(
                err.starts_with("Failed to resolve current directory:"),
                "{err}"
            );
        });
    }

    #[test]
    fn effective_resume_spawn_cwd_prefers_workspace_from_cwd_script() {
        let dir = tempfile::tempdir().unwrap();
        let provider_name = "provider6";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let workspace = dir.path().join("workspace");
        let caller_cwd = dir.path().join("caller");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&caller_cwd).unwrap();

        let state = imported_chain_state(&dir.path().join("state.db"), provider_name, session_id);
        let models = oulipoly_state::ModelStore::new();
        let providers_cfg = providers_cfg_with_cwd_script(
            provider_name,
            format!(
                "printf '{{\"found\":true,\"cwd\":\"{}\"}}\\n'",
                workspace.display()
            ),
        );
        let sessions_cfg = oulipoly_config::SessionsConfig::default();

        let cwd = effective_resume_spawn_cwd(
            &state,
            &models,
            &providers_cfg,
            &sessions_cfg,
            session_id,
            Some(&caller_cwd),
        )
        .unwrap();

        assert_eq!(cwd, workspace);
    }

    #[test]
    fn effective_resume_spawn_cwd_falls_back_when_cwd_script_reports_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let provider_name = "provider6";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let fallback = dir.path().join("caller");
        std::fs::create_dir_all(&fallback).unwrap();

        let state = imported_chain_state(&dir.path().join("state.db"), provider_name, session_id);
        let models = oulipoly_state::ModelStore::new();
        let providers_cfg =
            providers_cfg_with_cwd_script(provider_name, "printf '{\"found\":false}\\n'");
        let sessions_cfg = oulipoly_config::SessionsConfig::default();

        let cwd = effective_resume_spawn_cwd(
            &state,
            &models,
            &providers_cfg,
            &sessions_cfg,
            session_id,
            Some(&fallback),
        )
        .unwrap();

        assert_eq!(cwd, fallback);
    }

    #[test]
    fn effective_resume_spawn_cwd_falls_back_when_cwd_script_returns_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        let provider_name = "provider6";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let fallback = dir.path().join("caller");
        std::fs::create_dir_all(&fallback).unwrap();

        let state = imported_chain_state(&dir.path().join("state.db"), provider_name, session_id);
        let models = oulipoly_state::ModelStore::new();
        let providers_cfg = providers_cfg_with_cwd_script(provider_name, "printf 'not-json\\n'");
        let sessions_cfg = oulipoly_config::SessionsConfig::default();

        let cwd = effective_resume_spawn_cwd(
            &state,
            &models,
            &providers_cfg,
            &sessions_cfg,
            session_id,
            Some(&fallback),
        )
        .unwrap();

        assert_eq!(cwd, fallback);
    }
}
