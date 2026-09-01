#![cfg(unix)]

use oulipoly_runtime::repl_default_provider::{RuntimeServices, run_repl_with_default_provider};
use oulipoly_runtime::services::ProductionRoutingService;
use oulipoly_state::StateDb;
use oulipoly_state::repositories::{ProductionStateDbOpener, StateDbOpener};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvRestore {
    old_data_dir: Option<std::ffi::OsString>,
    old_data_home: Option<std::ffi::OsString>,
}

impl EnvRestore {
    fn set_data_dir(path: &Path) -> Self {
        let old_data_dir = std::env::var_os("OULIPOLY_DATA_DIR");
        let old_data_home = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("OULIPOLY_DATA_DIR", path);
            std::env::set_var("XDG_DATA_HOME", path);
        }
        Self {
            old_data_dir,
            old_data_home,
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match self.old_data_dir.take() {
            Some(value) => unsafe {
                std::env::set_var("OULIPOLY_DATA_DIR", value);
            },
            None => unsafe {
                std::env::remove_var("OULIPOLY_DATA_DIR");
            },
        }
        match self.old_data_home.take() {
            Some(value) => unsafe {
                std::env::set_var("XDG_DATA_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("XDG_DATA_HOME");
            },
        }
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(
        path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
    )
    .unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn write_provider_endpoint(path: &Path) {
    fs::write(
        path,
        r#"#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
print(json.dumps({
    "contract": request["contract"],
    "request_id": request["request_id"],
    "ok": True,
    "result": {
        "provider_id": "age33-default-provider-fixture",
        "display_name": "AGE-33 Default Provider Fixture",
        "contract_versions": [request["contract"]],
        "preferred_contract": request["contract"],
        "capabilities": {
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
        },
    },
}))
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn source_from<'a>(source: &'a str, start: &str) -> &'a str {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    &source[start_idx..]
}

fn source_block_after<'a>(source: &'a str, start: &str) -> &'a str {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    let open_idx = source[start_idx..]
        .find('{')
        .map(|idx| start_idx + idx)
        .unwrap_or_else(|| panic!("missing opening brace after {start}"));
    let mut depth = 1usize;
    let mut idx = open_idx + 1;
    let bytes = source.as_bytes();

    while idx < bytes.len() {
        match bytes[idx] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open_idx + 1..idx];
                }
            }
            _ => {}
        }
        idx += 1;
    }

    panic!("missing closing brace after {start}");
}

#[derive(Clone, Debug, Default)]
struct FakeStateDbOpener {
    calls: Arc<Mutex<Vec<String>>>,
}

impl FakeStateDbOpener {
    fn calls(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.calls)
    }
}

impl StateDbOpener for FakeStateDbOpener {
    fn open_default(&self) -> Result<StateDb, String> {
        self.calls.lock().unwrap().push("open_default".to_string());
        Err("sentinel default-open from fake StateDbOpener".to_string())
    }

    fn open_at(&self, path: &Path) -> Result<StateDb, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("open_at:{}", path.display()));
        Err("explicit open_at branch should not run for state_db_path None".to_string())
    }

    fn open_in_memory(&self) -> StateDb {
        StateDb::open(Path::new(":memory:")).expect("in-memory StateDb must open")
    }
}

#[test]
fn age_33_runtime_default_provider_cutover_preserves_load_open_select_launch_order() {
    let source = include_str!("../src/repl_default_provider.rs");
    let runtime_services = source_block_after(source, "pub struct RuntimeServices");
    let run = source_from(source, "fn run_repl_with_default_provider_with_launcher");

    assert!(
        runtime_services.contains("state_db_opener:"),
        "RuntimeServices must carry state_db_opener as a field on the struct itself"
    );
    assert!(
        runtime_services.contains("routing_service:"),
        "RuntimeServices must carry the AGE-35 routing service dependency"
    );
    assert!(
        !source.contains("impl std::ops::Deref for RuntimeServices"),
        "RuntimeServices must not reach a static opener through the rejected Deref shortcut"
    );
    let app_config = run
        .find("AppConfig::load(&app_config_path)")
        .expect("app config load");
    let default_provider = run
        .find("'default_provider' must be set")
        .expect("default_provider requirement");
    let providers = run
        .find("ProvidersConfig::load(&providers_path)")
        .expect("providers config load");
    let open_at = run
        .find("services.state_db_opener.open_at(path)")
        .expect("explicit state opener branch");
    let open_default = run
        .find("services.state_db_opener.open_default()")
        .expect("default state opener branch");
    let route = run
        .find(".select_route(")
        .expect("routing service provider selection");
    let _routing_request = run
        .find("RoutingServiceRequest")
        .expect("routing service request construction");
    let _cached_only = run.find("ctx: None").expect("cached-only routing request");
    let provider_selection = run
        .find("providers.runtime_provider(member_name)")
        .expect("runtime provider selection");
    let out_of_bounds = run
        .find("provider_index >= carrier_model.providers.len()")
        .expect("out-of-bounds provider index guard");
    let launcher = run.find(".launch(").expect("launcher invocation");

    assert!(
        app_config < default_provider && default_provider < providers,
        "runtime default-provider must load config.toml, require default_provider, then load providers.toml"
    );
    assert!(
        providers < open_at && providers < open_default,
        "state open must remain downstream of strict app/provider config loading"
    );
    assert!(
        open_at < route
            && open_default < route
            && route < provider_selection
            && provider_selection < launcher,
        "provider selection and launcher invocation must remain downstream of the opened state"
    );
    assert!(
        run.contains("RoutingServiceRequest") && run.contains("ctx: None"),
        "runtime default-provider should build a cached-only routing request"
    );
    assert!(
        route < out_of_bounds && out_of_bounds < provider_selection,
        "caller-owned out-of-bounds mapping must remain after service routing and before runtime provider resolution"
    );
    assert!(
        !run.contains("balancer::select_provider"),
        "runtime default-provider --new must not call select_provider directly after AGE-35 cutover"
    );
}

