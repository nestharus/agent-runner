#![cfg(unix)]

use serde_json::{Value, json};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SESSION_ID: &str = "ses_public_export_fixture_01";

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts")
}

fn python3() -> &'static Path {
    for candidate in [Path::new("/usr/bin/python3"), Path::new("/bin/python3")] {
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("an absolute Python 3 interpreter is required for adapter tests");
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_executable(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn fake_prelude() -> &'static str {
    r#"#!/bin/sh
set -eu
if [ "${XDG_DATA_HOME-}" != "${EXPECTED_XDG_DATA_HOME-}" ]; then
  printf '%s' 'wrong isolated data root' >&2
  exit 97
fi
printf '%s' "$XDG_DATA_HOME" > "$OBSERVED_XDG_FILE"
"#
}

fn write_fake(path: &Path, body: &str) {
    write_executable(path, &format!("{}{}", fake_prelude(), body));
}

fn write_fake_export(path: &Path, body: &str) {
    write_fake(
        path,
        &format!(
            r#"if [ "$#" -ne 2 ] || [ "$1" != 'export' ] || [ "$2" != "$EXPECTED_SESSION_ID" ]; then
  printf '%s' 'wrong export argv' >&2
  exit 98
fi
{body}
"#
        ),
    );
}

fn export_json_body(value: &Value) -> String {
    format!(
        "printf '%s\\n' {}",
        shell_quote(&serde_json::to_string(value).unwrap())
    )
}

struct OpenCodeFixture {
    _temp: tempfile::TempDir,
    base_dir: PathBuf,
    selected_data_home: PathBuf,
    ambient_data_home: PathBuf,
    fake_bin_dir: PathBuf,
    observed_xdg: PathBuf,
    home: PathBuf,
    config_home: PathBuf,
    cache_home: PathBuf,
}

impl OpenCodeFixture {
    fn new(numbered_root: Option<&str>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let selected_data_home = match numbered_root {
            Some(name) => temp.path().join(name),
            None => temp.path().join("account-data"),
        };
        let base_dir = selected_data_home.join("opencode");
        let ambient_data_home = temp.path().join("wrong-ambient-data");
        let fake_bin_dir = temp.path().join("fake-bin");
        let observed_xdg = temp.path().join("observed-xdg");
        let home = temp.path().join("home");
        let config_home = temp.path().join("config");
        let cache_home = temp.path().join("cache");
        for directory in [
            &base_dir,
            &ambient_data_home,
            &fake_bin_dir,
            &home,
            &config_home,
            &cache_home,
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        Self {
            _temp: temp,
            base_dir,
            selected_data_home,
            ambient_data_home,
            fake_bin_dir,
            observed_xdg,
            home,
            config_home,
            cache_home,
        }
    }

    fn isolated_python_command(&self, opencode_bin: Option<&OsStr>) -> Command {
        let mut command = Command::new(python3());
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_CACHE_HOME", &self.cache_home)
            .env("XDG_DATA_HOME", &self.ambient_data_home)
            .env("PATH", &self.fake_bin_dir)
            .env("EXPECTED_XDG_DATA_HOME", &self.selected_data_home)
            .env("OBSERVED_XDG_FILE", &self.observed_xdg)
            .env("EXPECTED_SESSION_ID", SESSION_ID);
        if let Some(opencode_bin) = opencode_bin {
            command.env("OPENCODE_BIN", opencode_bin);
        }
        command
    }

    fn command(&self, opencode_bin: Option<&OsStr>) -> Command {
        let mut command = self.isolated_python_command(opencode_bin);
        command
            .arg(scripts_dir().join("opencode-cwd"))
            .arg(&self.base_dir)
            .arg(SESSION_ID);
        command
    }

    fn run_fake(&self, fake: &Path, body: &str) -> Output {
        let _ = fs::remove_file(&self.observed_xdg);
        write_fake_export(fake, body);
        let output = self.command(Some(fake.as_os_str())).output().unwrap();
        self.assert_selected_xdg_observed();
        output
    }

    fn assert_selected_xdg_observed(&self) {
        assert_eq!(
            fs::read_to_string(&self.observed_xdg).unwrap(),
            self.selected_data_home.to_string_lossy()
        );
    }
}

fn parse_response(output: &Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stderr.is_empty(),
        "adapter stderr must be empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_sensitive_output_absent(output: &Output, sentinel: &str) {
    assert!(!String::from_utf8_lossy(&output.stdout).contains(sentinel));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(sentinel));
}

fn assert_indeterminate(output: &Output, expected_error: &str) {
    let response = parse_response(output);
    assert_eq!(response["found"], false, "{response}");
    assert_eq!(response["error"], expected_error, "{response}");
    assert!(response.get("owned").is_none(), "{response}");
}

fn assert_owned_without_cwd(output: &Output, expected_error: &str) {
    let response = parse_response(output);
    assert_eq!(response["owned"], true, "{response}");
    assert_eq!(response["found"], false, "{response}");
    assert_eq!(response["error"], expected_error, "{response}");
}

fn wait_until_process_is_gone(pid: u32) {
    let process_path = PathBuf::from(format!("/proc/{pid}"));
    let deadline = Instant::now() + Duration::from_secs(3);
    while process_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!process_path.exists(), "process {pid} was not reaped");
}

