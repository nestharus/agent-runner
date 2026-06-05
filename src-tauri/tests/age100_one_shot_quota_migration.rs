#![cfg(unix)]

use oulipoly_state::StateDb;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FORCE_KIND: &str = "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();

        Self {
            dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn write_model(&self, model_name: &str, providers: &[&str]) {
        let mut body = String::new();
        for provider in providers {
            body.push_str(&format!(
                r#"[[providers]]
name = "{provider}"
args = []

"#
            ));
        }
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    fn write_providers(&self, providers: &[(&str, &Path, &str)]) {
        let mut body = String::new();
        for (provider, marker, failure) in providers {
            let command = self.write_script(
                &format!("{provider}-command.sh"),
                &format!(
                    "printf '%s\\n' ran >> {}\nprintf '%s\\n' '{}' >&2\nexit 42",
                    toml_string(&marker.display().to_string()),
                    failure
                ),
            );
            body.push_str(&format!(
                r#"[{provider}]
command = {}
args = []
prompt_mode = "arg"

"#,
                toml_string(&command.display().to_string())
            ));
        }
        fs::write(self.app_config_dir.join("providers.toml"), body).unwrap();
    }

    fn run_one_shot(&self, model_name: &str) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(model_name)
            .arg("prompt");
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env("HOME", &self.data_home);
        cmd.env(FORCE_KIND, "QuotaExhaustedInband");
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn line_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|content| content.lines().count())
        .unwrap_or(0)
}

fn assert_contains_keys(value: &Value, keys: &[&str], context: &str) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{context} must be an object: {value}"));
    for key in keys {
        assert!(
            object.contains_key(*key),
            "{context} missing required key {key}: {value}"
        );
    }
}

fn assert_pool_exhausted_failure_stdout(stdout: &str) {
    assert!(
        !stdout
            .lines()
            .any(|line| line.starts_with("OULIPOLY_RESULT=")),
        "pre-invocation pool exhaustion must not emit OULIPOLY_RESULT:\n{stdout}"
    );

    let failure_lines = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_FAILURE="))
        .collect::<Vec<_>>();
    assert_eq!(
        failure_lines.len(),
        1,
        "expected exactly one OULIPOLY_FAILURE line in stdout:\n{stdout}"
    );

    let payload = serde_json::from_str::<Value>(failure_lines[0])
        .unwrap_or_else(|err| panic!("invalid OULIPOLY_FAILURE JSON: {err}\n{stdout}"));
    assert_contains_keys(
        &payload,
        &[
            "failure_kind",
            "stage",
            "status",
            "success",
            "exit_code",
            "terminal_reason",
            "error_category",
            "finished_at",
            "message",
            "detail",
            "agent_runner_invocation_id",
            "provider_name",
            "provider_session_id",
            "agent_runner_chain_id",
        ],
        "OULIPOLY_FAILURE payload",
    );
    assert_eq!(payload["failure_kind"], "pre_invocation");
    assert_eq!(payload["stage"], "pool_exhausted");
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["success"], false);
    assert!(payload["exit_code"].is_null(), "{payload}");
    assert!(payload["error_category"].is_null(), "{payload}");
    assert_eq!(payload["terminal_reason"], "pre_invocation_failure");
    for field in [
        "agent_runner_invocation_id",
        "provider_name",
        "provider_session_id",
        "agent_runner_chain_id",
    ] {
        assert!(payload[field].is_null(), "{field} must be null: {payload}");
    }

    assert_contains_keys(
        &payload["detail"],
        &[
            "attempted_providers",
            "model_name",
            "provider_index",
            "reason",
        ],
        "OULIPOLY_FAILURE detail",
    );
    let attempted_providers = payload["detail"]["attempted_providers"]
        .as_array()
        .unwrap_or_else(|| panic!("detail.attempted_providers must be an array: {payload}"));
    for provider in ["age100-a", "age100-b"] {
        assert!(
            attempted_providers
                .iter()
                .any(|attempted| attempted.as_str() == Some(provider)),
            "detail.attempted_providers must contain {provider}: {payload}"
        );
    }
}

#[test]
fn one_shot_all_pool_members_quota_exhausted_returns_blocked_all_providers_exhausted() {
    let fixture = Fixture::new();
    let first_marker = fixture.dir.path().join("one-shot-a.txt");
    let second_marker = fixture.dir.path().join("one-shot-b.txt");
    fixture.write_model("age100-one-shot", &["age100-a", "age100-b"]);
    fixture.write_providers(&[
        (
            "age100-a",
            &first_marker,
            "quota exhausted for one-shot provider a",
        ),
        (
            "age100-b",
            &second_marker,
            "quota exhausted for one-shot provider b",
        ),
    ]);

    let output = fixture.run_one_shot("age100-one-shot");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_pool_exhausted_failure_stdout(&stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("BLOCKED:all-providers-exhausted")
            || stderr.contains("all providers in pool age100-one-shot are quota-exhausted"),
        "{stderr}"
    );
    assert_eq!(line_count(&first_marker), 1);
    assert_eq!(line_count(&second_marker), 1);

    let db = fixture.open_db();
    for provider in ["age100-a", "age100-b"] {
        assert!(
            db.get_quota(provider)
                .unwrap()
                .and_then(|quota| quota.next_available_at)
                .is_some(),
            "{provider} should have next_available_at set after a quota \
             exhaustion (AGE-163 WU-A.4 moved the durable working-set write \
             from exhausted_at to next_available_at via \
             apply_post_failure_forensics)"
        );
    }
}