#[test]
fn age_33_runtime_default_provider_config_errors_fail_before_launch() {
    let missing_default = tempfile::tempdir().unwrap();
    let missing_root = missing_default.path().join("config-root");
    fs::create_dir_all(&missing_root).unwrap();
    fs::write(
        missing_root.join("config.toml"),
        r#"diagnostics_model = "fixture""#,
    )
    .unwrap();

    let marker = missing_default.path().join("launched.txt");
    let provider_script = missing_default.path().join("provider.sh");
    write_executable(
        &provider_script,
        &format!("printf launched > {:?}\n", marker.to_string_lossy()),
    );
    fs::write(
        missing_root.join("providers.toml"),
        format!(
            r#"[fixture]
command = {:?}
interactive_args = ["interactive-launch"]
prompt_mode = "arg"
"#,
            provider_script.to_string_lossy()
        ),
    )
    .unwrap();

    let err = run_repl_with_default_provider(
        RuntimeServices {
            config_root: missing_root,
            state_db_path: Some(missing_default.path().join("state.db")),
            working_dir: None,
            state_db_opener: ProductionStateDbOpener,
            routing_service: Arc::new(ProductionRoutingService),
        },
        |_| Ok(()),
    )
    .unwrap_err();

    assert!(err.contains("'default_provider' must be set in"), "{err}");
    assert!(
        !marker.exists(),
        "launcher must not run when default_provider is missing"
    );

    let malformed_providers = tempfile::tempdir().unwrap();
    let malformed_root = malformed_providers.path().join("config-root");
    fs::create_dir_all(&malformed_root).unwrap();
    fs::write(
        malformed_root.join("config.toml"),
        r#"default_provider = "fixture""#,
    )
    .unwrap();
    fs::write(malformed_root.join("providers.toml"), "not = [").unwrap();

    let err = run_repl_with_default_provider(
        RuntimeServices {
            config_root: malformed_root,
            state_db_path: Some(malformed_providers.path().join("state.db")),
            working_dir: None,
            state_db_opener: ProductionStateDbOpener,
            routing_service: Arc::new(ProductionRoutingService),
        },
        |_| Ok(()),
    )
    .unwrap_err();

    assert!(err.contains("TOML parse error"), "{err}");
    assert!(err.contains("providers.toml"), "{err}");

    let malformed_config = tempfile::tempdir().unwrap();
    let malformed_config_root = malformed_config.path().join("config-root");
    fs::create_dir_all(&malformed_config_root).unwrap();
    fs::write(malformed_config_root.join("config.toml"), "not = [").unwrap();

    let malformed_config_marker = malformed_config.path().join("launched.txt");
    let malformed_config_provider_script = malformed_config.path().join("provider.sh");
    write_executable(
        &malformed_config_provider_script,
        &format!(
            "printf launched > {:?}\n",
            malformed_config_marker.to_string_lossy()
        ),
    );
    fs::write(
        malformed_config_root.join("providers.toml"),
        format!(
            r#"[fixture]
command = {:?}
interactive_args = ["interactive-launch"]
prompt_mode = "arg"
"#,
            malformed_config_provider_script.to_string_lossy()
        ),
    )
    .unwrap();

    let err = run_repl_with_default_provider(
        RuntimeServices {
            config_root: malformed_config_root,
            state_db_path: Some(malformed_config.path().join("state.db")),
            working_dir: None,
            state_db_opener: ProductionStateDbOpener,
            routing_service: Arc::new(ProductionRoutingService),
        },
        |_| Ok(()),
    )
    .unwrap_err();

    assert!(err.contains("failed to parse app config"), "{err}");
    assert!(err.contains("TOML parse error"), "{err}");
    assert!(err.contains("config.toml"), "{err}");
    assert!(
        !malformed_config_marker.exists(),
        "launcher must not run when config.toml is malformed"
    );
}

