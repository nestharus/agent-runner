#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rusqlite::Connection;
use serde_json::Value;

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
fn new_flag_binds_exact_live_session_before_nested_registration_returns() {
    const SESSION_ID: &str = "ses_age284_live_fixture";

    let fixture = TempXdgHome::new();
    let artifacts = fixture.dir.path().join("age284-agent-bash");
    let runner = env!("CARGO_BIN_EXE_oulipoly-agent-runner");
    let script = fixture.write_script(
        "live-provider.sh",
        &format!(
            r#"request=$(cat || true)
request_id=$(printf '%s' "$request" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
case "${{1-}}" in
  describe)
    printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","ok":true,"result":{{"provider_id":"age284-fixture","display_name":"AGE-284 Fixture","contract_versions":["oulipoly.provider/v1"],"preferred_contract":"oulipoly.provider/v1","capabilities":{{"launch":false,"policy":false,"quota":false,"session":true,"session_enumerate":false,"terminal":false,"rotation":false,"discovery":false,"settings":false,"setup_brain":false,"setup":false,"migration":false}}}}}}\n' "$request_id"
    ;;
  session.capture)
    printf '%s' "$request" | grep -F '"live_report"' >/dev/null
    printf '%s' "$request" | grep -F '"provider_session_id":"{SESSION_ID}"' >/dev/null
    printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","ok":true,"result":{{"provider_session_id":"{SESSION_ID}","state":null,"artifacts":[]}}}}\n' "$request_id"
    ;;
  interactive)
    invocation_uuid=$(printf '%s' "$OULIPOLY_PARENT_INVOCATION" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
    mkdir -p '{artifacts}'
    printf '{{"owner_session_id":"{SESSION_ID}","owner_invocation_uuid":"%s","caller_chain":[]}}\n' "$invocation_uuid" > '{artifacts}/meta.json'
    : > '{artifacts}/log'
    printf '0\n' > '{artifacts}/rc'
    '{runner}' notify agent-bash-register \
      --handle age284-live-child \
      --delivery-mode async \
      --state-dir '{artifacts}' \
      --meta '{artifacts}/meta.json' \
      --log '{artifacts}/log' \
      --rc '{artifacts}/rc' \
      --json >/dev/null
    '{runner}' notify agent-bash-activate \
      --handle age284-live-child \
      --json >/dev/null
    '{runner}' notify agent-bash-complete \
      --caller-ppid "$$" \
      --handle age284-live-child \
      --state-dir '{artifacts}' \
      --meta '{artifacts}/meta.json' \
      --log '{artifacts}/log' \
      --rc '{artifacts}/rc' \
      --json >/dev/null
    printf 'AGE284_REGISTRATION_ACCEPTED\n' >&2
    sleep 0.1
    ;;
  *) exit 64 ;;
esac"#,
            artifacts = artifacts.display(),
        ),
    );
    fixture.write_config(r#"default_provider = "live""#);
    fixture.write_providers(&provider_entry("live1", &script, "interactive"));

    let output = fixture.run_new();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let invocation_index = stderr.find("OULIPOLY_INVOCATION=").unwrap();
    let session_index = stderr.find("OULIPOLY_SESSION=").unwrap();
    let registration_index = stderr.find("AGE284_REGISTRATION_ACCEPTED").unwrap();
    assert!(invocation_index < session_index, "{stderr}");
    assert!(session_index < registration_index, "{stderr}");
    assert_eq!(stderr.matches("OULIPOLY_SESSION=").count(), 1, "{stderr}");
    let marker: Value = serde_json::from_str(
        stderr
            .lines()
            .find_map(|line| line.strip_prefix("OULIPOLY_SESSION="))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(marker["provider_session_id"], SESSION_ID);

    let state_path = fixture
        .data_home
        .join("oulipoly-agent-runner")
        .join("state.db");
    let state = Connection::open(state_path).unwrap();
    let binding: (String, String) = state
        .query_row(
            "SELECT provider_session_id, provider_session_capture_method FROM invocations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(binding.0, SESSION_ID);
    assert_eq!(binding.1, "provider_live_report");
    let mailbox = Connection::open(
        fixture
            .data_home
            .join("oulipoly-agent-runner")
            .join("pid-identity.db"),
    )
    .unwrap();
    let listener_count: i64 = mailbox
        .query_row(
            "SELECT COUNT(*) FROM completion_event_listener WHERE session_id = ?1",
            [SESSION_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(listener_count, 1);
    let mail_count: i64 = mailbox
        .query_row(
            "SELECT COUNT(*) FROM mailbox WHERE session_id = ?1 AND kind = 'agent_bash_complete'",
            [SESSION_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(mail_count, 1);
}

#[test]
fn new_flag_fails_closed_when_provider_never_reports_a_live_session() {
    let fixture = TempXdgHome::new();
    let script = fixture.write_script(
        "no-live-session-provider.sh",
        r#"request=$(cat || true)
request_id=$(printf '%s' "$request" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
case "${1-}" in
  describe)
    printf '{"contract":"oulipoly.provider/v1","request_id":"%s","ok":true,"result":{"provider_id":"age284-no-session","display_name":"AGE-284 No Session","contract_versions":["oulipoly.provider/v1"],"preferred_contract":"oulipoly.provider/v1","capabilities":{"launch":false,"policy":false,"quota":false,"session":true,"session_enumerate":false,"terminal":false,"rotation":false,"discovery":false,"settings":false,"setup_brain":false,"setup":false,"migration":false}}}\n' "$request_id"
    ;;
  interactive)
    exit 0
    ;;
  *) exit 64 ;;
esac"#,
    );
    fixture.write_config(r#"default_provider = "silent""#);
    fixture.write_providers(&provider_entry("silent1", &script, "interactive"));

    let output = fixture.run_new();

    assert!(!output.status.success(), "{output:?}");
    let combined = combined_output(&output);
    assert!(
        combined.contains("live_session_identity_unavailable"),
        "{combined}"
    );
    assert!(!combined.contains("OULIPOLY_SESSION="), "{combined}");

    let state = Connection::open(
        fixture
            .data_home
            .join("oulipoly-agent-runner")
            .join("state.db"),
    )
    .unwrap();
    let invocation: (String, Option<String>, Option<String>) = state
        .query_row(
            "SELECT status, provider_session_id, error_category FROM invocations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(invocation.0, "failed");
    assert_eq!(invocation.1, None);
    assert_eq!(
        invocation.2.as_deref(),
        Some("live_session_identity_unavailable")
    );
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

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let combined = combined_output(&output);
    assert!(
        combined.contains("AGE31_PROVIDER_LAUNCHED arg=fixture-interactive"),
        "fixture provider marker missing from output: {combined}"
    );
    assert!(
        combined.contains("live_session_identity_unavailable"),
        "{combined}"
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

    assert_eq!(output.status.code(), Some(1), "{output:?}");
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
    assert!(
        combined.contains("live_session_identity_unavailable"),
        "{combined}"
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

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        combined_output(&output).contains("live_session_identity_unavailable"),
        "{output:?}"
    );
    assert_eq!(
        fs::read_to_string(argv_dump)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        vec!["--append-system-prompt", "AGE31 family policy"]
    );
}
