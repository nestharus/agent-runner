//! ## Declared roles
//! orchestration, accessor, formatter, mapper, validator, predicate
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/tests/age219_quota_q2_characterization.rs
//!     role: intrinsic-surface
//!     Domain: quota_q2_routing_characterization_harness
//!     Owns:
//!       - PATH, OULIPOLY_DATA_HOME, and OULIPOLY_DATA_DIR env override isolation under a process mutex
//!       - provider/session config and StateDb fixture construction
//!       - quota/usage shell-script fixture formatting and shell-word quoting
//!       - quota refresh routing/source/auth-refresh behavior assertions
//! ```

use oulipoly_config::{
    ProviderEntry, ProvidersConfig, SessionSourceEntry, SessionStorage, SessionsConfig,
};
use oulipoly_runtime::quota::{
    InFlight, RefreshOutcome, has_refresh_source, refresh_provider, refresh_provider_for_routing,
};
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct PathOverride {
    previous: Option<OsString>,
}

impl PathOverride {
    fn prepend(dir: &Path) -> Self {
        let previous = std::env::var_os("PATH");
        let mut paths = vec![dir.to_path_buf()];
        if let Some(existing) = previous.as_ref() {
            paths.extend(std::env::split_paths(existing));
        }
        let joined = std::env::join_paths(paths).expect("join PATH");
        // SAFETY: tests that mutate PATH hold ENV_LOCK for the whole override,
        // so this process-global environment mutation is serialized locally.
        unsafe {
            std::env::set_var("PATH", joined);
        }
        Self { previous }
    }
}

