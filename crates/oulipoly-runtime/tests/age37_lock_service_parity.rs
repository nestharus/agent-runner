use oulipoly_config::{ProvidersConfig, load_models};
use oulipoly_runtime::services::{
    ProductionSessionLockService, SessionLockFailure, SessionLockServicePort,
    SessionLockServiceRequest, SessionLockSuccess,
};
use oulipoly_runtime::session_lock::{LockError, ReleaseReceipt, SessionLock};
use oulipoly_state::{ResumeError, StateDb};
use rusqlite::params;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const MISSING_SESSION: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const MODEL: &str = "claude-opus";
const CLAUDE_PROVIDER: &str = "claude";

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    old_config: Option<OsString>,
    old_data: Option<OsString>,
    old_home: Option<OsString>,
}

impl EnvGuard {
    fn new(config_home: &Path, data_home: &Path) -> Self {
        let guard = Self {
            old_config: std::env::var_os("XDG_CONFIG_HOME"),
            old_data: std::env::var_os("XDG_DATA_HOME"),
            old_home: std::env::var_os("HOME"),
        };
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", config_home);
            std::env::set_var("XDG_DATA_HOME", data_home);
            std::env::set_var("HOME", data_home);
        }
        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            restore_env("XDG_CONFIG_HOME", self.old_config.take());
            restore_env("XDG_DATA_HOME", self.old_data.take());
            restore_env("HOME", self.old_home.take());
        }
    }
}

unsafe fn restore_env(key: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe { std::env::set_var(key, value) },
        None => unsafe { std::env::remove_var(key) },
    }
}

struct LockFixture {
    _dir: tempfile::TempDir,
    _env: EnvGuard,
    config_root: PathBuf,
    models_dir: PathBuf,
    data_root: PathBuf,
    lock_dir: PathBuf,
}

impl LockFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let config_root = config_home.join("oulipoly-agent-runner");
        let models_dir = config_root.join("models");
        let data_root = data_home.join("oulipoly-agent-runner");
        let lock_dir = data_root.join("locks");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&data_root).unwrap();
        let env = EnvGuard::new(&config_home, &data_home);
        Self {
            _dir: dir,
            _env: env,
            config_root,
            models_dir,
            data_root,
            lock_dir,
        }
    }

    fn prepare_active_session(&self) {
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!("[[providers]]\nname = \"{CLAUDE_PROVIDER}\"\n"),
        )
        .unwrap();
        fs::write(
            self.config_root.join("providers.toml"),
            format!(
                r#"[{CLAUDE_PROVIDER}]
command = "provider-command-that-must-not-run"
args = []
interactive_args = []
prompt_mode = "arg"
"#
            ),
        )
        .unwrap();
        let db = StateDb::open(&self.data_root.join("state.db")).unwrap();
        db.connection()
            .execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
                params![CHAIN_A, MODEL],
            )
            .unwrap();
        db.connection()
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
                params![CHAIN_A, CLAUDE_PROVIDER, SESSION_A],
            )
            .unwrap();
    }

    fn prepare_empty_state(&self) {
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!("[[providers]]\nname = \"{CLAUDE_PROVIDER}\"\n"),
        )
        .unwrap();
        let _ = StateDb::open(&self.data_root.join("state.db")).unwrap();
    }
}

#[test]
fn lock_service_acquire_matches_pause_handshake_dependencies() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let direct_fixture = LockFixture::new();
    direct_fixture.prepare_active_session();
    let direct = direct_acquire(&direct_fixture, 30_000);
    drop(direct_fixture);

    let service_fixture = LockFixture::new();
    service_fixture.prepare_active_session();
    let service = ProductionSessionLockService::default();
    let output = service
        .lock_session(SessionLockServiceRequest::Acquire {
            session_id: SESSION_A.to_string(),
            ttl_ms: 30_000,
        })
        .unwrap();

    match output.result.unwrap() {
        SessionLockSuccess::Acquired {
            session_id,
            chain_id,
            provider_name,
            lease,
        } => {
            assert_eq!(session_id, SESSION_A);
            assert_eq!(chain_id, CHAIN_A);
            assert_eq!(provider_name, CLAUDE_PROVIDER);
            assert_eq!(lease.session_id, direct.session_id);
            assert_eq!(lease.provider_name, direct.provider_name);
            assert!(lease.token.starts_with("pause_"));
            assert_eq!(lease.lock_path.file_name(), direct.lock_path.file_name());
        }
        other => panic!("expected Acquired success, got {other:?}"),
    }
}

