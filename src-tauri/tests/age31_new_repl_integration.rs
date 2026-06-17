#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct TempXdgHome {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
}

impl TempXdgHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(&app_config_dir).unwrap();

        Self {
            dir,
            config_home,
            data_home,
            app_config_dir,
        }
    }

    fn write_config(&self, contents: &str) {
        fs::write(self.app_config_dir.join("config.toml"), contents).unwrap();
    }

    fn write_providers(&self, contents: &str) {
        fs::write(self.app_config_dir.join("providers.toml"), contents).unwrap();
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn run_new(&self) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("--new")
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env_remove("OULIPOLY_DATA_DIR")
            .env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }
}

fn provider_entry(name: &str, script: &Path, marker: &str) -> String {
    format!(
        r#"[{name}]
command = "{}"
args = ["one-shot-only"]
interactive_args = ["{marker}"]
prompt_mode = "arg"
"#,
        script.display()
    )
}

fn primary_policy_token() -> String {
    ["cla", "ude"].concat()
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn new_flag_dispatches_to_default_provider_repl_with_fixture_provider() {
    let fixture = TempXdgHome::new();
    let script = fixture.write_script(
        "fixture-provider.sh",
        r#"printf 'AGE31_PROVIDER_LAUNCHED arg=%s\n' "${1:-missing}""#,
    );
    fixture.write_config(r#"default_provider = "fixture""#);
    fixture.write_providers(&provider_entry("fixture", &script, "fixture-interactive"));

    let output = fixture.run_new();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let combined = combined_output(&output);
    assert!(
        combined.contains("AGE31_PROVIDER_LAUNCHED arg=fixture-interactive"),
        "fixture provider marker missing from output: {combined}"
    );
}

#[test]
fn new_flag_with_missing_default_provider_returns_runtime_error() {
    let fixture = TempXdgHome::new();
    fixture.write_config(r#"diagnostics_model = "codex~high""#);

    let output = fixture.run_new();

    assert!(
        !output.status.success(),
        "missing default_provider should fail: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("'default_provider' must be set in"),
        "stderr should name the missing default_provider: {stderr}"
    );
    assert!(
        stderr.contains("for '--new'"),
        "stderr should name the surviving --new surface: {stderr}"
    );
    assert!(
        !stderr.contains("'agent'"),
        "stderr should not mention the removed standalone agent surface: {stderr}"
    );
}

#[test]
fn new_flag_with_provider_family_launches_balancer_selected_member() {
    let fixture = TempXdgHome::new();
    let claude2 = fixture.write_script(
        "claude2.sh",
        r#"printf 'MEMBER=claude2 arg=%s\n' "${1:-missing}""#,
    );
    let claude3 = fixture.write_script(
        "claude3.sh",
        r#"printf 'MEMBER=claude3 arg=%s\n' "${1:-missing}""#,
    );
    fixture.write_config(r#"default_provider = "claude""#);
    fixture.write_providers(
        &[
            provider_entry("claude2", &claude2, "claude2-interactive"),
            provider_entry("claude3", &claude3, "claude3-interactive"),
        ]
        .join("\n"),
    );

    let output = fixture.run_new();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let combined = combined_output(&output);
    // With an empty isolated state DB the balancer currently selects the
    // first suffixed family member, but the AGE-31 contract only needs selected launch.
    let member_markers = ["MEMBER=claude2 ", "MEMBER=claude3 "];
    let matches = member_markers
        .iter()
        .filter(|marker| combined.contains(**marker))
        .count();
    assert_eq!(
        matches, 1,
        "exactly one provider-family member should launch: {combined}"
    );
}

#[test]
fn new_flag_family_carrier_preserves_inferred_policy_from_command_basename() {
    let fixture = TempXdgHome::new();
    let argv_dump = fixture.dir.path().join("family-policy-argv.txt");
    let script = fixture.write_script(
        &format!("{}-family-policy.sh", primary_policy_token()),
        &format!(
            r#"printf '%s\n' "$@" > "{argv_dump}"
printf 'family policy ok\n'"#,
            argv_dump = argv_dump.display()
        ),
    );
    fixture.write_config(r#"default_provider = "family""#);
    fixture.write_providers(&format!(
        r#"[family2]
command = "{}"
args = ["one-shot-only"]
interactive_args = []
prompt_mode = "arg"
system_prompt_override = "AGE31 family policy"
"#,
        script.display()
    ));

    let output = fixture.run_new();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        fs::read_to_string(argv_dump)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        vec!["--append-system-prompt", "AGE31 family policy"]
    );
}