#[test]
fn cwd_scripts_unchanged() {
    for script_name in ["claude-code-cwd", "codex-cwd", "opencode-cwd"] {
        let path = scripts_dir().join(script_name);
        let metadata = fs::metadata(&path).unwrap();
        assert!(metadata.is_file(), "{path:?} should be a file");
        assert_ne!(
            metadata.permissions().mode() & 0o111,
            0,
            "{path:?} should be executable"
        );
    }
}

#[test]
fn claude_code_cwd_decodes_project_directory_name() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let workspace = dir.path().join("workspace").join("rfq");
    fs::create_dir_all(&workspace).unwrap();
    let encoded = format!(
        "-{}",
        workspace
            .to_string_lossy()
            .trim_start_matches('/')
            .replace('/', "-")
    );
    let project_dir = dir.path().join(encoded);
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(project_dir.join(format!("{session_id}.jsonl")), "{}\n").unwrap();

    let output = Command::new(scripts_dir().join("claude-code-cwd"))
        .arg(dir.path())
        .arg(session_id)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["found"], true);
    assert_eq!(
        value["cwd"],
        workspace.canonicalize().unwrap().display().to_string()
    );
}

#[test]
fn codex_cwd_reads_payload_cwd_from_rollout_first_line() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let rollout_dir = dir.path().join("2026/05/10");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!("rollout-2026-05-10T00-00-00-{session_id}.jsonl")),
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"{}\"}}}}\n",
            workspace.path().display()
        ),
    )
    .unwrap();

    let output = Command::new(scripts_dir().join("codex-cwd"))
        .arg(dir.path())
        .arg(session_id)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["found"], true);
    assert_eq!(value["cwd"], workspace.path().to_string_lossy().as_ref());
}

#[test]
fn opencode_cwd_uses_public_export_without_private_sqlite_access() {
    let fixture = OpenCodeFixture::new(None);
    let workspace = fixture.home.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let poison = fixture.base_dir.join("opencode.db");
    let poison_contents = b"PRIVATE_DB_SENTINEL";
    fs::write(&poison, poison_contents).unwrap();
    let mut permissions = fs::metadata(&poison).unwrap().permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&poison, permissions).unwrap();
    let before = fs::metadata(&poison).unwrap();
    let fake = fixture.fake_bin_dir.join("public-export");
    let output = fixture.run_fake(
        &fake,
        &export_json_body(&json!({
            "info": {"id": SESSION_ID, "directory": workspace},
            "messages": [{"content": "SENSITIVE_EXPORT_SENTINEL"}]
        })),
    );

    let response = parse_response(&output);
    assert_eq!(response["owned"], true);
    assert_eq!(response["found"], true);
    assert_eq!(
        response["cwd"],
        workspace.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("SENSITIVE_EXPORT_SENTINEL"));
    let after = fs::metadata(&poison).unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    assert_eq!(after.atime(), before.atime());
    assert_eq!(after.atime_nsec(), before.atime_nsec());
    assert_eq!(after.permissions().mode() & 0o777, 0);
    let mut permissions = after.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(&poison, permissions).unwrap();
    assert_eq!(fs::read(&poison).unwrap(), poison_contents);

    let source = fs::read_to_string(scripts_dir().join("opencode-cwd")).unwrap();
    for forbidden in [
        "import sqlite3",
        "opencode.db",
        "SELECT directory",
        "FROM session",
    ] {
        assert!(
            !source.contains(forbidden),
            "forbidden private token: {forbidden}"
        );
    }
}

