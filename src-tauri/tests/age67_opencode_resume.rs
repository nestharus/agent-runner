#![cfg(unix)]
//! AGE-67 — `agents --resume <ses_ id>` resumes a provider-backed session.
//!
//! End-to-end regression for the resume cwd-resolution defect: a non-UUID
//! provider session id (`ses_...`) must resolve its original workspace root
//! instead of failing `invalid session id`, and the wrong-id-kind error must
//! suggest an id that resume actually accepts.
//!
//! ## Declared roles
//!
//! Roles: formatter, mapper, orchestration, validator.
//!
//! - formatter: TOML/script/cwd-probe text builders return ready-to-write text.
//! - mapper: path-layout and incident-row builders assemble fixture structs.
//! - orchestration: fixture setup sequences directory/config/state writes and
//!   runs the resume binary.
//! - validator: the test asserts the wrong-id suggestion and the resumed argv.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/tests/age67_opencode_resume.rs
//!     role: adapter
//!     Translates:
//!       - executor-resume-cli-invocation-contract
//!       - oulipoly-state-incident-seeding-contract
//!       - provider-session-storage-config-contract
//!       - unix-process-fixture-recording-contract
//! ```

mod provider_authority_fixture;

use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const INVOCATION_ID: &str = "09c25e88-5879-46d5-ae44-360516c48fd0";
const PROVIDER_SESSION_ID: &str = "ses_1497e8a38ffed2xIQk3xxgOXRZ";
const MODEL: &str = "gpt-xhigh";
const PROVIDER: &str = "fixture-provider";

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    workspace: PathBuf,
    caller_cwd: PathBuf,
    argv_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let fixture = fixture_layout(tempfile::tempdir().unwrap());
        fixture.create_directories();
        fixture.write_config();
        fixture.seed_incident_state();
        fixture
    }

    fn create_directories(&self) {
        fs::create_dir_all(&self.models_dir).unwrap();
        fs::create_dir_all(self.data_home.join("oulipoly-agent-runner")).unwrap();
        fs::create_dir_all(&self.workspace).unwrap();
        fs::create_dir_all(&self.caller_cwd).unwrap();
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.current_dir(&self.caller_cwd);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_AUTO_WAKE");
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn write_config(&self) {
        let script = self.write_provider_script();
        fs::write(self.models_dir.join(format!("{MODEL}.toml")), model_toml()).unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            provider_authority_fixture::with_explicit_provider_authority(&providers_toml(
                &script,
                &self.workspace,
            )),
        )
        .unwrap();
    }

    fn write_provider_script(&self) -> PathBuf {
        self.write_script(
            "fixture-provider.sh",
            &provider_script_body(&self.argv_path),
        )
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.app_config_dir.join(name);
        fs::write(&path, executable_script(body)).unwrap();
        make_executable(&path);
        path
    }

    fn seed_incident_state(&self) {
        let db = StateDb::open(&self.db_path()).unwrap();
        let row_id = db.start_invocation(&incident_invocation_start()).unwrap();
        db.bind_invocation_provider_session_start(row_id, &incident_provider_binding())
            .unwrap();
        db.finalize_invocation(row_id, true, 0, None, None).unwrap();
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }
}

fn fixture_layout(dir: tempfile::TempDir) -> Fixture {
    let root = dir.path();
    let config_home = root.join("config");
    let data_home = root.join("data");
    let app_config_dir = config_home.join("oulipoly-agent-runner");
    let models_dir = app_config_dir.join("models");
    Fixture {
        workspace: root.join("original-workspace"),
        caller_cwd: root.join("caller-cwd"),
        argv_path: root.join("provider-argv.txt"),
        models_dir,
        app_config_dir,
        data_home,
        config_home,
        _dir: dir,
    }
}