#[test]
fn age_33_runtime_default_provider_none_state_path_uses_injected_default_opener() {
    let temp = tempfile::tempdir().unwrap();
    let config_root = temp.path().join("config-root");
    fs::create_dir_all(&config_root).unwrap();
    fs::write(
        config_root.join("config.toml"),
        r#"default_provider = "fixture""#,
    )
    .unwrap();

    let marker = temp.path().join("launched.txt");
    let provider_script = temp.path().join("provider.sh");
    let provider_endpoint = temp.path().join("provider-endpoint.py");
    write_provider_endpoint(&provider_endpoint);
    write_executable(
        &provider_script,
        &format!("printf launched > {:?}\n", marker.to_string_lossy()),
    );
    fs::write(
        config_root.join("providers.toml"),
        format!(
            r#"[fixture]
command = {:?}
interactive_args = ["interactive-launch"]
prompt_mode = "arg"

[fixture.implementation]
family = "fixture"
executable = {:?}
"#,
            provider_script.to_string_lossy(),
            provider_endpoint.to_string_lossy(),
        ),
    )
    .unwrap();

    let opener = FakeStateDbOpener::default();
    let calls = opener.calls();

    let err = run_repl_with_default_provider(
        RuntimeServices {
            config_root,
            state_db_path: None,
            working_dir: None,
            state_db_opener: opener,
            routing_service: Arc::new(ProductionRoutingService),
        },
        |_| Ok(()),
    )
    .unwrap_err();

    assert!(
        err.contains("sentinel default-open from fake StateDbOpener"),
        "{err}"
    );
    assert_eq!(*calls.lock().unwrap(), vec!["open_default".to_string()]);
    assert!(
        !marker.exists(),
        "launcher must not run after the fake default opener returns its sentinel error"
    );
}

#[test]
fn age_33_runtime_default_provider_uses_explicit_state_db_path_when_supplied() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let config_root = temp.path().join("config-root");
    fs::create_dir_all(&config_root).unwrap();
    let explicit_state_db = temp.path().join("explicit-state").join("state.db");
    let blocked_default_data_home = temp.path().join("blocked-default-data-home");
    fs::write(&blocked_default_data_home, "not a directory").unwrap();
    let _restore = EnvRestore::set_data_dir(&blocked_default_data_home);

    let marker = temp.path().join("launched.txt");
    let provider_script = temp.path().join("provider.sh");
    let provider_endpoint = temp.path().join("provider-endpoint.py");
    write_provider_endpoint(&provider_endpoint);
    write_executable(
        &provider_script,
        &format!(
            "printf '%s' \"${{1:-missing}}\" > {:?}\n",
            marker.to_string_lossy()
        ),
    );
    fs::write(
        config_root.join("config.toml"),
        r#"default_provider = "fixture""#,
    )
    .unwrap();
    fs::write(
        config_root.join("providers.toml"),
        format!(
            r#"[fixture]
command = {:?}
args = ["one-shot-only"]
interactive_args = ["interactive-launch"]
prompt_mode = "arg"

[fixture.implementation]
family = "fixture"
executable = {:?}
"#,
            provider_script.to_string_lossy(),
            provider_endpoint.to_string_lossy(),
        ),
    )
    .unwrap();

    let error = run_repl_with_default_provider(
        RuntimeServices {
            config_root,
            state_db_path: Some(PathBuf::from(&explicit_state_db)),
            working_dir: None,
            state_db_opener: ProductionStateDbOpener,
            routing_service: Arc::new(ProductionRoutingService),
        },
        |_| Ok(()),
    )
    .unwrap_err();

    assert!(
        error.contains("live_session_identity_unavailable"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&marker).unwrap(), "interactive-launch");
    assert!(explicit_state_db.exists());
    assert!(
        !blocked_default_data_home
            .join("oulipoly-agent-runner")
            .exists(),
        "explicit state_db_path should avoid StateDb::open_default path discovery"
    );
}

#[test]
fn default_provider_launch_preserves_runtime_invocation_mode_when_rewriting_name() {
    let source = include_str!("../src/repl_default_provider.rs");
    let run = source_from(source, "fn run_repl_with_default_provider_with_launcher");

    assert!(
        run.contains("runtime_provider(member_name)"),
        "default-provider launch must resolve the runtime provider before rewriting carrier name"
    );
    assert!(
        run.contains("..provider")
            || (run.contains("invocation_mode:") && run.contains(".invocation_mode")),
        "default-provider carrier-name rewrite must preserve ProviderConfig.invocation_mode"
    );
}
