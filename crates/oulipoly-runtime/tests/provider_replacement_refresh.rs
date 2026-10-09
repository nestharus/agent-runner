//! Declared roles: orchestration, validator.
//!
//! Compatible provider replacement through one live registry. Fake providers
//! are real executables run by the provider client; each records the
//! generation that actually answered, so old cached agreement and new binary
//! use are distinguishable without restarting the registry.

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEndpointConfig, ProviderEntry, ProvidersConfig,
};
use oulipoly_provider::generated::{CONTRACT_VERSION, DescribeResult};
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryError, ProviderRegistryOptions,
};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Generation<'a> {
    name: &'a str,
    launch_output_v1: bool,
    /// Describe waits for this file after writing `<log>.started`.
    gate: Option<&'a Path>,
}

fn provider_body(log: &Path, generation: &Generation<'_>) -> String {
    let gate = generation.gate.map_or_else(
        || "None".to_string(),
        |path| serde_json::to_string(&path.display().to_string()).unwrap(),
    );
    format!(
        r#"#!/usr/bin/env python3
import json, os, sys, time
LOG = {log}
GATE = {gate}
request = json.loads(sys.stdin.read() or "{{}}")
sub = sys.argv[1] if len(sys.argv) > 1 else ""
with open(LOG, "a") as f:
    f.write("{name} " + sub + "\n")
if GATE is not None and sub == "describe":
    open(LOG + ".started", "w").close()
    while not os.path.exists(GATE):
        time.sleep(0.01)
print(json.dumps({{
    "contract": request["contract"],
    "request_id": request["request_id"],
    "ok": True,
    "result": {{
        "provider_id": "{name}",
        "display_name": "Fake Provider",
        "contract_versions": ["{contract}"],
        "preferred_contract": "{contract}",
        "capabilities": {{
            "launch": True, "launch_output_v1": {launch_output}, "policy": True,
            "quota": False, "session": False, "terminal": False, "rotation": False,
            "discovery": False, "settings": False, "setup_brain": False,
            "setup": False, "migration": False,
        }},
    }},
}}))
"#,
        log = serde_json::to_string(&log.display().to_string()).unwrap(),
        gate = gate,
        name = generation.name,
        contract = CONTRACT_VERSION,
        launch_output = if generation.launch_output_v1 {
            "True"
        } else {
            "False"
        },
    )
}

fn executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Build elsewhere, then rename over the configured path (new inode).
fn install_atomic(path: &Path, log: &Path, generation: &Generation<'_>) {
    let staged = path.with_extension("staged");
    fs::write(&staged, provider_body(log, generation)).unwrap();
    executable(&staged);
    fs::rename(&staged, path).unwrap();
}

/// Truncate and rewrite the configured file itself (same inode).
fn write_in_place(path: &Path, log: &Path, generation: &Generation<'_>) {
    fs::write(path, provider_body(log, generation)).unwrap();
    executable(path);
}

fn answered(log: &Path) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn registry_for(accounts: &[(&str, &Path)]) -> ProviderRegistry {
    let providers = ProvidersConfig {
        entries: accounts
            .iter()
            .map(|(account, path)| {
                (
                    account.to_string(),
                    ProviderEntry {
                        implementation: Some(ProviderEndpointConfig {
                            family: format!("{account}-family"),
                            executable: path.display().to_string(),
                        }),
                        settings_id: Some(format!("{account}-settings")),
                        ..ProviderEntry::default()
                    },
                )
            })
            .collect::<HashMap<_, _>>(),
    };
    let models = accounts
        .iter()
        .map(|(account, _)| ModelConfig {
            name: format!("{account}-model"),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(*account, Vec::new())],
            inputs: Vec::new(),
            provider: None,
        })
        .collect::<Vec<_>>();
    ProviderRegistry::from_configs(&models, &providers, ProviderRegistryOptions::default()).unwrap()
}

fn invoke_describe(client: &oulipoly_provider::client::ProviderClient) -> DescribeResult {
    client
        .invoke_typed(
            "describe",
            serde_json::json!({
                "contract": CONTRACT_VERSION,
                "request_id": "held-endpoint-call",
                "host": {"app": "oulipoly-agent-runner", "env": {}},
                "params": {}
            }),
            [],
        )
        .unwrap()
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

const ONE: Generation<'static> = Generation {
    name: "gen-one",
    launch_output_v1: false,
    gate: None,
};
const TWO: Generation<'static> = Generation {
    name: "gen-two",
    launch_output_v1: true,
    gate: None,
};