#[test]
fn opencode_cwd_reports_conclusive_export_not_found() {
    let fixture = OpenCodeFixture::new(None);
    let fake = fixture.fake_bin_dir.join("not-found");
    let output = fixture.run_fake(&fake, "printf '%s\\n' '  Session not found  ' >&2\nexit 1");

    assert_eq!(
        parse_response(&output),
        json!({"owned": false, "found": false})
    );
}

#[test]
fn opencode_cwd_keeps_export_ownership_when_directory_is_unusable() {
    let fixture = OpenCodeFixture::new(None);
    let fake = fixture.fake_bin_dir.join("unusable-cwd");
    let missing = fixture.home.join("missing-workspace");
    let loop_a = fixture.home.join("loop-a");
    let loop_b = fixture.home.join("loop-b");
    symlink(&loop_b, &loop_a).unwrap();
    symlink(&loop_a, &loop_b).unwrap();
    let cases = [
        (json!({"info": {"id": SESSION_ID}}), "opencode_missing_cwd"),
        (
            json!({"info": {"id": SESSION_ID, "directory": ""}}),
            "opencode_missing_cwd",
        ),
        (
            json!({"info": {"id": SESSION_ID, "directory": 42}}),
            "opencode_cwd_not_string",
        ),
        (
            json!({"info": {"id": SESSION_ID, "directory": "relative/path"}}),
            "opencode_cwd_not_absolute",
        ),
        (
            json!({"info": {"id": SESSION_ID, "directory": missing}}),
            "opencode_cwd_missing",
        ),
        (
            json!({"info": {"id": SESSION_ID, "directory": loop_a}}),
            "opencode_cwd_canonicalize_failed",
        ),
        (
            json!({
                "info": {
                    "id": SESSION_ID,
                    "directory": "/tmp/PRIVATE_CWD_SENTINEL\0tail"
                }
            }),
            "opencode_cwd_canonicalize_failed",
        ),
    ];

    for (export, expected_error) in cases {
        let output = fixture.run_fake(&fake, &export_json_body(&export));
        assert_owned_without_cwd(&output, expected_error);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("missing-workspace"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("loop-a"));
        assert_sensitive_output_absent(&output, "PRIVATE_CWD_SENTINEL");
    }
}

#[test]
fn opencode_cwd_does_not_treat_unknown_exit_one_as_not_owned() {
    let fixture = OpenCodeFixture::new(None);
    let fake = fixture.fake_bin_dir.join("unknown-exit-one");
    let output = fixture.run_fake(
        &fake,
        "printf '%s\\n' 'PRIVATE_FAILURE_SENTINEL' >&2\nexit 1",
    );

    assert_indeterminate(&output, "opencode_export_exit_failure");
    assert_sensitive_output_absent(&output, "PRIVATE_FAILURE_SENTINEL");

    let mixed_output = fixture.run_fake(
        &fake,
        "printf '%s' 'PRIVATE_MIXED_OUTPUT_SENTINEL'\nprintf '%s\n' 'Session not found' >&2\nexit 1",
    );
    assert_indeterminate(&mixed_output, "opencode_export_exit_failure");
    assert_sensitive_output_absent(&mixed_output, "PRIVATE_MIXED_OUTPUT_SENTINEL");
}

