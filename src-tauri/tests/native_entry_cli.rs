#![cfg(target_os = "linux")]

//! ## Declared roles
//!
//! `orchestration`, `validator`, `accessor`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/tests/native_entry_cli.rs
//!     role: intrinsic-surface
//!     Domain: native_entry_cli_tests
//!     Owns:
//!       - ordinary -m / agent launches selected onto a stand-in native caller
//!       - unmapped, malformed and unsupported-form refusals without caller or legacy State
//! ```
//!
//! The caller is a stand-in shell script that records its argv and prompt
//! and writes the caller's result files; no front door, owner or model runs.
//! "No legacy" is observed as: no OULIPOLY_DATA_DIR content created.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    data_dir: PathBuf,
    record: PathBuf,
}

const FAKE_CALLER: &str = r#"#!/bin/sh
# Stand-in for oulipoly-native-call: record argv/prompt, write result files.
rec="$FAKE_RECORD"
printf '%s\n' "$@" > "$rec.argv"
out=; prompt=; route=
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out="$2"; shift ;;
    --prompt-file) prompt="$2"; shift ;;
    --route) route="$2"; shift ;;
  esac
  shift
done
cp "$prompt" "$rec.prompt"
mkdir -m 700 "$out"
if [ "$route" = "no-answer-route" ]; then
  printf '{"class":"no-answer","front_door_exit":87}\n' > "$out/result.json"
  exit 1
fi
printf 'stand-in answer via %s' "$route" > "$out/final.md"
printf '{"class":"answered","front_door_exit":87}\n' > "$out/result.json"
exit 0
"#;