impl Drop for PathOverride {
    fn drop(&mut self) {
        // SAFETY: PathOverride is only constructed while ENV_LOCK is held, and
        // the guard outlives the override in every test below.
        unsafe {
            match &self.previous {
                Some(previous) => std::env::set_var("PATH", previous),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}

/// Points the required data-directory variables at a fresh tempdir so the per-account
/// auth-refresh single-flight lock's freshness stamp starts empty for this
/// test (otherwise a reused provider key would coalesce one auth test's
/// shell-out against another's). Caller MUST hold `env_lock()` for the guard's
/// lifetime.
struct DataHomeOverride {
    _home: tempfile::TempDir,
    previous_home: Option<OsString>,
    previous_data_dir: Option<OsString>,
}

impl DataHomeOverride {
    fn set() -> Self {
        let home = tempfile::tempdir().expect("data home tempdir");
        let previous_home = std::env::var_os("OULIPOLY_DATA_HOME");
        let previous_data_dir = std::env::var_os("OULIPOLY_DATA_DIR");
        // SAFETY: callers hold ENV_LOCK for the override's lifetime, serializing
        // this process-global mutation with every other env-mutating test here.
        unsafe {
            std::env::set_var("OULIPOLY_DATA_DIR", home.path());
            std::env::set_var("OULIPOLY_DATA_HOME", home.path());
        }
        Self {
            _home: home,
            previous_home,
            previous_data_dir,
        }
    }
}

impl Drop for DataHomeOverride {
    fn drop(&mut self) {
        // SAFETY: the owning test still holds ENV_LOCK until after this restore.
        unsafe {
            match &self.previous_home {
                Some(previous) => std::env::set_var("OULIPOLY_DATA_HOME", previous),
                None => std::env::remove_var("OULIPOLY_DATA_HOME"),
            }
            match &self.previous_data_dir {
                Some(previous) => std::env::set_var("OULIPOLY_DATA_DIR", previous),
                None => std::env::remove_var("OULIPOLY_DATA_DIR"),
            }
        }
    }
}

fn state() -> StateDb {
    StateDb::open(Path::new(":memory:")).expect("state")
}

fn provider_with(entry: ProviderEntry) -> ProvidersConfig {
    providers_with_entries([("quota-provider", entry)])
}

fn providers_with_entries<const N: usize>(entries: [(&str, ProviderEntry); N]) -> ProvidersConfig {
    ProvidersConfig {
        entries: entries
            .into_iter()
            .map(|(name, entry)| (name.to_string(), entry))
            .collect(),
    }
}

fn sessions_with_entries<const N: usize>(
    entries: [(&str, SessionSourceEntry); N],
) -> SessionsConfig {
    SessionsConfig {
        entries: entries
            .into_iter()
            .map(|(name, entry)| (name.to_string(), entry))
            .collect(),
    }
}

fn session_entry(turn_script: String) -> SessionSourceEntry {
    SessionSourceEntry {
        turn_script,
        transcript_locator: None,
        state_dir: None,
    }
}

fn claude_storage(projects_dir: PathBuf) -> SessionStorage {
    SessionStorage::ClaudeCode { projects_dir }
}

fn codex_storage(sessions_dir: PathBuf) -> SessionStorage {
    SessionStorage::Codex { sessions_dir }
}

fn assert_updated_percent(outcome: RefreshOutcome, expected: f64) {
    match outcome {
        RefreshOutcome::Updated { windows } => {
            assert_eq!(windows.len(), 1);
            assert!((windows[0].used_percent - expected).abs() < 1e-6);
        }
        other => panic!("expected Updated, got {other:?}"),
    }
}

fn assert_updated_empty(outcome: RefreshOutcome) {
    match outcome {
        RefreshOutcome::Updated { windows } => assert!(windows.is_empty()),
        other => panic!("expected empty Updated, got {other:?}"),
    }
}

fn assert_failed_contains(outcome: RefreshOutcome, expected: &str) {
    match outcome {
        RefreshOutcome::Failed(message) => assert!(
            message.contains(expected),
            "expected failure message to contain {expected:?}, got {message:?}"
        ),
        other => panic!("expected Failed, got {other:?}"),
    }
}

fn seed_prior_windows(state: &StateDb, provider: &str) {
    state
        .upsert_quota_refresh(
            provider,
            &[QuotaWindowInput {
                used_percent: 0.50,
                resets_at: chrono::Utc::now() + chrono::Duration::hours(24),
            }],
        )
        .expect("seed prior quota windows");
}

fn shell_word(input: &str) -> String {
    if input
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~'))
    {
        return input.to_string();
    }
    format!("'{}'", input.replace('\'', r#"'\''"#))
}

fn double_quoted_shell_word(input: &str) -> String {
    format!("\"{}\"", input.replace('\\', r"\\").replace('"', "\\\""))
}

fn script_quote(path: &Path) -> String {
    shell_word(&path.display().to_string())
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("script metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod script");
    }
}

fn install_usage_script(bin_dir: &Path, name: &str, log: &Path, used_percent: u32) {
    let path = bin_dir.join(name);
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" > {}\nprintf '{{\"windows\":[{{\"used_percent\":{},\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}\\n'\n",
            script_quote(log),
            used_percent
        ),
    )
    .expect("write usage script");
    make_executable(&path);
}

fn install_empty_usage_script(bin_dir: &Path, name: &str, log: &Path) {
    let path = bin_dir.join(name);
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> {}\nprintf '{{\"windows\":[]}}\\n'\n",
            script_quote(log)
        ),
    )
    .expect("write empty usage script");
    make_executable(&path);
}

fn read_trimmed(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
        .trim()
        .to_string()
}

#[test]
fn refresh_provider_for_routing_is_publicly_importable_from_quota_facade() {
    let _public_path: fn(
        &str,
        &ProvidersConfig,
        &SessionsConfig,
        &InFlight,
        &StateDb,
    ) -> RefreshOutcome = refresh_provider_for_routing;
}

#[test]
fn routing_refresh_prefers_explicit_non_empty_quota_script_over_provider_and_legacy_sources() {
    let _env_guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin dir");
    let provider_log = dir.path().join("provider-derived.log");
    let legacy_log = dir.path().join("legacy-derived.log");
    install_usage_script(&bin_dir, "anthropic-usage", &provider_log, 24);
    install_usage_script(&bin_dir, "chatgpt-usage", &legacy_log, 35);
    let _path = PathOverride::prepend(&bin_dir);

    let claude_projects = dir.path().join("provider-account").join("projects");
    let codex_sessions = dir.path().join("legacy account").join("sessions");
    let providers = provider_with(ProviderEntry {
        quota_script: Some(
            "echo '{\"windows\":[{\"used_percent\":13,\"resets_at\":\"2099-01-01T00:00:00Z\"}]}'"
                .to_string(),
        ),
        session_storage: Some(claude_storage(claude_projects)),
        ..ProviderEntry::default()
    });
    let sessions = sessions_with_entries([(
        "quota-provider",
        session_entry(format!(
            "codex-turns {}",
            shell_word(&codex_sessions.display().to_string())
        )),
    )]);

    let outcome = refresh_provider_for_routing(
        "quota-provider",
        &providers,
        &sessions,
        &InFlight::new(),
        &state(),
    );

    assert_updated_percent(outcome, 0.13);
    assert!(
        !provider_log.exists(),
        "provider session_storage fallback must not run when explicit script is non-empty"
    );
    assert!(
        !legacy_log.exists(),
        "legacy sessions.toml fallback must not run when explicit script is non-empty"
    );
}

