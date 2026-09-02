//! AGE-162 Symptom 3 — `--usage` must NOT hard-fail when a model TOML
//! references a provider that has no stanza in `providers.toml`. It must
//! warn-and-skip the dangling reference so operators retain quota
//! visibility for providers that are present.
//!
//! Live incident (2026-05-19): `agents --usage` hard-failed on 11 cascading
//! missing-provider references (`gemini`, `droid`, `forge`, `seedance-*`,
//! `seedream-*`). Each missing reference printed:
//!
//!   provider <name> is missing from providers.toml; referenced by model <model>
//!
//! and exited non-zero — blocking operator visibility into routing state
//! during the very incident the visibility was needed for.
//!
//! Workaround applied today: 11 stub stanzas were appended to providers.toml.
//! Real fix needed: the parser must warn-and-skip missing-provider
//! references and continue rendering the table for the providers that ARE
//! present.
//!
//! Existing test `usage_exits_nonzero_when_model_references_provider_missing_from_providers_toml`
//! in `age15_usage_cli_characterization.rs` pins the CURRENT hard-fail
//! behavior. These tests pin the INTENDED warn-and-skip behavior and will
//! fail red against current code — that is the intended outcome for AGE-162
//! Phase 1.

#![cfg(unix)]

mod provider_authority_fixture;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    marker_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let agents_dir = app_config_dir.join("agents");
        let marker_dir = dir.path().join("markers");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&agents_dir).unwrap();
        fs::create_dir_all(&marker_dir).unwrap();

        Self {
            _dir: dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
            marker_dir,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env("OULIPOLY_CONFIG_HOME", &self.config_home);
        cmd.env("OULIPOLY_DATA_HOME", &self.data_home);
        cmd
    }

    fn usage_command(&self) -> Command {
        let mut cmd = self.command();
        cmd.arg("--usage").arg("--models-dir").arg(&self.models_dir);
        cmd
    }

    fn write_model(&self, model_name: &str, providers: &[&str]) {
        let body = providers
            .iter()
            .map(|provider| format!("[[providers]]\nname = \"{provider}\"\n"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    fn write_providers_toml(&self, body: &str) {
        fs::write(
            self.app_config_dir.join("providers.toml"),
            provider_authority_fixture::with_explicit_provider_authority(body),
        )
        .unwrap();
    }

    fn write_quota_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self._dir.path().join(name);
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }
}

fn run_usage(fixture: &Fixture) -> (i32, String, String) {
    let output = fixture.usage_command().output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn escape_toml(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn quota_script_static_json(marker: &Path, json: &str) -> String {
    format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf ran >> '{}'\ncat <<'JSON'\n{}\nJSON\n",
        marker.display(),
        json
    )
}

/// Symptom 3 primary repro: a single model TOML references one provider
/// that has a stanza and one that does not. `--usage` must exit 0, render
/// the present provider's row, and surface a non-fatal warning naming the
/// missing reference (so the operator knows about the dangling reference
/// without losing visibility into the providers that ARE present).
#[test]
fn age162_usage_warn_and_skips_when_model_references_provider_missing_from_providers_toml() {
    let fixture = Fixture::new();
    fixture.write_model("primary", &["claude-present", "gemini-missing"]);

    let quota_log = fixture.marker_dir.join("quota.log");
    let present_script = fixture.write_quota_script(
        "present-quota.sh",
        &quota_script_static_json(
            &quota_log,
            r#"{"windows":[{"label":"weekly","used_percent":13,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
        ),
    );

    fixture.write_providers_toml(&format!(
        "[claude-present]\ncommand = \"{}\"\nargs = []\nprompt_mode = \"arg\"\n\
         quota_script = \"{}\"\n",
        escape_toml("claude"),
        escape_toml(&present_script.display().to_string()),
    ));

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_eq!(
        code, 0,
        "AGE-162 Symptom 3: `--usage` must warn-and-skip dangling provider \
         references, not hard-fail. stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("claude-present"),
        "present provider must still appear in the rendered table:\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("weekly") || stdout.contains("13%"),
        "present provider's window data must still render so operators \
         retain quota visibility:\nstdout:\n{stdout}"
    );

    let warning_surface = format!("{stdout}\n{stderr}");
    assert!(
        warning_surface.contains("gemini-missing"),
        "the missing-provider warning must name the dangling reference \
         (`gemini-missing`):\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        warning_surface.to_lowercase().contains("warn")
            || warning_surface.to_lowercase().contains("skip")
            || warning_surface.to_lowercase().contains("missing"),
        "the missing-provider notice must be surfaced as a non-fatal warning \
         (matching `warn` / `skip` / `missing`):\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// Cascading-references repro: the live incident had 11 simultaneous
/// missing-provider references. `--usage` must absorb ALL of them
/// (warn-and-skip each), exit 0, and continue to render the present
/// provider's row. Mirrors the actual production failure shape that
/// required the 11-stub workaround.
#[test]
fn age162_usage_warn_and_skips_eleven_cascading_missing_provider_references() {
    let fixture = Fixture::new();
    let missing = [
        "gemini",
        "droid",
        "forge",
        "seedance-i2v",
        "seedance-i2v-fast",
        "seedance-r2v",
        "seedance-r2v-fast",
        "seedance-t2v",
        "seedance-t2v-fast",
        "seedream-i2i",
        "seedream-t2i",
    ];

    let mut model_providers = vec!["claude-present"];
    model_providers.extend_from_slice(&missing);
    fixture.write_model("primary", &model_providers);

    let present_script = fixture.write_quota_script(
        "present-quota.sh",
        &quota_script_static_json(
            &fixture.marker_dir.join("quota.log"),
            r#"{"windows":[{"label":"weekly","used_percent":7,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
        ),
    );
    fixture.write_providers_toml(&format!(
        "[claude-present]\ncommand = \"{}\"\nargs = []\nprompt_mode = \"arg\"\n\
         quota_script = \"{}\"\n",
        escape_toml("claude"),
        escape_toml(&present_script.display().to_string()),
    ));

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_eq!(
        code, 0,
        "AGE-162 Symptom 3 cascade: `--usage` must warn-and-skip ALL 11 \
         dangling references; got non-zero exit. stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("claude-present"),
        "the present provider's row must still render in the cascade case:\n\
         stdout:\n{stdout}"
    );

    let warning_surface = format!("{stdout}\n{stderr}");
    for name in missing {
        assert!(
            warning_surface.contains(name),
            "missing-provider warning must name each dangling reference; \
             `{name}` was absent.\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
}
