//! Setup-only caller boundaries. Placeholder dependencies are never executed.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "native-setup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        eprintln!("setup fixture created: {}", dir.display());
        let deps = dir.join("deps");
        fs::create_dir_all(deps.join("node_modules/opencode-linux-x64/bin")).unwrap();
        fs::create_dir_all(deps.join("node_modules/@opencode-ai/plugin")).unwrap();
        fs::write(
            deps.join("node_modules/opencode-linux-x64/bin/opencode"),
            b"",
        )
        .unwrap();
        fs::write(
            deps.join("package-lock.json"),
            include_bytes!("../native/opencode/package-lock.json"),
        )
        .unwrap();
        fs::write(dir.join("bash.ts"), b"// setup fixture, never loaded\n").unwrap();
        fs::write(dir.join("agent-bash"), b"").unwrap();
        Self { dir }
    }

    fn setup(&self) -> Value {
        json!({
            "dir": self.dir.join("launch"),
            "deps": self.dir.join("deps"),
            "agent_bash_tool": self.dir.join("bash.ts"),
            "agent_bash_bin": self.dir.join("agent-bash"),
            "bash_allow": ["printf setup-fixture"],
            "model": "fixture/model",
            "provider": { "fixture": {} },
        })
    }

    fn call(&self, setup: &Value, close_receipt: bool) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_oulipoly-native-opencode-setup"))
            .env_clear()
            .current_dir(&self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Close the caller's actual pipe reader before releasing the input.
        // The child cannot provision or publish until it reads that input.
        if close_receipt {
            drop(child.stdout.take().unwrap());
        }
        writeln!(child.stdin.take().unwrap(), "{setup}").unwrap();
        child.wait_with_output().unwrap()
    }

    fn assert_provisioned(&self) {
        let config_dir = self.dir.join("launch/xdg/config/opencode");
        let config: Value =
            serde_json::from_slice(&fs::read(config_dir.join("opencode.json")).unwrap()).unwrap();
        assert_eq!(config["model"], "fixture/model");
        assert_eq!(config["provider"], json!({ "fixture": {} }));
        assert!(config_dir.join("acp-v2-endpoint.ts").is_file());
        assert!(config_dir.join("tool/bash.ts").is_file());
        assert!(config_dir.join("agent-bash/bash.ts").is_file());
        assert_eq!(
            fs::read_link(config_dir.join("node_modules")).unwrap(),
            self.dir.join("deps/node_modules")
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        match fs::remove_dir_all(&self.dir) {
            Ok(()) => eprintln!("setup fixture removed: {}", self.dir.display()),
            Err(error) => eprintln!("setup fixture retained: {}: {error}", self.dir.display()),
        }
    }
}

fn assert_input_invalid(fixture: &Fixture, setup: &Value, reason: &str) {
    let output = fixture.call(setup, false);
    assert_eq!(output.status.code(), Some(64));
    assert!(
        !fixture.dir.join("launch").exists(),
        "input-invalid refusal created the launch directory"
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["effects"], "none");
    assert!(receipt["refused"].as_str().unwrap().contains(reason));
}

#[test]
fn invalid_model_refuses_without_setup_writes() {
    let fixture = Fixture::new();
    let mut setup = fixture.setup();
    setup["model"] = json!("missing-separator");
    assert_input_invalid(&fixture, &setup, "model must be provider/model");
}

#[test]
fn missing_model_provider_refuses_without_setup_writes() {
    let fixture = Fixture::new();
    let mut setup = fixture.setup();
    setup["provider"] = json!({ "other": {} });
    assert_input_invalid(&fixture, &setup, "provider has no fixture");
}

#[test]
fn successful_receipt_describes_completed_provisioning() {
    let fixture = Fixture::new();
    let output = fixture.call(&fixture.setup(), false);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["endpoint"], "unix-socket");
    assert_eq!(
        receipt["config_dir"],
        json!(fixture.dir.join("launch/xdg/config/opencode"))
    );
    fixture.assert_provisioned();
}

#[test]
fn closed_receipt_recipient_is_non_pass_and_keeps_provisioning() {
    let fixture = Fixture::new();
    let output = fixture.call(&fixture.setup(), true);
    fixture.assert_provisioned();
    assert_eq!(output.status.code(), Some(74), "{output:?}");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        diagnostic.contains("provisioning completed"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("receipt delivery failed"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("do not replay"), "{diagnostic}");
}

#[test]
fn receipt_reports_the_named_list_policy_the_config_holds() {
    let fixture = Fixture::new();
    let output = fixture.call(&fixture.setup(), false);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    let native = r#"{"*":"deny","bash":{"*":"deny","printf setup-fixture":"allow"}}"#;
    assert_eq!(
        receipt["policy"],
        json!({ "bash": { "allow": ["printf setup-fixture"] }, "other": "deny", "native": native })
    );
    let config =
        fs::read_to_string(fixture.dir.join("launch/xdg/config/opencode/opencode.json")).unwrap();
    assert!(
        config.ends_with(&format!(r#""permission":{native}}}"#)),
        "{config}"
    );
}

#[test]
fn trusted_task_receipt_reports_open_bash_and_the_config_holds_it() {
    let fixture = Fixture::new();
    let mut setup = fixture.setup();
    setup.as_object_mut().unwrap().remove("bash_allow");
    setup["bash_authority"] = json!("trusted-task");
    let output = fixture.call(&setup, false);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    let native = r#"{"*":"deny","bash":{"*":"allow"}}"#;
    assert_eq!(
        receipt["policy"],
        json!({ "bash": "trusted-task", "other": "deny", "native": native })
    );
    let config =
        fs::read_to_string(fixture.dir.join("launch/xdg/config/opencode/opencode.json")).unwrap();
    assert!(
        config.ends_with(&format!(r#""permission":{native}}}"#)),
        "{config}"
    );
}

#[test]
fn bash_policy_is_one_explicit_form_or_refused_without_setup_writes() {
    let fixture = Fixture::new();
    let mut setup = fixture.setup();
    setup["bash_authority"] = json!("trusted-task");
    assert_input_invalid(
        &fixture,
        &setup,
        "bash_allow and bash_authority are exclusive",
    );
    setup["bash_authority"] = json!("all");
    assert_input_invalid(&fixture, &setup, "unknown variant");
    setup.as_object_mut().unwrap().remove("bash_authority");
    setup.as_object_mut().unwrap().remove("bash_allow");
    assert_input_invalid(&fixture, &setup, "bash_allow names no command");
}