#[test]
fn opencode_cwd_classifies_export_process_failures_as_indeterminate() {
    let fixture = OpenCodeFixture::new(None);
    let fake = fixture.fake_bin_dir.join("process-failure");

    let exit_output = fixture.run_fake(&fake, "printf '%s\\n' 'PRIVATE_EXIT_SENTINEL' >&2\nexit 2");
    assert_indeterminate(&exit_output, "opencode_export_exit_failure");

    let signal_output = fixture.run_fake(&fake, "kill -TERM $$");
    assert_indeterminate(&signal_output, "opencode_export_signal");

    let absent = fixture.home.join("absent-opencode");
    let absent_output = fixture.command(Some(absent.as_os_str())).output().unwrap();
    assert_indeterminate(&absent_output, "opencode_export_spawn_failed");

    let malformed_output = fixture
        .command(Some(OsStr::new("'unterminated")))
        .output()
        .unwrap();
    assert_indeterminate(&malformed_output, "opencode_bin_invalid");
    assert!(!String::from_utf8_lossy(&exit_output.stdout).contains("PRIVATE_EXIT_SENTINEL"));
}

#[test]
fn opencode_cwd_requires_matching_export_info_identity() {
    let fixture = OpenCodeFixture::new(None);
    let fake = fixture.fake_bin_dir.join("identity");
    let oversized_integer = format!(
        "{{\"info\":{{\"id\":\"{SESSION_ID}\"}},\"count\":{}}}",
        "9".repeat(5_000)
    );
    let cases = [
        ("exit 0".to_string(), "opencode_export_malformed_json"),
        ("printf '\\377'".to_string(), "opencode_export_invalid_utf8"),
        (
            "printf '%s\\n' '{'".to_string(),
            "opencode_export_malformed_json",
        ),
        (
            export_json_body(&json!([])),
            "opencode_export_invalid_shape",
        ),
        (
            export_json_body(&json!({"other": {}})),
            "opencode_export_invalid_info",
        ),
        (
            export_json_body(&json!({"info": "bad"})),
            "opencode_export_invalid_info",
        ),
        (
            export_json_body(&json!({"info": {}})),
            "opencode_export_invalid_identity",
        ),
        (
            export_json_body(&json!({"info": {"id": 17}})),
            "opencode_export_invalid_identity",
        ),
        (
            export_json_body(&json!({"info": {"id": "ses_other"}})),
            "opencode_export_identity_mismatch",
        ),
        (
            format!(
                "printf '%s\\n' {}",
                shell_quote(&format!("{{\"info\":{{\"id\":\"{SESSION_ID}\"}}}}{{}}"))
            ),
            "opencode_export_malformed_json",
        ),
        (
            format!("printf '%s\\n' {}", shell_quote(&oversized_integer)),
            "opencode_export_malformed_json",
        ),
    ];

    for (body, expected_error) in cases {
        let output = fixture.run_fake(&fake, &body);
        assert_indeterminate(&output, expected_error);
    }
}

#[test]
fn opencode_cwd_emits_canonical_cwd_for_matching_export() {
    let fixture = OpenCodeFixture::new(None);
    let workspace = fixture.home.join("real-workspace");
    let workspace_link = fixture.home.join("workspace-link");
    fs::create_dir_all(&workspace).unwrap();
    symlink(&workspace, &workspace_link).unwrap();
    let fake = fixture.fake_bin_dir.join("canonical");
    let output = fixture.run_fake(
        &fake,
        &export_json_body(&json!({"info": {"id": SESSION_ID, "directory": workspace_link}})),
    );
    let canonical_cwd = workspace.canonicalize().unwrap();
    let expected = format!(
        "{{\"owned\":true,\"found\":true,\"cwd\":{}}}",
        serde_json::to_string(&canonical_cwd).unwrap()
    );

    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{expected}\n")
    );
}