#[test]
fn atomic_replacement_refreshes_cached_agreement_without_registry_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("provider");
    let log = temp.path().join("answered.log");
    install_atomic(&path, &log, &ONE);
    let registry = registry_for(&[("account", &path)]);

    let old = registry.preflight_account("account").unwrap();
    assert_eq!(old.capabilities().provider_id, "gen-one");
    assert_ne!(old.capabilities().capabilities.launch_output_v1, Some(true));
    let family = registry.preflight_family("account-family").unwrap();
    assert_eq!(
        registry
            .describe_model_provider_instance("account-model", "account")
            .unwrap()
            .provider_id,
        "gen-one"
    );
    // Unchanged revision: cached agreement is reused, nothing re-describes.
    for _ in 0..20 {
        assert!(Arc::ptr_eq(
            &old,
            &registry.preflight_account("account").unwrap()
        ));
    }
    assert!(Arc::ptr_eq(
        &family,
        &registry.preflight_family("account-family").unwrap()
    ));
    registry
        .describe_model_provider_instance("account-model", "account")
        .unwrap();
    assert_eq!(answered(&log), ["gen-one describe", "gen-one describe"]);

    install_atomic(&path, &log, &TWO);

    let new = registry.preflight_account("account").unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    assert_eq!(new.capabilities().provider_id, "gen-two");
    assert_eq!(new.capabilities().capabilities.launch_output_v1, Some(true));
    assert_eq!(new.account_name(), "account");
    assert_eq!(new.settings_id().unwrap(), "account-settings");
    let new_family = registry.preflight_family("account-family").unwrap();
    assert_eq!(new_family.capabilities().provider_id, "gen-two");
    assert_eq!(
        registry
            .describe_model_provider_instance("account-model", "account")
            .unwrap()
            .provider_id,
        "gen-two"
    );
    assert!(Arc::ptr_eq(
        &new,
        &registry.preflight_account("account").unwrap()
    ));
    // Account and family refresh their own agreement; the artifact describe
    // cache was refreshed by whichever ran first and is then reused.
    assert_eq!(
        answered(&log)[2..],
        ["gen-two describe", "gen-two describe"]
    );

    // A holder of the old endpoint keeps the image it was admitted with.
    assert_eq!(invoke_describe(old.client()).provider_id, "gen-one");
    assert_eq!(invoke_describe(new.client()).provider_id, "gen-two");
    assert_ne!(
        old.endpoint_identity().unwrap().endpoint_identity_sha256,
        new.endpoint_identity().unwrap().endpoint_identity_sha256
    );
}

#[test]
fn completed_in_place_write_refreshes_cached_agreement() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("provider");
    let log = temp.path().join("answered.log");
    install_atomic(&path, &log, &ONE);
    let inode = fs::metadata(&path).unwrap().ino();
    let registry = registry_for(&[("account", &path)]);
    let old = registry.preflight_account("account").unwrap();
    assert_eq!(old.capabilities().provider_id, "gen-one");

    write_in_place(&path, &log, &TWO);
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);

    let new = registry.preflight_account("account").unwrap();
    assert_eq!(new.capabilities().provider_id, "gen-two");
    assert_eq!(new.capabilities().capabilities.launch_output_v1, Some(true));
    assert!(Arc::ptr_eq(
        &new,
        &registry.preflight_account("account").unwrap()
    ));
    assert_eq!(answered(&log), ["gen-one describe", "gen-two describe"]);
}