#[test]
fn whitespace_quota_script_falls_through_to_provider_session_storage_source() {
    let _env_guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin dir");
    let arg_log = dir.path().join("anthropic-arg.log");
    install_usage_script(&bin_dir, "anthropic-usage", &arg_log, 41);
    let _path = PathOverride::prepend(&bin_dir);

    let account_root = dir.path().join("claude-account");
    let projects_dir = account_root.join("projects");
    let providers = provider_with(ProviderEntry {
        quota_script: Some(" \n\t ".to_string()),
        session_storage: Some(claude_storage(projects_dir)),
        ..ProviderEntry::default()
    });

    let outcome = refresh_provider_for_routing(
        "quota-provider",
        &providers,
        &SessionsConfig::default(),
        &InFlight::new(),
        &state(),
    );

    assert_updated_percent(outcome, 0.41);
    assert_eq!(
        read_trimmed(&arg_log),
        account_root.join(".credentials.json").display().to_string()
    );
}

#[test]
fn provider_session_storage_fallback_derives_and_executes_claude_and_codex_quota_scripts() {
    let _env_guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin dir");
    let claude_log = dir.path().join("claude-arg.log");
    let codex_log = dir.path().join("codex-arg.log");
    install_usage_script(&bin_dir, "anthropic-usage", &claude_log, 12);
    install_usage_script(&bin_dir, "chatgpt-usage", &codex_log, 34);
    let _path = PathOverride::prepend(&bin_dir);

    let claude_root = dir.path().join("claude-account");
    let codex_root = dir.path().join("codex-account");
    let providers = providers_with_entries([
        (
            "claude-provider",
            ProviderEntry {
                session_storage: Some(claude_storage(claude_root.join("projects"))),
                ..ProviderEntry::default()
            },
        ),
        (
            "codex-provider",
            ProviderEntry {
                session_storage: Some(codex_storage(codex_root.join("sessions"))),
                ..ProviderEntry::default()
            },
        ),
    ]);

    assert_updated_percent(
        refresh_provider_for_routing(
            "claude-provider",
            &providers,
            &SessionsConfig::default(),
            &InFlight::new(),
            &state(),
        ),
        0.12,
    );
    assert_updated_percent(
        refresh_provider_for_routing(
            "codex-provider",
            &providers,
            &SessionsConfig::default(),
            &InFlight::new(),
            &state(),
        ),
        0.34,
    );
    assert_eq!(
        read_trimmed(&claude_log),
        claude_root.join(".credentials.json").display().to_string()
    );
    assert_eq!(
        read_trimmed(&codex_log),
        codex_root.join("auth.json").display().to_string()
    );
}