#[test]
fn opencode_cwd_overrides_ambient_xdg_with_base_dir_parent() {
    let fixture = OpenCodeFixture::new(None);
    let workspace = fixture.home.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let ambient_poison = fixture.ambient_data_home.join("opencode/opencode.db");
    fs::create_dir_all(ambient_poison.parent().unwrap()).unwrap();
    fs::write(&ambient_poison, b"AMBIENT_PRIVATE_SENTINEL").unwrap();
    let before = fs::metadata(&ambient_poison).unwrap();
    let fake = fixture.fake_bin_dir.join("xdg-override");
    let output = fixture.run_fake(
        &fake,
        &export_json_body(&json!({
            "info": {"id": SESSION_ID, "directory": workspace}
        })),
    );

    assert_eq!(parse_response(&output)["owned"], true);
    let after = fs::metadata(&ambient_poison).unwrap();
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    assert_eq!(after.atime(), before.atime());
    assert_eq!(after.atime_nsec(), before.atime_nsec());
    assert_eq!(
        fs::read(&ambient_poison).unwrap(),
        b"AMBIENT_PRIVATE_SENTINEL"
    );
}

#[test]
fn opencode_cwd_honors_quoted_opencode_bin_argv() {
    let fixture = OpenCodeFixture::new(None);
    let workspace = fixture.home.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let fake = fixture
        ._temp
        .path()
        .join("bin with spaces")
        .join("fake opencode");
    write_fake(
        &fake,
        &format!(
            r#"if [ "$#" -ne 5 ] || [ "$1" != '--profile' ] || [ "$2" != 'account three' ] || [ "$3" != 'semi;colon' ] || [ "$4" != 'export' ] || [ "$5" != "$EXPECTED_SESSION_ID" ]; then
  printf '%s' 'wrong quoted argv' >&2
  exit 98
fi
{}
"#,
            export_json_body(&json!({"info": {"id": SESSION_ID, "directory": workspace}}))
        ),
    );
    let override_value = format!(
        "\"{}\" --profile \"account three\" \"semi;colon\"",
        fake.display()
    );
    let output = fixture
        .command(Some(OsStr::new(&override_value)))
        .output()
        .unwrap();

    fixture.assert_selected_xdg_observed();
    assert_eq!(parse_response(&output)["owned"], true);
}

#[test]
fn opencode_cwd_derives_numbered_executable_from_base_dir() {
    let fixture = OpenCodeFixture::new(Some(".opencode3"));
    let workspace = fixture.home.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let selected = fixture.fake_bin_dir.join("opencode3");
    let wrong_marker = fixture._temp.path().join("wrong-default-ran");
    write_fake_export(
        &selected,
        &export_json_body(&json!({"info": {"id": SESSION_ID, "directory": workspace}})),
    );
    write_fake_export(
        &fixture.fake_bin_dir.join("opencode"),
        &format!(
            "printf wrong > {}\nexit 99",
            shell_quote(&wrong_marker.display().to_string())
        ),
    );
    let output = fixture.command(None).output().unwrap();

    fixture.assert_selected_xdg_observed();
    assert_eq!(parse_response(&output)["owned"], true);
    assert!(!wrong_marker.exists());
}

#[test]
fn opencode_cwd_uses_default_executable_without_numbered_base() {
    let fixture = OpenCodeFixture::new(None);
    let workspace = fixture.home.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let selected = fixture.fake_bin_dir.join("opencode");
    let wrong_marker = fixture._temp.path().join("wrong-numbered-ran");
    write_fake_export(
        &selected,
        &export_json_body(&json!({"info": {"id": SESSION_ID, "directory": workspace}})),
    );
    write_fake_export(
        &fixture.fake_bin_dir.join("opencode3"),
        &format!(
            "printf wrong > {}\nexit 99",
            shell_quote(&wrong_marker.display().to_string())
        ),
    );
    let output = fixture.command(None).output().unwrap();

    fixture.assert_selected_xdg_observed();
    assert_eq!(parse_response(&output)["owned"], true);
    assert!(!wrong_marker.exists());
}

