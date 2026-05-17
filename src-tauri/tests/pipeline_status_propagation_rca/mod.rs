//! RCA Phase 1 reproduction harness for `rca-agent-runner-crashes-2026-05-16`,
//! Finding F1 (pipeline-status propagation). See the scenario file for the full
//! gap analysis and assertion rationale.
//!
//! This module owns the fixture + helper plumbing shared by the scenario
//! tests; the actual `#[test]` lives in
//! `rc1_abnormal_termination_under_tail_pipeline.rs`. The helpers are scoped
//! to this RCA so the test stays self-contained and does not touch any other
//! integration test's fixture surface.

use oulipoly_state::StateDb;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

pub mod rc1_abnormal_termination_under_tail_pipeline;

/// Tempdir-isolated fixture mirroring the shape used by
/// `pr_a_invocation_integration.rs::Fixture`, with three additions for this
/// RCA: an `XDG_STATE_HOME` redirect (so any filesystem-artifact recovery
/// channel writes under the tempdir), a fixture-unique marker string for
/// pgrep-based PID discovery, and the ability to drive multiple runs against
/// the same isolated config/data home.
pub struct PipelineFixture {
    pub dir: tempfile::TempDir,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub state_home: PathBuf,
    pub models_dir: PathBuf,
    pub fixture_marker: String,
}

impl PipelineFixture {
    /// Build a fixture whose fake provider executes `script_body`. The body
    /// is wrapped in a `#!/usr/bin/env bash` shebang.
    pub fn with_script_body(script_body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let state_home = dir.path().join("state");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&state_home).unwrap();
        fs::create_dir_all(&data_home).unwrap();

