//! Offline Contract B initial-routing regression and selected-account custody.
//! Declared roles: orchestration, mapper, validator, accessor.
use super::*;
use crate::balancer::BalanceContext;
use crate::quota::InFlight;
use crate::quota::marker_verification::test_support::EnvGuard;
use crate::services::RoutingServiceOutput;
use chrono::{Duration, Utc};
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::cell::RefCell;
use std::fs;

const ACCOUNTS: [&str; 3] = ["fixture", "fixture2", "fixture3"];

struct Fixture {
    root: tempfile::TempDir,
    state_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let state_path = root.path().join("state.db");
        fs::write(
            root.path().join("config.toml"),
            "default_provider = 'fixture'",
        )
        .unwrap();
        let entries = [
            "fixture3",
            "fixture-work",
            "myfixture",
            "fixture",
            "fixture2",
        ]
        .map(|name| account_config(root.path(), name))
        .join("\n");
        fs::write(root.path().join("providers.toml"), entries).unwrap();
        let sessions = ACCOUNTS
            .map(|name| session_config(root.path(), name))
            .join("\n");
        fs::write(root.path().join("sessions.toml"), sessions).unwrap();
        // All accounts use one managed wrapper; it must never be run by the recording launcher.
        fs::write(root.path().join("wrapper.sh"), "#!/bin/sh\nexit 91\n").unwrap();
        Self { root, state_path }
    }

    fn services(&self, routing_service: Arc<dyn RoutingServicePort>) -> RuntimeServices {
        RuntimeServices {
            config_root: self.root.path().to_path_buf(),
            state_db_path: Some(self.state_path.clone()),
            working_dir: Some(self.root.path().to_path_buf()),
            state_db_opener: ProductionStateDbOpener,
            routing_service,
        }
    }

    fn seed(&self, path: &Path) -> StateDb {
        let state = StateDb::open(path).unwrap();
        for name in ACCOUNTS {
            let used_percent = if name == "fixture" { 0.10 } else { 1.0 };
            state
                .upsert_quota_refresh(
                    name,
                    &[QuotaWindowInput {
                        used_percent,
                        resets_at: Utc::now() + Duration::hours(5),
                    }],
                )
                .unwrap();
        }
        rusqlite::Connection::open(path)
            .unwrap()
            .execute(
                "UPDATE provider_quotas SET refreshed_at = '2000-01-01T00:00:00Z'",
                [],
            )
            .unwrap();
        state
    }
}