#[test]
fn opencode_cwd_bounds_export_timeout_and_kills_descendants() {
    let fixture = OpenCodeFixture::new(None);
    let fake = fixture.fake_bin_dir.join("timeout");
    let child_pid_path = fixture._temp.path().join("child-pid");
    let descendant_pid_path = fixture._temp.path().join("descendant-pid");
    write_fake_export(
        &fake,
        r#"printf '%s' "$$" > "$CHILD_PID_FILE"
/bin/sleep 30 &
printf '%s' "$!" > "$DESCENDANT_PID_FILE"
wait
"#,
    );
    let started = Instant::now();
    let output = fixture
        .command(Some(fake.as_os_str()))
        .env("OPENCODE_CWD_TIMEOUT_MS", "200")
        .env("CHILD_PID_FILE", &child_pid_path)
        .env("DESCENDANT_PID_FILE", &descendant_pid_path)
        .output()
        .unwrap();

    assert!(started.elapsed() < Duration::from_secs(3), "{output:?}");
    fixture.assert_selected_xdg_observed();
    assert_indeterminate(&output, "opencode_export_timeout");
    let child_pid: u32 = fs::read_to_string(child_pid_path).unwrap().parse().unwrap();
    let descendant_pid: u32 = fs::read_to_string(descendant_pid_path)
        .unwrap()
        .parse()
        .unwrap();
    wait_until_process_is_gone(child_pid);
    wait_until_process_is_gone(descendant_pid);
}

#[test]
fn opencode_cwd_capture_failure_closes_selector_and_kills_descendants() {
    let fixture = OpenCodeFixture::new(None);
    let fake = fixture.fake_bin_dir.join("capture-failure");
    let child_pid_path = fixture._temp.path().join("capture-child-pid");
    let descendant_pid_path = fixture._temp.path().join("capture-descendant-pid");
    let selector_closed_path = fixture._temp.path().join("selector-closed");
    write_fake_export(
        &fake,
        r#"printf '%s' "$$" > "$CHILD_PID_FILE"
/bin/sleep 30 &
printf '%s' "$!" > "$DESCENDANT_PID_FILE"
wait
"#,
    );
    let harness = r#"
import os
import runpy
import selectors
import sys
import time

class FailingSelector:
    def register(self, *_args):
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            if os.path.exists(os.environ["CHILD_PID_FILE"]) and os.path.exists(os.environ["DESCENDANT_PID_FILE"]):
                break
            time.sleep(0.01)
        raise OSError("injected selector failure")

    def close(self):
        with open(os.environ["SELECTOR_CLOSED_FILE"], "w", encoding="utf-8") as marker:
            marker.write("closed")

selectors.DefaultSelector = FailingSelector
sys.argv = sys.argv[1:]
runpy.run_path(sys.argv[0], run_name="__main__")
"#;
    let started = Instant::now();
    let output = fixture
        .isolated_python_command(Some(fake.as_os_str()))
        .arg("-c")
        .arg(harness)
        .arg(scripts_dir().join("opencode-cwd"))
        .arg(&fixture.base_dir)
        .arg(SESSION_ID)
        .env("CHILD_PID_FILE", &child_pid_path)
        .env("DESCENDANT_PID_FILE", &descendant_pid_path)
        .env("SELECTOR_CLOSED_FILE", &selector_closed_path)
        .output()
        .unwrap();

    assert!(started.elapsed() < Duration::from_secs(3), "{output:?}");
    fixture.assert_selected_xdg_observed();
    assert_indeterminate(&output, "opencode_export_capture_failed");
    assert_eq!(fs::read_to_string(selector_closed_path).unwrap(), "closed");
    let child_pid: u32 = fs::read_to_string(child_pid_path).unwrap().parse().unwrap();
    let descendant_pid: u32 = fs::read_to_string(descendant_pid_path)
        .unwrap()
        .parse()
        .unwrap();
    wait_until_process_is_gone(child_pid);
    wait_until_process_is_gone(descendant_pid);
}