#[test]
fn legacy_sessions_fallback_derives_and_executes_claude_and_codex_quota_scripts() {
    let _env_guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin dir");
    let claude_log = dir.path().join("legacy-claude-arg.log");
    let codex_log = dir.path().join("legacy-codex-arg.log");
    install_usage_script(&bin_dir, "anthropic-usage", &claude_log, 22);
    install_usage_script(&bin_dir, "chatgpt-usage", &codex_log, 44);
    let _path = PathOverride::prepend(&bin_dir);

    let claude_root = dir.path().join("legacy-claude");
    let codex_root = dir.path().join("legacy-codex");
    let providers = providers_with_entries([
        ("claude-provider", ProviderEntry::default()),
        ("codex-provider", ProviderEntry::default()),
    ]);
    let sessions = sessions_with_entries([
        (
            "claude-provider",
            session_entry(format!(
                "claude-code-turns {}",
                shell_word(&claude_root.join("projects").display().to_string())
            )),
        ),
        (
            "codex-provider",
            session_entry(format!(
                "codex-cwd {}",
                shell_word(&codex_root.join("sessions").display().to_string())
            )),
        ),
    ]);

    assert_updated_percent(
        refresh_provider_for_routing(
            "claude-provider",
            &providers,
            &sessions,
            &InFlight::new(),
            &state(),
        ),
        0.22,
    );
    assert_updated_percent(
        refresh_provider_for_routing(
            "codex-provider",
            &providers,
            &sessions,
            &InFlight::new(),
            &state(),
        ),
        0.44,
    );
    assert_eq!(
        read_trimmed(&claude_log),
        claude_root.join(".credentials.json").display().to_string()
    );
    assert_eq!(
        read_trimmed(&codex_log),
        codex_root.join("auth.json").display().to_string()
    );
}

#[test]
fn auth_refresh_command_is_preserved_for_explicit_provider_storage_and_legacy_sources() {
    let _env_guard = env_lock();
    let _data_home = DataHomeOverride::set();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin dir");
    let anthropic_log = dir.path().join("anthropic-empty.log");
    let chatgpt_log = dir.path().join("chatgpt-empty.log");
    install_empty_usage_script(&bin_dir, "anthropic-usage", &anthropic_log);
    install_empty_usage_script(&bin_dir, "chatgpt-usage", &chatgpt_log);
    let _path = PathOverride::prepend(&bin_dir);

    let explicit_marker = dir.path().join("explicit-refresh");
    let storage_marker = dir.path().join("storage-refresh");
    let legacy_marker = dir.path().join("legacy-refresh");
    let claude_root = dir.path().join("storage-claude");
    let codex_root = dir.path().join("legacy-codex");
    let providers = providers_with_entries([
        (
            "explicit-provider",
            ProviderEntry {
                quota_script: Some("echo '{\"windows\":[]}'".to_string()),
                auth_refresh_command: Some(format!("touch {}", script_quote(&explicit_marker))),
                ..ProviderEntry::default()
            },
        ),
        (
            "storage-provider",
            ProviderEntry {
                session_storage: Some(claude_storage(claude_root.join("projects"))),
                auth_refresh_command: Some(format!("touch {}", script_quote(&storage_marker))),
                ..ProviderEntry::default()
            },
        ),
        (
            "legacy-provider",
            ProviderEntry {
                auth_refresh_command: Some(format!("touch {}", script_quote(&legacy_marker))),
                ..ProviderEntry::default()
            },
        ),
    ]);
    let sessions = sessions_with_entries([(
        "legacy-provider",
        session_entry(format!(
            "codex-turns {}",
            shell_word(&codex_root.join("sessions").display().to_string())
        )),
    )]);
    let state = state();
    for provider in ["explicit-provider", "storage-provider", "legacy-provider"] {
        seed_prior_windows(&state, provider);
    }

    for provider in ["explicit-provider", "storage-provider", "legacy-provider"] {
        assert_updated_empty(refresh_provider_for_routing(
            provider,
            &providers,
            &sessions,
            &InFlight::new(),
            &state,
        ));
    }

    assert!(explicit_marker.exists());
    assert!(storage_marker.exists());
    assert!(legacy_marker.exists());
}

