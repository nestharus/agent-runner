use agent_runner_lib::load_app_config;
use std::path::Path;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_config_home(test: impl FnOnce(&Path)) {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let old_config_home = std::env::var_os("XDG_CONFIG_HOME");

    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test(dir.path())));

    match old_config_home {
        Some(value) => unsafe {
            std::env::set_var("XDG_CONFIG_HOME", value);
        },
        None => unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        },
    }

    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

fn app_config_dir(config_home: &Path) -> std::path::PathBuf {
    config_home.join("oulipoly-agent-runner")
}

#[test]
fn reads_diagnostics_model_from_root_config() {
    with_config_home(|config_home| {
        let app_dir = app_config_dir(config_home);
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("config.toml"),
            r#"diagnostics_model = "codex~high"
"#,
        )
        .unwrap();

        let config = load_app_config();

        assert_eq!(config.diagnostics_model.as_deref(), Some("codex~high"));
    });
}

#[test]
fn missing_root_config_returns_no_diagnostics_model() {
    with_config_home(|_| {
        let config = load_app_config();

        assert_eq!(config.diagnostics_model, None);
    });
}

#[test]
fn unreadable_root_config_returns_no_diagnostics_model() {
    with_config_home(|config_home| {
        let app_dir = app_config_dir(config_home);
        std::fs::create_dir_all(app_dir.join("config.toml")).unwrap();

        let config = load_app_config();

        assert_eq!(config.diagnostics_model, None);
    });
}

#[test]
fn malformed_root_config_returns_no_diagnostics_model() {
    with_config_home(|config_home| {
        let app_dir = app_config_dir(config_home);
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("config.toml"), "diagnostics_model = [").unwrap();

        let config = load_app_config();

        assert_eq!(config.diagnostics_model, None);
    });
}