#[test]
fn opencode_cwd_rejects_oversized_export_without_leaking_content() {
    let fixture = OpenCodeFixture::new(None);
    let fake = fixture.fake_bin_dir.join("oversized-stdout");
    let child_pid_path = fixture._temp.path().join("stdout-child-pid");
    let descendant_pid_path = fixture._temp.path().join("stdout-descendant-pid");
    write_fake_export(
        &fake,
        r#"printf '%s' "$$" > "$CHILD_PID_FILE"
/bin/sleep 30 &
printf '%s' "$!" > "$DESCENDANT_PID_FILE"
while :; do
  printf '%s' 'SENSITIVE_OVERSIZE_SENTINEL'
done
"#,
    );
    let started = Instant::now();
    let output = fixture
        .command(Some(fake.as_os_str()))
        .env("OPENCODE_CWD_STDOUT_LIMIT_BYTES", "256")
        .env("CHILD_PID_FILE", &child_pid_path)
        .env("DESCENDANT_PID_FILE", &descendant_pid_path)
        .output()
        .unwrap();

    assert!(started.elapsed() < Duration::from_secs(3), "{output:?}");
    fixture.assert_selected_xdg_observed();
    assert_indeterminate(&output, "opencode_export_stdout_limit");
    assert_sensitive_output_absent(&output, "SENSITIVE_OVERSIZE_SENTINEL");
    let child_pid: u32 = fs::read_to_string(child_pid_path).unwrap().parse().unwrap();
    let descendant_pid: u32 = fs::read_to_string(descendant_pid_path)
        .unwrap()
        .parse()
        .unwrap();
    wait_until_process_is_gone(child_pid);
    wait_until_process_is_gone(descendant_pid);
}

#[test]
fn opencode_cwd_rejects_oversized_stderr_and_kills_descendants() {
    let fixture = OpenCodeFixture::new(None);
    let fake = fixture.fake_bin_dir.join("oversized-stderr");
    let child_pid_path = fixture._temp.path().join("stderr-child-pid");
    let descendant_pid_path = fixture._temp.path().join("stderr-descendant-pid");
    write_fake_export(
        &fake,
        r#"printf '%s' "$$" > "$CHILD_PID_FILE"
/bin/sleep 30 &
printf '%s' "$!" > "$DESCENDANT_PID_FILE"
while :; do
  printf '%s' 'SENSITIVE_STDERR_SENTINEL' >&2
done
"#,
    );
    let started = Instant::now();
    let output = fixture
        .command(Some(fake.as_os_str()))
        .env("OPENCODE_CWD_STDERR_LIMIT_BYTES", "256")
        .env("CHILD_PID_FILE", &child_pid_path)
        .env("DESCENDANT_PID_FILE", &descendant_pid_path)
        .output()
        .unwrap();

    assert!(started.elapsed() < Duration::from_secs(3), "{output:?}");
    fixture.assert_selected_xdg_observed();
    assert_indeterminate(&output, "opencode_export_stderr_limit");
    assert_sensitive_output_absent(&output, "SENSITIVE_STDERR_SENTINEL");
    let child_pid: u32 = fs::read_to_string(child_pid_path).unwrap().parse().unwrap();
    let descendant_pid: u32 = fs::read_to_string(descendant_pid_path)
        .unwrap()
        .parse()
        .unwrap();
    wait_until_process_is_gone(child_pid);
    wait_until_process_is_gone(descendant_pid);
}

#[test]
fn opencode_cwd_closes_producer_stdin() {
    let fixture = OpenCodeFixture::new(None);
    let workspace = fixture.home.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let fake = fixture.fake_bin_dir.join("closed-stdin");
    write_fake_export(
        &fake,
        &format!(
            r#"if IFS= read -r inherited_input; then
  printf '%s' "PRIVATE_STDIN_SENTINEL:$inherited_input" >&2
  exit 99
fi
{}
"#,
            export_json_body(&json!({"info": {"id": SESSION_ID, "directory": workspace}}))
        ),
    );
    let mut child = fixture
        .command(Some(fake.as_os_str()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"MUST_NOT_REACH_PRODUCER\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    fixture.assert_selected_xdg_observed();
    assert_eq!(parse_response(&output)["owned"], true);
    assert_sensitive_output_absent(&output, "PRIVATE_STDIN_SENTINEL");
}
