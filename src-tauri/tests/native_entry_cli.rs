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
case "$route" in
  missing-final) ;;
  invalid-final) printf '\377' > "$out/final.md" ;;
  unreadable-final|cancelled-bad-final) mkdir "$out/final.md" ;;
  large-answer) awk 'BEGIN { for (i=0; i<1048576; i++) printf "x" }' > "$out/final.md" ;;
  *) printf 'stand-in answer via %s' "$route" > "$out/final.md" ;;
esac
case "$route" in
  cancelled-*)
    printf '{"class":"cancelled","front_door_exit":87}\n' > "$out/result.json"
    exit 5 ;;
esac
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

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        command
            .current_dir(self.root.join("work"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env_remove("OULIPOLY_CONFIG_HOME")
            .env("OULIPOLY_DATA_DIR", &self.data_dir)
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("HOME", &self.root)
            .env("FAKE_RECORD", &self.record)
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .stdin(std::process::Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
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
    assert!(value_after(&argv, "--credential-codex-profile").is_none());
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

// Oracles: README CLI Usage's agent-file example and prompt-source priority;
// ~/ai/AGENTS.md's defined-agent shape selects the frontmatter model without -m.
fn prompt_selections(fixture: &Fixture) -> Vec<(Vec<String>, &'static str, &'static str)> {
    let agent_file = fixture
        .root
        .join("config/oulipoly-agent-runner/agents/reviewer.md")
        .to_str()
        .unwrap()
        .to_owned();
    vec![
        (
            vec!["-a".into(), agent_file.clone()],
            "opus-medium",
            "REVIEWER INSTRUCTIONS\n\n\n",
        ),
        (
            vec!["reviewer".into()],
            "opus-medium",
            "REVIEWER INSTRUCTIONS\n\n\n",
        ),
        (vec!["-m".into(), "codex~high".into()], "sol-high", ""),
        (
            vec!["-a".into(), agent_file, "-m".into(), "codex~high".into()],
            "sol-high",
            "REVIEWER INSTRUCTIONS\n\n\n",
        ),
    ]
}

fn assert_prompt_request(fixture: &Fixture, output: &Output, route: &str, expected: &str) {
    assert_eq!(output.status.code(), Some(0), "{}", stderr(output));
    assert_eq!(fixture.prompt(), expected);
    assert_eq!(
        value_after(&fixture.argv().unwrap(), "--route"),
        Some(route)
    );
    assert!(fixture.legacy_untouched());
}

#[test]
fn prompt_composition_preserves_first_and_remaining_positional_components() {
    for words in [vec!["FIRST café\nline"], vec!["FIRST", "second", "第三"]] {
        let fixture = Fixture::new(CONFIG);
        for (mut args, route, prefix) in prompt_selections(&fixture) {
            args.extend(words.iter().map(|word| (*word).to_owned()));
            let output = fixture.command().args(&args).output().unwrap();
            assert_prompt_request(
                &fixture,
                &output,
                route,
                &format!("{prefix}{}", words.join(" ")),
            );
        }
    }
}

#[test]
fn prompt_composition_file_takes_priority_over_positionals_and_stdin() {
    use std::io::Write;
    use std::process::Stdio;

    let fixture = Fixture::new(CONFIG);
    let file = fixture.root.join("prompt.md");
    let body = "FILE café\nsecond line\n";
    fs::write(&file, body).unwrap();
    for (mut args, route, prefix) in prompt_selections(&fixture) {
        args.extend([
            "-f".into(),
            file.to_str().unwrap().into(),
            "ignored first".into(),
            "ignored tail".into(),
        ]);
        let mut child = fixture
            .command()
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"ignored stdin")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_prompt_request(&fixture, &output, route, &format!("{prefix}{body}"));
    }
}

#[test]
fn prompt_composition_stdin_is_used_only_without_file_or_positionals() {
    use std::io::Write;
    use std::process::Stdio;

    let fixture = Fixture::new(CONFIG);
    for (args, route, prefix) in prompt_selections(&fixture) {
        for positional in [None, Some("FIRST positional")] {
            let mut command = fixture.command();
            command.args(&args);
            if let Some(text) = positional {
                command.arg(text);
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let body = "STDIN café\nsecond line\n";
            child
                .stdin
                .take()
                .unwrap()
                .write_all(body.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert_prompt_request(
                &fixture,
                &output,
                route,
                &format!("{prefix}{}", positional.unwrap_or(body)),
            );
        }
    }
}

#[test]
fn prompt_composition_missing_input_refuses_without_caller_artifacts() {
    let fixture = Fixture::new(CONFIG);
    for (args, _, _) in prompt_selections(&fixture) {
        let output = fixture.command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
        assert!(
            stderr(&output).contains("Empty prompt"),
            "{}",
            stderr(&output)
        );
        assert!(fixture.argv().is_none());
        assert!(!fixture.root.join("runs").exists());
        assert!(fixture.legacy_untouched());
    }
}

#[test]
fn prompt_composition_unmapped_agent_or_model_refuses_before_prompt_read() {
    let fixture = Fixture::new(&CONFIG.replace("[models.\"opus~medium\"]", "[models.other]"));
    let missing_prompt = fixture.root.join("absent-prompt.md");
    for (mut args, route, _) in prompt_selections(&fixture) {
        // Explicit -m keeps precedence over agent frontmatter but must map too.
        if route == "sol-high" {
            let model = args.iter().position(|arg| arg == "-m").unwrap() + 1;
            args[model] = "unmapped-explicit".into();
        }
        args.extend(["-f".into(), missing_prompt.to_str().unwrap().into()]);
        let output = fixture.command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
        let text = stderr(&output);
        assert!(
            text.contains("has no native route mapping") && text.contains("no substitute model"),
            "{text}"
        );
        assert!(!text.contains("Failed to read prompt file"), "{text}");
        assert!(fixture.argv().is_none());
        assert!(!fixture.root.join("runs").exists());
        assert!(fixture.legacy_untouched());
    }
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

#[test]
fn unresolved_config_root_is_refused_before_legacy_scheduling() {
    let fixture = Fixture::new(CONFIG);
    // Even a regressed selector cannot spawn a legacy maintenance worker:
    // its data root is an ordinary file, so scheduling must fail before spawn.
    fs::create_dir_all(fixture.data_dir.parent().unwrap()).unwrap();
    fs::write(&fixture.data_dir, "data-root blocker").unwrap();
    let output = fixture
        .command()
        .env_remove("XDG_CONFIG_HOME")
        .args(["-m", "codex~high", "hi"])
        .output()
        .unwrap();
    eprintln!(
        "config-root control: entry {:?}; {}",
        output.status.code(),
        stderr(&output)
    );
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(stderr(&output).contains("cannot determine native configuration selection"));
    assert!(stderr(&output).contains("OULIPOLY_CONFIG_HOME is not set"));
    assert!(!stderr(&output).contains("OULIPOLY_MAINTENANCE_GAP"));
    assert!(fixture.argv().is_none());
    assert!(!fixture.root.join("runs").exists());
    assert_eq!(
        fs::read_to_string(&fixture.data_dir).unwrap(),
        "data-root blocker"
    );

    // A malformed executable-adjacent paths file has priority over a valid
    // environment config root. Refuse its actual resolver error too.
    let image = fixture.root.join("image");
    fs::create_dir(&image).unwrap();
    let executable = image.join("oulipoly-agent-runner");
    fs::copy(env!("CARGO_BIN_EXE_oulipoly-agent-runner"), &executable).unwrap();
    fs::write(image.join("config.toml"), "not valid TOML").unwrap();
    let command = fixture.command();
    // Reuse the isolated environment, changing just the executable.
    let output = Command::new(&executable)
        .envs(
            command
                .get_envs()
                .filter_map(|(name, value)| value.map(|value| (name, value))),
        )
        .env_remove("OULIPOLY_CONFIG_HOME")
        .env_remove("OULIPOLY_PARENT_INVOCATION")
        .current_dir(fixture.root.join("work"))
        .stdin(std::process::Stdio::null())
        .args(["-m", "codex~high", "hi"])
        .output()
        .unwrap();
    eprintln!(
        "config-root control: entry {:?}; {}",
        output.status.code(),
        stderr(&output)
    );
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(stderr(&output).contains("cannot determine native configuration selection"));
    assert!(stderr(&output).contains("Could not parse runtime paths file"));
    assert!(stderr(&output).contains(image.join("config.toml").to_str().unwrap()));
    assert!(!stderr(&output).contains("OULIPOLY_MAINTENANCE_GAP"));
    assert!(fixture.argv().is_none());
    assert!(!fixture.root.join("runs").exists());
}

fn outcome_records(fixture: &Fixture, output: &Output, class: &str, code: i32) -> PathBuf {
    let text = stderr(output);
    eprintln!(
        "presentation control: entry {:?}; stdout {} bytes; {text}",
        output.status.code(),
        output.stdout.len()
    );
    assert!(
        text.contains(&format!(
            "class {class}; caller exit {code}; front door exit 87; records "
        )),
        "{text}"
    );
    let runs: Vec<_> = fs::read_dir(fixture.root.join("runs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    assert_eq!(runs.len(), 1);
    let out = &runs[0];
    assert!(text.contains(out.to_str().unwrap()), "{text}");
    let result: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("result.json")).unwrap()).unwrap();
    assert_eq!(result["class"], class);
    assert_eq!(result["front_door_exit"], 87);
    assert_eq!(fixture.prompt(), "hi");
    assert!(fixture.legacy_untouched());
    out.clone()
}

#[test]
fn answer_read_failures_are_visible_with_caller_outcome_and_records() {
    for route in ["missing-final", "invalid-final", "unreadable-final"] {
        let fixture = Fixture::new(&CONFIG.replace("sol-high", route));
        let output = fixture.run(&["-m", "codex~high", "hi"]);
        assert_eq!(
            output.status.code(),
            Some(6),
            "{route}: {}",
            stderr(&output)
        );
        assert!(output.stdout.is_empty());
        let text = stderr(&output);
        assert!(
            text.contains("answer presentation failed: cannot read"),
            "{text}"
        );
        assert!(text.contains("entry exit 6"), "{text}");
        let out = outcome_records(&fixture, &output, "answered", 0);
        assert!(text.contains(out.join("final.md").to_str().unwrap()));
    }
}

#[test]
fn stdout_loss_is_visible_with_caller_outcome_and_retained_answer() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;

    for broken_socket in [false, true] {
        let fixture = Fixture::new(&CONFIG.replace("sol-high", "large-answer"));
        let sink = if broken_socket {
            let (writer, reader) = UnixStream::pair().unwrap();
            drop(reader);
            Stdio::from(OwnedFd::from(writer))
        } else {
            Stdio::from(
                fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            )
        };
        let output = fixture
            .command()
            .args(["-m", "codex~high", "hi"])
            .stdout(sink)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
        let text = stderr(&output);
        assert!(
            text.contains("answer presentation failed: cannot write answer to stdout"),
            "{text}"
        );
        assert!(
            text.contains(if broken_socket {
                "Broken pipe"
            } else {
                "No space left"
            }),
            "{text}"
        );
        let out = outcome_records(&fixture, &output, "answered", 0);
        assert_eq!(fs::metadata(out.join("final.md")).unwrap().len(), 1048576);
    }
}

#[test]
fn nonanswered_diagnostic_text_and_read_loss_never_upgrade_caller_code() {
    for route in ["cancelled-text", "cancelled-bad-final"] {
        let fixture = Fixture::new(&CONFIG.replace("sol-high", route));
        let output = fixture.run(&["-m", "codex~high", "hi"]);
        assert_eq!(output.status.code(), Some(5), "{}", stderr(&output));
        outcome_records(&fixture, &output, "cancelled", 5);
        if route == "cancelled-text" {
            assert_eq!(stdout(&output), "stand-in answer via cancelled-text\n");
            assert!(!stderr(&output).contains("answer presentation failed"));
        } else {
            assert!(output.stdout.is_empty());
            assert!(stderr(&output).contains("answer presentation failed: cannot read"));
            assert!(stderr(&output).contains("entry exit 5"));
        }
    }
}

#[test]
fn nonanswered_stdout_loss_preserves_nonzero_caller_code() {
    let fixture = Fixture::new(&CONFIG.replace("sol-high", "cancelled-text"));
    let file = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let output = fixture
        .command()
        .args(["-m", "codex~high", "hi"])
        .stdout(file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(5), "{}", stderr(&output));
    assert!(stderr(&output).contains("answer presentation failed"));
    assert!(stderr(&output).contains("No space left"));
    assert!(stderr(&output).contains("entry exit 5"));
    let out = outcome_records(&fixture, &output, "cancelled", 5);
    assert_eq!(
        fs::read_to_string(out.join("final.md")).unwrap(),
        "stand-in answer via cancelled-text"
    );
}

#[test]
fn old_credential_config_refuses_without_invoking_caller() {
    let fixture = Fixture::new(&CONFIG.replace(
        "children =",
        "credential_codex_profile = \"/unread\"\nchildren =",
    ));
    let output = fixture.run(&["-m", "codex~high", "q"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(stderr(&output).contains("unknown field"));
    assert!(fixture.argv().is_none());
    assert!(!fixture.data_dir.exists());
}