        let script_path = dir.path().join("fixture-provider.sh");
        fs::write(
            &script_path,
            format!("#!/usr/bin/env bash\n{script_body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        fs::write(
            models_dir.join("fixture.toml"),
            r#"[[providers]]
name = "fixture-provider"
"#,
        )
        .unwrap();
        fs::write(
            app_config_dir.join("providers.toml"),
            format!(
                r#"[fixture-provider]
command = "{}"
args = []
prompt_mode = "arg"
"#,
                script_path.display()
            ),
        )
        .unwrap();

        // The tempdir path is globally unique per fixture, so it makes a
        // reliable substring marker for pgrep -f when locating the runner
        // among other processes on the host.
        let fixture_marker = dir.path().display().to_string();

        Self {
            dir,
            config_home,
            data_home,
            state_home,
            models_dir,
            fixture_marker,
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    pub fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
    }

    /// Apply the fixture's XDG overrides to a `Command`.
    pub fn apply_env(&self, cmd: &mut Command) {
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("XDG_STATE_HOME", &self.state_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
    }
}

/// Poll the fixture's `state.db` for the first row whose `status='running'`
/// has the expected `model_name`. Returns the `invocation_uuid` once visible
/// or `None` on timeout.
pub fn poll_first_running_uuid(fixture: &PipelineFixture, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if fixture.db_path().exists()
            && let Ok(conn) = rusqlite::Connection::open(fixture.db_path())
        {
            let row = conn
                .query_row(
                    "SELECT invocation_uuid FROM invocations \
                     WHERE status = 'running' \
                     ORDER BY id ASC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok();
            if let Some(uuid) = row {
                return Some(uuid);
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    None
}

/// Search `/proc/*/cmdline` for the agent-runner process belonging to this
/// fixture. We match strictly on `argv[0]` (the first null-terminated token
/// in `/proc/<pid>/cmdline`) ending in `oulipoly-agent-runner`, AND on the
/// fixture-unique tempdir marker appearing somewhere later in `cmdline`.
/// The `argv[0]` check excludes the surrounding `bash -c '<full command>'`
/// wrapper, whose `argv[0]` is `bash` but whose cmdline still contains the
/// runner path as a substring of `argv[2]`.
///
/// Returns the PID, or `None` if no matching process is currently alive.
pub fn find_runner_pid_for_fixture(fixture: &PipelineFixture) -> Option<i32> {
    let self_pid = std::process::id() as i32;
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let pid: i32 = match entry.file_name().to_string_lossy().parse() {
            Ok(pid) => pid,
            Err(_) => continue,
        };
        if pid == self_pid {
            continue;
        }
        let cmdline = match fs::read(format!("/proc/{pid}/cmdline")) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let argv0_end = cmdline
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(cmdline.len());
        let argv0 = &cmdline[..argv0_end];
        let argv0_str = std::str::from_utf8(argv0).unwrap_or("");
        let argv0_base = argv0_str.rsplit('/').next().unwrap_or(argv0_str);
        if argv0_base != "oulipoly-agent-runner" {
            continue;
        }
        if !contains_subslice(&cmdline, fixture.fixture_marker.as_bytes()) {
            continue;
        }
        return Some(pid);
    }
    None
}

/// Send SIGKILL to `pid` via `kill(1)`. Returns `true` iff `kill` reported
/// success. The kernel marks the target terminated synchronously, but if the
/// target is a child of someone other than the test it will linger as a
/// zombie until its real parent reaps it — callers should not poll
/// `/proc/<pid>/` or `kill -0` for "gone" status; instead, wait on the
/// parent (e.g. the bash wrapper) via `Child::wait`.
pub fn sigkill_pid(pid: i32) -> bool {
    Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Test whether `haystack` contains `needle` as a contiguous byte
/// substring. Mirrors `slice::contains` for slice-of-slice on stable.
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Walk a directory recursively and return the paths of every regular file
/// found beneath it. Used to inspect `XDG_STATE_HOME` / `XDG_DATA_HOME` for
/// any filesystem-based terminal-status artifact the runtime may produce.
pub fn list_files_recursive(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                walk(&path, out);
            } else if meta.is_file() {
                out.push(path);
            }
        }
    }
}

/// A terminal-status "envelope" is any text artifact that names the
/// invocation UUID AND surfaces a terminal-status signal (one of
/// `terminal_reason`, `status=`, `success`, `exit_code`, or a marker that
/// looks like `OULIPOLY_RESULT=`/`OULIPOLY_TERMINAL=`/`OULIPOLY_FINAL=`).
///
/// This deliberately accepts a wide range of envelope shapes so that any of
/// the four Phase-3 fix-decision designs (envelope-last stdout marker,
/// mirrored stderr signal, `.result` filesystem artifact, hybrid) will turn
/// the test green without re-litigating the assertion.
pub fn text_contains_terminal_envelope(text: &str, invocation_uuid: &str) -> bool {
    text.lines().any(|line| {
        line.contains(invocation_uuid)
            && (line.contains("terminal_reason")
                || line.contains("\"status\"")
                || line.contains("status=")
                || line.contains("exit_code")
                || line.contains("\"success\"")
                || line.contains("OULIPOLY_RESULT")
                || line.contains("OULIPOLY_TERMINAL")
                || line.contains("OULIPOLY_FINAL")
                || line.contains("OULIPOLY_ENVELOPE"))
    })
}

/// Returns true if any file under `root` either (a) contains the invocation
/// UUID and a terminal-status keyword, or (b) has a name like
/// `<uuid>.result` / `<uuid>.terminal` / `<uuid>.final.json`.
pub fn filesystem_artifact_recovers_terminal(root: &Path, invocation_uuid: &str) -> bool {
    for path in list_files_recursive(root) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.contains(invocation_uuid)
            && (name.ends_with(".result")
                || name.ends_with(".terminal")
                || name.ends_with(".final")
                || name.ends_with(".final.json")
                || name.ends_with(".envelope.json"))
        {
            return true;
        }
        if let Ok(contents) = fs::read_to_string(&path)
            && text_contains_terminal_envelope(&contents, invocation_uuid)
        {
            return true;
        }
    }
    false
}