fn incident_invocation_start() -> InvocationStart {
    InvocationStart {
        invocation_uuid: INVOCATION_ID.to_string(),
        model_name: MODEL.to_string(),
        provider_name: PROVIDER.to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
}

fn incident_provider_binding() -> ProviderSessionBinding {
    ProviderSessionBinding {
        provider_session_id: PROVIDER_SESSION_ID.to_string(),
        capture_method: "stdout_json_event",
        resume_input_id: None,
        provider_session_resolved_account: None,
    }
}

fn model_toml() -> String {
    format!(
        r#"[[providers]]
name = "{PROVIDER}"
args = []
interactive_args = ["model-interactive-arg", "--variant", "poison"]
"#
    )
}

fn providers_toml(script: &Path, workspace: &Path) -> String {
    format!(
        r#"[{PROVIDER}]
command = {}
args = ["run", "--dangerously-skip-permissions"]
interactive_args = ["run"]
prompt_mode = "arg"

[{PROVIDER}.resume]
kind = "flag"
flag = "--session"

[{PROVIDER}.session_storage]
kind = "script"
cwd_script = {}
"#,
        toml_string(&script.display().to_string()),
        toml_string(&cwd_probe_script(workspace))
    )
}

fn provider_script_body(argv_path: &Path) -> String {
    format!(
        r#"printf '%s\n' "$@" > "{}"
if [ "$#" -ne 3 ]; then
  printf 'expected native session resume argv, got argc=%s argv=%s\n' "$#" "$*" >&2
  exit 64
fi
if [ "$1" != "run" ] || [ "$2" != "--session" ] || [ "$3" != "{PROVIDER_SESSION_ID}" ]; then
  printf 'expected native session resume argv: run --session {PROVIDER_SESSION_ID}; got: %s\n' "$*" >&2
  exit 65
fi
printf 'provider resumed {PROVIDER_SESSION_ID}\n'
"#,
        argv_path.display()
    )
}

fn cwd_probe_script(workspace: &Path) -> String {
    format!(
        "printf '{{\"found\":true,\"cwd\":\"{}\"}}\\n'",
        workspace.display()
    )
}

fn executable_script(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn make_executable(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

fn run_resume(fixture: &Fixture, id: &str) -> Output {
    fixture.command().arg("--resume").arg(id).output().unwrap()
}

#[test]
fn wrong_id_suggestion_resumes_with_native_session_arg() {
    let fixture = Fixture::new();

    let wrong_id = run_resume(&fixture, INVOCATION_ID);
    assert_eq!(wrong_id.status.code(), Some(1), "{wrong_id:?}");
    let wrong_id_stderr = String::from_utf8_lossy(&wrong_id.stderr);
    assert!(
        wrong_id_stderr.contains(&format!(
            "wrong id kind: {INVOCATION_ID} is an agent-runner invocation id for provider {PROVIDER}"
        )),
        "{wrong_id_stderr}"
    );
    assert!(
        wrong_id_stderr.contains(&format!("Use `agents --resume {PROVIDER_SESSION_ID}`")),
        "{wrong_id_stderr}"
    );

    let resumed = run_resume(&fixture, PROVIDER_SESSION_ID);
    assert_eq!(
        resumed.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&resumed.stdout),
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed_stderr = String::from_utf8_lossy(&resumed.stderr);
    assert!(
        resumed_stderr.contains(&format!("[resume] -> {PROVIDER}")),
        "{resumed_stderr}"
    );
    assert_eq!(
        fs::read_to_string(&fixture.argv_path).unwrap(),
        format!("run\n--session\n{PROVIDER_SESSION_ID}\n")
    );
    assert!(
        !resumed_stderr.contains(&format!("invalid session id: {PROVIDER_SESSION_ID}")),
        "suggested provider session id must be accepted by resume metadata resolution; stderr={resumed_stderr}"
    );
    for diagnostic in [
        "storage-owner-not-found",
        "storage-ownership-ambiguous",
        "storage-ownership-indeterminate",
        "storage-owner-chain-ambiguous",
    ] {
        assert!(
            !resumed_stderr.contains(diagnostic),
            "single native lineage must not require ownership disambiguation; stderr={resumed_stderr}"
        );
    }
}