#[test]
fn removed_or_unavailable_artifact_fails_without_sticking_then_replacement_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("provider");
    let log = temp.path().join("answered.log");
    install_atomic(&path, &log, &ONE);
    let registry = registry_for(&[("account", &path)]);
    let old = registry.preflight_account("account").unwrap();

    fs::remove_file(&path).unwrap();
    for _ in 0..2 {
        match registry.preflight_account("account") {
            Err(ProviderRegistryError::ProviderTransport { kind, .. }) => {
                assert_eq!(kind, "missing_artifact")
            }
            other => panic!("removed artifact must not serve cached agreement: {other:?}"),
        }
    }
    // The in-flight holder still owns its pinned image after removal.
    assert_eq!(invoke_describe(old.client()).provider_id, "gen-one");

    // Mid non-atomic rewrite: present but not yet usable.
    fs::write(&path, b"#!/usr/bin/env python3\nimport sys; sys.exit(3)\n").unwrap();
    executable(&path);
    assert!(registry.preflight_account("account").is_err());
    assert!(registry.preflight_account("account").is_err());

    write_in_place(&path, &log, &TWO);
    let recovered = registry.preflight_account("account").unwrap();
    assert_eq!(recovered.capabilities().provider_id, "gen-two");

    let three = Generation {
        name: "gen-three",
        launch_output_v1: false,
        gate: None,
    };
    install_atomic(&path, &log, &three);
    let again = registry.preflight_account("account").unwrap();
    assert_eq!(again.capabilities().provider_id, "gen-three");
    assert_ne!(
        again.capabilities().capabilities.launch_output_v1,
        Some(true)
    );
    assert_eq!(
        answered(&log),
        [
            "gen-one describe",
            "gen-one describe",
            "gen-two describe",
            "gen-three describe"
        ]
    );
}

#[test]
fn concurrent_refresh_describes_once_and_does_not_block_other_accounts() {
    let temp = tempfile::tempdir().unwrap();
    let slow_path = temp.path().join("slow-provider");
    let other_path = temp.path().join("other-provider");
    let slow_log = temp.path().join("slow.log");
    let other_log = temp.path().join("other.log");
    let gate = temp.path().join("release");
    install_atomic(&slow_path, &slow_log, &ONE);
    install_atomic(&other_path, &other_log, &ONE);
    let registry = Arc::new(registry_for(&[
        ("slow", &slow_path),
        ("other", &other_path),
    ]));
    registry.preflight_account("slow").unwrap();
    registry.preflight_account("other").unwrap();

    install_atomic(
        &slow_path,
        &slow_log,
        &Generation {
            name: "gen-slow",
            launch_output_v1: true,
            gate: Some(&gate),
        },
    );
    install_atomic(&other_path, &other_log, &TWO);

    let waiters = (0..4)
        .map(|_| {
            let registry = registry.clone();
            std::thread::spawn(move || registry.preflight_account("slow").unwrap())
        })
        .collect::<Vec<_>>();
    wait_for(&slow_log.with_extension("log.started"));

    // While one account's describe is in flight, another account refreshes.
    let other = {
        let registry = registry.clone();
        std::thread::spawn(move || registry.preflight_account("other").unwrap())
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !other.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let other_finished_during_slow_describe = other.is_finished();
    let slow_still_waiting = waiters.iter().all(|waiter| !waiter.is_finished());
    fs::write(&gate, b"").unwrap();
    assert!(
        other_finished_during_slow_describe,
        "another account's refresh must not wait for an in-flight describe"
    );
    assert!(slow_still_waiting);
    assert_eq!(other.join().unwrap().capabilities().provider_id, "gen-two");
    let results = waiters
        .into_iter()
        .map(|waiter| waiter.join().unwrap())
        .collect::<Vec<_>>();
    assert!(
        results
            .iter()
            .all(|endpoint| Arc::ptr_eq(endpoint, &results[0]))
    );
    assert_eq!(results[0].capabilities().provider_id, "gen-slow");
    assert_eq!(
        answered(&slow_log),
        ["gen-one describe", "gen-slow describe"]
    );
}

#[test]
fn relative_symlink_retarget_refreshes_cached_agreement() {
    let temp = tempfile::tempdir().unwrap();
    let releases = [temp.path().join("release-1"), temp.path().join("release-2")];
    let log = temp.path().join("answered.log");
    for release in &releases {
        fs::create_dir(release).unwrap();
    }
    install_atomic(&releases[0].join("provider"), &log, &ONE);
    install_atomic(&releases[1].join("provider"), &log, &TWO);
    let link: PathBuf = temp.path().join("current");
    std::os::unix::fs::symlink("release-1/provider", &link).unwrap();
    let registry = registry_for(&[("account", &link)]);
    assert_eq!(
        registry
            .preflight_account("account")
            .unwrap()
            .capabilities()
            .provider_id,
        "gen-one"
    );

    let staged = temp.path().join("current.staged");
    std::os::unix::fs::symlink("release-2/provider", &staged).unwrap();
    fs::rename(&staged, &link).unwrap();
    let new = registry.preflight_account("account").unwrap();
    assert_eq!(new.capabilities().provider_id, "gen-two");
    assert_eq!(
        new.canonical_executable(),
        releases[1].join("provider").canonicalize().unwrap()
    );
}