#[test]
fn has_refresh_source_matrix_covers_explicit_derived_unsupported_missing_and_empty_sources() {
    let explicit = provider_with(ProviderEntry {
        quota_script: Some("echo '{\"windows\":[]}'".to_string()),
        ..ProviderEntry::default()
    });
    assert!(has_refresh_source(
        "quota-provider",
        &explicit,
        &SessionsConfig::default()
    ));

    let provider_derived = provider_with(ProviderEntry {
        session_storage: Some(claude_storage(PathBuf::from("/tmp/claude/projects"))),
        ..ProviderEntry::default()
    });
    assert!(has_refresh_source(
        "quota-provider",
        &provider_derived,
        &SessionsConfig::default()
    ));

    let legacy_derived = SessionsConfig {
        entries: HashMap::from([(
            "quota-provider".to_string(),
            session_entry("codex-turns /tmp/codex/sessions".to_string()),
        )]),
    };
    assert!(has_refresh_source(
        "quota-provider",
        &ProvidersConfig::default(),
        &legacy_derived
    ));

    let unsupported = provider_with(ProviderEntry {
        session_storage: Some(SessionStorage::Script {
            cwd_script: "custom-turns /tmp/custom/sessions".to_string(),
            transcript_script: None,
            storage_type: None,
        }),
        ..ProviderEntry::default()
    });
    assert!(!has_refresh_source(
        "quota-provider",
        &unsupported,
        &SessionsConfig::default()
    ));

    let whitespace_only = provider_with(ProviderEntry {
        quota_script: Some(" \n\t ".to_string()),
        ..ProviderEntry::default()
    });
    assert!(!has_refresh_source(
        "quota-provider",
        &whitespace_only,
        &SessionsConfig::default()
    ));

    assert!(!has_refresh_source(
        "missing-provider",
        &ProvidersConfig::default(),
        &SessionsConfig::default()
    ));
    assert!(!has_refresh_source(
        "quota-provider",
        &ProvidersConfig::default(),
        &SessionsConfig::default()
    ));
}

#[test]
fn auth_refresh_command_failure_is_non_fatal_when_retried_quota_script_succeeds() {
    let _env_guard = env_lock();
    let _data_home = DataHomeOverride::set();
    let dir = tempfile::tempdir().expect("tempdir");
    let first_run_marker = dir.path().join("first-run");
    let quota_script = format!(
        "if [ -e {marker} ]; then echo '{{\"windows\":[{{\"used_percent\":66,\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'; else touch {marker}; printf 'quota denied\\n' >&2; exit 7; fi",
        marker = script_quote(&first_run_marker)
    );
    let providers = provider_with(ProviderEntry {
        quota_script: Some(quota_script),
        auth_refresh_command: Some("printf 'refresh denied\\n' >&2; exit 9".to_string()),
        ..ProviderEntry::default()
    });

    let outcome = refresh_provider_for_routing(
        "quota-provider",
        &providers,
        &SessionsConfig::default(),
        &InFlight::new(),
        &state(),
    );

    assert_updated_percent(outcome, 0.66);
}

#[test]
fn auth_refresh_retries_empty_windows_when_prior_windows_exist() {
    let _env_guard = env_lock();
    let _data_home = DataHomeOverride::set();
    let dir = tempfile::tempdir().expect("tempdir");
    let refreshed_marker = dir.path().join("refreshed");
    let quota_script = format!(
        "if [ -e {marker} ]; then echo '{{\"windows\":[{{\"used_percent\":7,\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'; else echo '{{\"windows\":[]}}'; fi",
        marker = script_quote(&refreshed_marker)
    );
    let providers = provider_with(ProviderEntry {
        quota_script: Some(quota_script),
        auth_refresh_command: Some(format!("touch {}", script_quote(&refreshed_marker))),
        ..ProviderEntry::default()
    });
    let state = state();
    seed_prior_windows(&state, "quota-provider");

    let outcome = refresh_provider("quota-provider", &providers, &InFlight::new(), &state);

    assert_updated_percent(outcome, 0.07);
    assert!(
        refreshed_marker.exists(),
        "auth_refresh_command must run for empty windows when prior windows exist"
    );
}

#[test]
fn auth_refresh_skips_first_contact_empty_windows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let refreshed_marker = dir.path().join("refreshed");
    let providers = provider_with(ProviderEntry {
        quota_script: Some("echo '{\"windows\":[]}'".to_string()),
        auth_refresh_command: Some(format!("touch {}", script_quote(&refreshed_marker))),
        ..ProviderEntry::default()
    });

    let outcome = refresh_provider("quota-provider", &providers, &InFlight::new(), &state());

    assert_updated_empty(outcome);
    assert!(
        !refreshed_marker.exists(),
        "auth_refresh_command must not run for first-contact empty windows"
    );
}