impl Fixture {
    fn new(native_toml: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let app_config = root.join("config/oulipoly-agent-runner");
        fs::create_dir_all(app_config.join("agents")).unwrap();
        let caller = root.join("fake-native-call");
        fs::write(&caller, FAKE_CALLER).unwrap();
        fs::set_permissions(&caller, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            app_config.join("agents/reviewer.md"),
            "---\ndescription: test agent\nmodel: opus~medium\n---\nREVIEWER INSTRUCTIONS\n",
        )
        .unwrap();
        let text = native_toml
            .replace("@CALLER@", caller.to_str().unwrap())
            .replace("@RUNS@", root.join("runs").to_str().unwrap());
        fs::write(app_config.join("native.toml"), text).unwrap();
        let fixture = Fixture {
            data_dir: root.join("data/oulipoly-agent-runner"),
            record: root.join("record"),
            _dir: dir,
            root,
        };
        fs::create_dir_all(fixture.root.join("work")).unwrap();
        fixture
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
            .args(args)
            .current_dir(self.root.join("work"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env_remove("OULIPOLY_CONFIG_HOME")
            .env("OULIPOLY_DATA_DIR", &self.data_dir)
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("HOME", &self.root)
            .env("FAKE_RECORD", &self.record)
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    fn argv(&self) -> Option<Vec<String>> {
        fs::read_to_string(self.record.with_extension("argv"))
            .ok()
            .map(|text| text.lines().map(str::to_owned).collect())
    }

    fn prompt(&self) -> String {
        fs::read_to_string(self.record.with_extension("prompt")).unwrap()
    }

    fn legacy_untouched(&self) -> bool {
        !self.data_dir.exists()
    }
}

fn value_after<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    argv.iter()
        .position(|arg| arg == flag)
        .and_then(|index| argv.get(index + 1))
        .map(String::as_str)
}

const CONFIG: &str = r#"
caller = "@CALLER@"
runs_dir = "@RUNS@"
deadline_s = 900

[models."codex~high"]
route = "sol-high"
bash = "trusted-task"
credential_codex_profile = "/nonexistent/.codex4"
children = ["luna-max"]
child_max_starts = 1

[models."opus~medium"]
route = "opus-medium"
bash_allow = ["git status"]

[models.quiet]
route = "no-answer-route"
bash = "trusted-task"
"#;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn direct_model_launch_runs_mapped_site_route_on_native_caller() {
    let fixture = Fixture::new(CONFIG);
    let output = fixture.run(&["-m", "codex~high", "fix", "the", "bug"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "stand-in answer via sol-high\n");
    let argv = fixture.argv().expect("native caller invoked");
    assert_eq!(value_after(&argv, "--route"), Some("sol-high"));
    assert_eq!(value_after(&argv, "--deadline"), Some("900"));
    assert_eq!(
        value_after(&argv, "--credential-codex-profile"),
        Some("/nonexistent/.codex4")
    );
    assert_eq!(value_after(&argv, "--child-route"), Some("luna-max"));
    assert_eq!(value_after(&argv, "--child-max-starts"), Some("1"));
    assert!(argv.contains(&"--trusted-task".to_owned()));
    let cwd = fs::canonicalize(fixture.root.join("work")).unwrap();
    assert_eq!(value_after(&argv, "--cwd"), Some(cwd.to_str().unwrap()));
    assert_eq!(fixture.prompt(), "fix the bug");
    assert!(fixture.legacy_untouched(), "legacy data dir was created");
}

#[test]
fn agent_frontmatter_model_selects_its_mapped_route() {
    let fixture = Fixture::new(CONFIG);
    let output = fixture.run(&["reviewer", "-i", "focus=tests", "look", "here"]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let argv = fixture.argv().expect("native caller invoked");
    assert_eq!(value_after(&argv, "--route"), Some("opus-medium"));
    let allow = value_after(&argv, "--allow-file").expect("allow-list policy");
    assert_eq!(fs::read_to_string(allow).unwrap(), r#"["git status"]"#);
    assert!(!argv.contains(&"--trusted-task".to_owned()));
    let prompt = fixture.prompt();
    assert!(prompt.starts_with("REVIEWER INSTRUCTIONS"), "{prompt}");
    assert!(
        prompt.contains("look here") && prompt.contains("\"focus\""),
        "{prompt}"
    );
    assert!(fixture.legacy_untouched());
}

#[test]
fn caller_outcome_is_returned_not_upgraded() {
    let fixture = Fixture::new(CONFIG);
    let output = fixture.run(&["-m", "quiet", "hi"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains("class no-answer"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn unmapped_model_is_refused_without_launch_or_substitute() {
    let fixture = Fixture::new(CONFIG);
    let output = fixture.run(&["-m", "claude~high", "hi"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(
        stderr(&output).contains("'claude~high' has no native route mapping"),
        "{}",
        stderr(&output)
    );
    assert!(fixture.argv().is_none(), "caller must not run");
    assert!(fixture.legacy_untouched(), "no legacy launch");
}

#[test]
fn agent_with_unmapped_model_is_refused() {
    let fixture = Fixture::new(&CONFIG.replace("[models.\"opus~medium\"]", "[models.other]"));
    let output = fixture.run(&["reviewer", "hi"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(
        stderr(&output).contains("selected by agent 'reviewer'"),
        "{}",
        stderr(&output)
    );
    assert!(fixture.argv().is_none());
    assert!(fixture.legacy_untouched());
}

#[test]
fn unsupported_forms_and_malformed_config_are_refused() {
    let fixture = Fixture::new(CONFIG);
    for args in [
        &["--resume", "00000000-0000-0000-0000-000000000000", "hi"][..],
        &["-m", "codex~high", "--pin-provider", "acct", "hi"][..],
        &["repl", "codex~high"][..],
    ] {
        let output = fixture.run(args);
        assert_eq!(
            output.status.code(),
            Some(3),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("not supported on the native entry"),
            "{}",
            stderr(&output)
        );
    }
    let broken = Fixture::new("caller = \"relative\"\nruns_dir = \"/r\"\n");
    let output = broken.run(&["-m", "codex~high", "hi"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(fixture.argv().is_none() && broken.argv().is_none());
    assert!(fixture.legacy_untouched() && broken.legacy_untouched());
}