#[test]
fn lock_service_acquire_preserves_resume_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let fixture = LockFixture::new();
    fixture.prepare_empty_state();
    let service = ProductionSessionLockService::default();
    let output = service
        .lock_session(SessionLockServiceRequest::Acquire {
            session_id: MISSING_SESSION.to_string(),
            ttl_ms: 30_000,
        })
        .unwrap();

    match output.result.unwrap_err() {
        SessionLockFailure::Resume(ResumeError::NoChainFound { input }) => {
            assert_eq!(input, MISSING_SESSION);
        }
        other => panic!("expected Resume(NoChainFound), got {other:?}"),
    }
}

#[test]
fn lock_service_acquire_preserves_lock_error_busy() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let fixture = LockFixture::new();
    fixture.prepare_active_session();
    let lock = SessionLock::new(&fixture.lock_dir).unwrap();
    let busy = lock
        .acquire(SESSION_A, CLAUDE_PROVIDER, Duration::from_secs(30))
        .unwrap();
    let service = ProductionSessionLockService::default();
    let output = service
        .lock_session(SessionLockServiceRequest::Acquire {
            session_id: SESSION_A.to_string(),
            ttl_ms: 30_000,
        })
        .unwrap();

    match output.result.unwrap_err() {
        SessionLockFailure::Lock(LockError::Busy { expires_at, .. }) => {
            assert_eq!(expires_at, busy.expires_at);
        }
        other => panic!("expected Lock(Busy), got {other:?}"),
    }
}

#[test]
fn lock_service_release_matches_resume_handshake_dependencies() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let fixture = LockFixture::new();
    fixture.prepare_active_session();
    let lease = SessionLock::new(&fixture.lock_dir)
        .unwrap()
        .acquire(SESSION_A, CLAUDE_PROVIDER, Duration::from_secs(30))
        .unwrap();
    let service = ProductionSessionLockService::default();
    let output = service
        .lock_session(SessionLockServiceRequest::Release {
            session_id: SESSION_A.to_string(),
            token: lease.token.clone(),
        })
        .unwrap();

    match output.result.unwrap() {
        SessionLockSuccess::Released { receipt } => {
            assert_release_receipt(receipt, &lease.token, false);
        }
        other => panic!("expected Released success, got {other:?}"),
    }
}

#[test]
fn lock_service_release_preserves_lock_error_token_invalid() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let fixture = LockFixture::new();
    fixture.prepare_empty_state();
    let service = ProductionSessionLockService::default();
    let output = service
        .lock_session(SessionLockServiceRequest::Release {
            session_id: SESSION_A.to_string(),
            token: "not-a-pause-token".to_string(),
        })
        .unwrap();

    match output.result.unwrap_err() {
        SessionLockFailure::Lock(LockError::TokenInvalid) => {}
        other => panic!("expected Lock(TokenInvalid), got {other:?}"),
    }
}

fn direct_acquire(fixture: &LockFixture, ttl_ms: u64) -> oulipoly_runtime::session_lock::Lease {
    let db = StateDb::open(&fixture.data_root.join("state.db")).unwrap();
    let providers = ProvidersConfig::load(&fixture.config_root.join("providers.toml")).unwrap();
    let models = load_models(&fixture.models_dir, Some(&providers)).unwrap();
    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();
    assert_eq!(resolved.chain_id, CHAIN_A);
    SessionLock::new(&fixture.lock_dir)
        .unwrap()
        .acquire(
            &resolved.active_session_id,
            &resolved.active_provider,
            Duration::from_millis(ttl_ms),
        )
        .unwrap()
}

fn assert_release_receipt(receipt: ReleaseReceipt, token: &str, already_released: bool) {
    assert_eq!(receipt.session_id, SESSION_A);
    assert_eq!(receipt.token, token);
    assert_eq!(receipt.already_released, already_released);
    assert!(!receipt.released_at.is_empty());
}
