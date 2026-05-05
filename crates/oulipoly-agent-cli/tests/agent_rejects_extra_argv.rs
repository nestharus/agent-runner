use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const ARGV_REJECTION_PREFIX: &str = "error: 'agent' takes no arguments";

struct TempConfigHome {
    path: PathBuf,
}

impl TempConfigHome {
    fn new(test_name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after UNIX_EPOCH")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "oulipoly-agent-cli-{test_name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("temp config home should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn config_root(&self) -> PathBuf {
        self.path.join("oulipoly-agent-runner")
    }
}

impl Drop for TempConfigHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_agent<I, S>(test_name: &str, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let config_home = TempConfigHome::new(test_name);
    Command::new(env!("CARGO_BIN_EXE_agent"))
        .args(args)
        .env("XDG_CONFIG_HOME", config_home.path())
        .output()
        .expect("agent binary should run")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn agent_rejects_single_extra_flag() {
    let output = run_agent("single-extra-flag", ["--foo"]);

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains(ARGV_REJECTION_PREFIX),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn agent_rejects_positional_arg() {
    let output = run_agent("positional-arg", ["bar"]);

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains(ARGV_REJECTION_PREFIX),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn agent_rejects_multiple_args() {
    let output = run_agent("multiple-args", ["--foo", "bar"]);

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert!(
        stderr(&output).contains(ARGV_REJECTION_PREFIX),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn agent_runs_without_args() {
    let config_home = TempConfigHome::new("without-args");
    fs::create_dir_all(config_home.config_root()).expect("config root should be created");
    fs::write(config_home.config_root().join("config.toml"), "")
        .expect("empty app config should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_agent"))
        .env("XDG_CONFIG_HOME", config_home.path())
        .output()
        .expect("agent binary should run");
    let stderr = stderr(&output);

    assert!(
        matches!(output.status.code(), Some(2) | Some(3)),
        "{stderr}"
    );
    assert!(
        stderr.contains("'default_provider' must be set in"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("for 'agent' / '--new'"), "stderr: {stderr}");
    assert!(!stderr.contains(ARGV_REJECTION_PREFIX), "stderr: {stderr}");
}