fn account_config(root: &Path, name: &str) -> String {
    let used = if name == "fixture2" { 10 } else { 100 };
    let quota_path = root.join(format!("{name}-quota.json"));
    fs::write(
        &quota_path,
        format!(r#"{{"windows":[{{"used_percent":{used},"resets_at":"2099-01-01T00:00:00Z"}}]}}"#),
    )
    .unwrap();
    format!(
        r#"[{name}]
command = "{}"
interactive_args = ["--settings-id", "{name}", "--config-root", "{}"]
environment = {{ ACCOUNT_FIXTURE = "{name}" }}
unset_environment = ["FIXTURE_SECRET"]
settings_id = "{name}"
quota_script = "cat '{}'"

[{name}.implementation]
family = "offline-fixture"
executable = "{}"
"#,
        root.join("wrapper.sh").display(),
        root.display(),
        quota_path.display(),
        super::tests::repl_test_provider_path().display()
    )
}

fn session_config(root: &Path, name: &str) -> String {
    let turns = root.join(format!("{name}-turns.jsonl"));
    fs::write(&turns, format!(r#"{{"session_id":"{name}-old-session","turn_id":"turn-1","timestamp":"2026-09-06T00:00:00Z","role":"assistant"}}
"#)).unwrap();
    format!(
        r#"[{name}]
turn_script = "cat '{}'"
state_dir = "{}"
"#,
        turns.display(),
        root.join(format!("{name}-scan-state")).display()
    )
}

fn model() -> ModelConfig {
    // Independent expected pool, not copied from the resolver or routing request.
    ModelConfig {
        name: "normal-headless-fixture".into(),
        prompt_mode: PromptMode::Stdin,
        providers: ACCOUNTS
            .map(|name| ProviderConfig::model_provider(name, vec![]))
            .into(),
        inputs: vec![],
        provider: None,
    }
}

#[derive(Default)]
struct RecordingLauncher {
    calls: RefCell<Vec<(ProviderConfig, InteractiveLiveSessionBinding, String)>>,
    prelaunch_turns: RefCell<Vec<Vec<u64>>>,
}

impl InteractiveLauncher for RecordingLauncher {
    fn launch(
        &self,
        provider: &ProviderConfig,
        _cwd: Option<&Path>,
        parent: Option<&str>,
        _state_path: Option<&Path>,
        binding: Option<InteractiveLiveSessionBinding>,
    ) -> Result<crate::executor::cli::InteractiveExecutionResult, String> {
        let binding = binding.unwrap();
        let state = StateDb::open(&binding.state_db_path).unwrap();
        self.prelaunch_turns.borrow_mut().push(
            ACCOUNTS
                .map(|name| state.count_assistant_turns_since(name, None).unwrap())
                .into(),
        );
        self.calls
            .borrow_mut()
            .push((provider.clone(), binding, parent.unwrap().into()));
        Ok(super::tests::successful_interactive_result())
    }
}

struct InspectContext;
impl RoutingServicePort for InspectContext {
    fn select_route(
        &self,
        request: RoutingServiceRequest<'_>,
    ) -> Result<RoutingServiceOutput, ServiceError> {
        assert_eq!(
            request
                .model
                .providers
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ACCOUNTS
        );
        assert!(request.model.providers.iter().all(|p| p.args.is_empty()));
        assert!(request.model.provider.is_none());
        let ctx = request
            .ctx
            .expect("fresh default pool must receive real BalanceContext");
        assert_eq!(ctx.providers_cfg.entries.len(), 5);
        assert_eq!(
            ctx.providers_cfg
                .runtime_provider("fixture2")
                .unwrap()
                .0
                .environment["ACCOUNT_FIXTURE"],
            "fixture2"
        );
        Ok(RoutingServiceOutput { provider_index: 1 })
    }
}

#[test]
fn default_pool_receives_context_and_wrapper_family() {
    let fixture = Fixture::new();
    let launcher = RecordingLauncher::default();
    run_repl_with_default_provider_with_launcher(
        fixture.services(Arc::new(InspectContext)),
        &launcher,
    )
    .unwrap();
    assert_eq!(launcher.calls.borrow().len(), 1);
}

#[test]
fn default_pool_does_not_require_legacy_sessions_config() {
    for content in [None, Some("not valid TOML = [")] {
        let fixture = Fixture::new();
        let path = fixture.root.path().join("sessions.toml");
        match content {
            None => fs::remove_file(path).unwrap(),
            Some(text) => fs::write(path, text).unwrap(),
        }
        run_repl_with_default_provider_with_launcher(
            fixture.services(Arc::new(InspectContext)),
            &RecordingLauncher::default(),
        )
        .unwrap();
    }
}

#[test]
fn default_pool_refresh_changes_unique_winner_and_matches_production() {
    let fixture = Fixture::new();
    let _env = EnvGuard::set("OULIPOLY_DATA_DIR", fixture.root.path().join("locks"));
    let state = fixture.seed(&fixture.state_path);
    let normal = fixture.seed(&fixture.root.path().join("normal.db"));
    let model = model();
    let cached = ProductionRoutingService
        .select_route(RoutingServiceRequest {
            model: &model,
            state: &state,
            ctx: None,
        })
        .unwrap();
    assert_eq!(
        cached.provider_index, 0,
        "cached-only baseline has exactly one eligible account"
    );
    let providers = ProvidersConfig::load(&fixture.root.path().join("providers.toml")).unwrap();
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers,
        in_flight: &in_flight,
    };
    let expected = ProductionRoutingService
        .select_route(RoutingServiceRequest {
            model: &model,
            state: &normal,
            ctx: Some(&ctx),
        })
        .unwrap();
    assert_eq!(
        expected.provider_index, 1,
        "offline refresh makes fixture2 uniquely eligible"
    );
    let launcher = RecordingLauncher::default();
    run_repl_with_default_provider_with_launcher(
        fixture.services(Arc::new(ProductionRoutingService)),
        &launcher,
    )
    .unwrap();
    let calls = launcher.calls.borrow();
    assert_selected_account(&fixture, &state, &calls[0]);
    assert_eq!(
        launcher.prelaunch_turns.borrow()[0],
        [0, 0, 0],
        "current initial routing must not reintroduce synchronous legacy session scans"
    );
    for name in ACCOUNTS {
        assert_eq!(
            state.get_windows(name).unwrap()[0].used_percent,
            normal.get_windows(name).unwrap()[0].used_percent
        );
        assert_eq!(
            state.count_assistant_turns_since(name, None).unwrap(),
            0,
            "no legacy preselection scan for {name}"
        );
    }
}

fn assert_selected_account(
    fixture: &Fixture,
    state: &StateDb,
    call: &(ProviderConfig, InteractiveLiveSessionBinding, String),
) {
    let (provider, binding, parent) = call;
    assert_eq!(
        binding.identity.provider_name, "fixture2",
        "default selection must agree with independently run production routing"
    );
    assert_eq!(binding.identity.settings_id, "fixture2");
    assert_eq!(provider.name, "<provider-family:fixture>");
    assert_eq!(
        provider.command,
        fixture.root.path().join("wrapper.sh").to_str().unwrap()
    );
    assert_eq!(
        provider.interactive_args.as_ref().unwrap(),
        &[
            "--settings-id",
            "fixture2",
            "--config-root",
            fixture.root.path().to_str().unwrap()
        ]
    );
    assert_eq!(provider.environment["ACCOUNT_FIXTURE"], "fixture2");
    assert_eq!(provider.unset_environment, ["FIXTURE_SECRET"]);
    let (identity, authority) =
        crate::executor::cli::spawn_identity::split_invocation_launch_environment(parent).unwrap();
    assert_eq!(
        CompositeInvocationId::parse_env_value(&identity)
            .unwrap()
            .source,
        "fixture2"
    );
    assert!(authority.is_some());
    let row = state
        .get_invocation_by_uuid(&binding.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.provider_name.as_deref(), Some("fixture2"));
    assert_eq!(row.provider_index, 1);
}

#[test]
fn default_pool_exhaustion_stops_before_launch() {
    let fixture = Fixture::new();
    let _env = EnvGuard::set("OULIPOLY_DATA_DIR", fixture.root.path().join("locks"));
    fixture.seed(&fixture.state_path);
    fs::write(
        fixture.root.path().join("fixture2-quota.json"),
        r#"{"windows":[{"used_percent":100,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
    )
    .unwrap();
    let launcher = RecordingLauncher::default();
    let error = run_repl_with_default_provider_with_launcher(
        fixture.services(Arc::new(ProductionRoutingService)),
        &launcher,
    )
    .unwrap_err();
    assert!(error.contains("exhausted"), "{error}");
    assert!(launcher.calls.borrow().is_empty());
}

#[test]
fn default_pool_verifies_markers_before_honoring_exhaustion() {
    let fixture = Fixture::new();
    let _env = EnvGuard::set("OULIPOLY_DATA_DIR", fixture.root.path().join("locks"));
    let state = fixture.seed(&fixture.state_path);
    let unavailable_until = Utc::now() + Duration::hours(2);
    state
        .record_provider_unavailable("fixture", Some(unavailable_until), "RollingWindow5h")
        .unwrap();
    state
        .record_provider_unavailable("fixture2", Some(unavailable_until), "UpstreamApiDown")
        .unwrap();
    let launcher = RecordingLauncher::default();
    run_repl_with_default_provider_with_launcher(
        fixture.services(Arc::new(ProductionRoutingService)),
        &launcher,
    )
    .unwrap();
    assert_eq!(
        launcher.calls.borrow()[0].1.identity.provider_name,
        "fixture2"
    );
    assert!(
        state
            .get_quota("fixture2")
            .unwrap()
            .unwrap()
            .next_available_at
            .is_none()
    );
    assert_eq!(
        state
            .get_quota("fixture")
            .unwrap()
            .unwrap()
            .next_available_at,
        Some(unavailable_until)
    );
}

#[test]
fn default_pool_repairs_topology_even_inside_refresh_ttl() {
    let fixture = Fixture::new();
    let _env = EnvGuard::set("OULIPOLY_DATA_DIR", fixture.root.path().join("locks"));
    let state = StateDb::open(&fixture.state_path).unwrap();
    let window = QuotaWindowInput {
        used_percent: 0.1,
        resets_at: Utc::now() + Duration::hours(5),
    };
    state
        .upsert_quota_refresh("fixture", std::slice::from_ref(&window))
        .unwrap();
    state
        .upsert_quota_refresh("fixture2", std::slice::from_ref(&window))
        .unwrap();
    state
        .upsert_quota_refresh(
            "fixture3",
            &[
                QuotaWindowInput {
                    used_percent: 1.0,
                    ..window.clone()
                },
                window,
            ],
        )
        .unwrap();
    fs::write(fixture.root.path().join("fixture-quota.json"), r#"{"windows":[{"used_percent":10,"resets_at":"2099-01-01T00:00:00Z"},{"used_percent":100,"resets_at":"2099-01-02T00:00:00Z"}]}"#).unwrap();
    assert!(!crate::quota::is_routing_stale(&state, "fixture"));
    let launcher = RecordingLauncher::default();
    run_repl_with_default_provider_with_launcher(
        fixture.services(Arc::new(ProductionRoutingService)),
        &launcher,
    )
    .unwrap();
    assert_eq!(
        state.get_windows("fixture").unwrap().len(),
        2,
        "fresh but incomplete topology must be probed"
    );
    assert!(
        state
            .get_quota("fixture")
            .unwrap()
            .unwrap()
            .last_topology_probe_at
            .is_some()
    );
    assert_eq!(
        launcher.calls.borrow()[0].1.identity.provider_name,
        "fixture2"
    );
}
