#![cfg(unix)]

use std::process::Command;

#[test]
fn runner_requires_an_explicit_data_dir_in_every_environment() {
    let directory = tempfile::tempdir().unwrap();
    let data_home = directory.path().join("data");
    let config_home = directory.path().join("config");
    let cache_home = directory.path().join("cache");

    let output = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .args([
            "mailbox",
            "list",
            "--session-id",
            "development-data-dir-guard",
        ])
        .env_remove("OULIPOLY_DATA_DIR")
        .env_remove("OULIPOLY_PARENT_INVOCATION")
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_CACHE_HOME", &cache_home)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "runner unexpectedly ran without OULIPOLY_DATA_DIR"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OULIPOLY_DATA_DIR is not set"), "{stderr}");
    assert!(stderr.contains("export OULIPOLY_DATA_DIR="), "{stderr}");
    assert!(
        !data_home.join("oulipoly-agent-runner").exists(),
        "uninstalled runner materialized persistent state without OULIPOLY_DATA_DIR"
    );
}

#[test]
fn runner_requires_an_explicit_config_home_when_loading_application_config() {
    let directory = tempfile::tempdir().unwrap();
    let data_dir = directory.path().join("data");

    let output = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .args(["--model", "missing-model", "prompt"])
        .env("OULIPOLY_DATA_DIR", &data_dir)
        .env_remove("OULIPOLY_CONFIG_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("OULIPOLY_PARENT_INVOCATION")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "runner unexpectedly loaded application config without an explicit config home"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("OULIPOLY_CONFIG_HOME is not set"),
        "{stderr}"
    );
    assert!(stderr.contains("export OULIPOLY_CONFIG_HOME="), "{stderr}");
}