#[test]
fn auth_refresh_retries_after_script_failure() {
    let _env_guard = env_lock();
    let _data_home = DataHomeOverride::set();
    let dir = tempfile::tempdir().expect("tempdir");
    let refreshed_marker = dir.path().join("refreshed");
    let quota_script = format!(
        "if [ -e {marker} ]; then echo '{{\"windows\":[{{\"used_percent\":3,\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'; else echo 'auth error' >&2; exit 1; fi",
        marker = script_quote(&refreshed_marker)
    );
    let providers = provider_with(ProviderEntry {
        quota_script: Some(quota_script),
        auth_refresh_command: Some(format!("touch {}", script_quote(&refreshed_marker))),
        ..ProviderEntry::default()
    });

    let outcome = refresh_provider("quota-provider", &providers, &InFlight::new(), &state());

    assert_updated_percent(outcome, 0.03);
    assert!(
        refreshed_marker.exists(),
        "auth_refresh_command must run after script failure"
    );
}

#[test]
fn auth_refresh_returns_failed_when_retry_still_fails() {
    let _env_guard = env_lock();
    let _data_home = DataHomeOverride::set();
    let providers = provider_with(ProviderEntry {
        quota_script: Some("printf 'quota denied\\n' >&2; exit 1".to_string()),
        auth_refresh_command: Some("true".to_string()),
        ..ProviderEntry::default()
    });

    let outcome = refresh_provider("quota-provider", &providers, &InFlight::new(), &state());

    assert_failed_contains(outcome, "quota denied");
}

#[test]
fn auth_refresh_combines_retry_and_refresh_errors_when_both_fail() {
    let _env_guard = env_lock();
    let _data_home = DataHomeOverride::set();
    let providers = provider_with(ProviderEntry {
        quota_script: Some("printf 'quota denied\\n' >&2; exit 1".to_string()),
        auth_refresh_command: Some("printf 'token gone\\n' >&2; exit 7".to_string()),
        ..ProviderEntry::default()
    });

    let outcome = refresh_provider("quota-provider", &providers, &InFlight::new(), &state());
    match outcome {
        RefreshOutcome::Failed(message) => {
            assert!(
                message.contains("quota denied"),
                "missing retry failure: {message}"
            );
            assert!(
                message.contains("auth_refresh_command also failed"),
                "missing combined auth-refresh suffix: {message}"
            );
            assert!(
                message.contains("token gone"),
                "missing auth-refresh stderr: {message}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn derived_quota_paths_preserve_shell_splitting_and_shell_word_quoting_for_legacy_roots() {
    let _env_guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_dir = dir.path().join("bin");
    fs::create_dir(&bin_dir).expect("bin dir");
    let claude_log = dir.path().join("quoted-claude-arg.log");
    let codex_log = dir.path().join("quoted-codex-arg.log");
    install_usage_script(&bin_dir, "anthropic-usage", &claude_log, 23);
    install_usage_script(&bin_dir, "chatgpt-usage", &codex_log, 45);
    let _path = PathOverride::prepend(&bin_dir);

    let claude_root = dir.path().join("quoted claude account's root");
    let codex_root = dir.path().join("quoted codex account's root");
    let sessions = sessions_with_entries([
        (
            "quoted-claude",
            session_entry(format!(
                "\"/usr/local/bin/claude-code-cwd\" {}",
                double_quoted_shell_word(
                    &claude_root
                        .join("projects with spaces")
                        .display()
                        .to_string()
                )
            )),
        ),
        (
            "quoted-codex",
            session_entry(format!(
                "\"/usr/local/bin/codex-turns\" {}",
                double_quoted_shell_word(
                    &codex_root
                        .join("sessions with spaces")
                        .display()
                        .to_string()
                )
            )),
        ),
    ]);

    assert_updated_percent(
        refresh_provider_for_routing(
            "quoted-claude",
            &ProvidersConfig::default(),
            &sessions,
            &InFlight::new(),
            &state(),
        ),
        0.23,
    );
    assert_updated_percent(
        refresh_provider_for_routing(
            "quoted-codex",
            &ProvidersConfig::default(),
            &sessions,
            &InFlight::new(),
            &state(),
        ),
        0.45,
    );
    assert_eq!(
        read_trimmed(&claude_log),
        claude_root.join(".credentials.json").display().to_string()
    );
    assert_eq!(
        read_trimmed(&codex_log),
        codex_root.join("auth.json").display().to_string()
    );
}
