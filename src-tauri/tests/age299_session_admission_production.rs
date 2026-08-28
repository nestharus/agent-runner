#![cfg(unix)]

use rusqlite::Connection;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct Fixture {
    directory: tempfile::TempDir,
    config_home: std::path::PathBuf,
    data_home: std::path::PathBuf,
    data_dir: std::path::PathBuf,
    models_dir: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config_home = directory.path().join("config");
        let data_home = directory.path().join("data");
        let data_dir = data_home.join("oulipoly-agent-runner");
        let models_dir = config_home.join("oulipoly-agent-runner/models");
        fs::create_dir_all(&models_dir).unwrap();
        Self {
            directory,
            config_home,
            data_home,
            data_dir,
            models_dir,
        }
    }

    fn runner_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        command
            .env("OULIPOLY_DATA_DIR", &self.data_dir)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("HOME", self.directory.path())
            .env_remove("OULIPOLY_PARENT_INVOCATION");
        command
    }

    fn sidecar(&self) -> std::path::PathBuf {
        let sidecar = self.data_dir.join("pid-identity.db");
        assert!(sidecar.starts_with(self.directory.path()));
        sidecar
    }
}

#[test]
fn initial_provider_observes_durable_admission_before_launch() {
    let fixture = Fixture::new();
    let app_config = fixture.config_home.join("oulipoly-agent-runner");
    let provider = fixture.directory.path().join("admission-provider.sh");
    write_executable(
        &provider,
        r#"python3 - <<'PY'
import os
import sqlite3

path = os.path.join(os.environ["XDG_DATA_HOME"], "oulipoly-agent-runner", "pid-identity.db")
connection = sqlite3.connect(path)
rows = connection.execute(
    "SELECT session_id, state FROM session_admission_queue WHERE state = 'launching'"
).fetchall()
assert rows == [(None, "launching")], rows
PY
printf '%s\n' 'initial admission observed'
"#,
    );
    fs::write(
        fixture.models_dir.join("admission-model.toml"),
        "[[providers]]\nname = \"admission-provider\"\nargs = []\n",
    )
    .unwrap();
    fs::write(
        app_config.join("providers.toml"),
        format!(
            "[admission-provider]\ncommand = {}\nargs = []\nprompt_mode = \"arg\"\n",
            toml_string(&provider.display().to_string())
        ),
    )
    .unwrap();

    let output = fixture
        .runner_command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("admission-model")
        .arg("admission probe")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let sidecar = fixture.sidecar();
    let connection = Connection::open(sidecar).unwrap();
    let (session_id, state, generation): (Option<String>, String, Option<String>) = connection
        .query_row(
            "SELECT session_id, state, runtime_generation_uuid
             FROM session_admission_queue",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(session_id, None);
    assert_eq!(state, "settled");
    assert!(generation.is_some());
}

#[test]
fn pressure_keeps_initial_request_alive_and_visibly_queued() {
    let fixture = Fixture::new();
    let app_config = fixture.config_home.join("oulipoly-agent-runner");
    let launched = fixture.directory.path().join("provider-launched");
    let provider = fixture.directory.path().join("queued-provider.sh");
    write_executable(
        &provider,
        &format!(
            "printf launched > {}\n",
            toml_string(&launched.display().to_string())
        ),
    );
    fs::write(
        fixture.models_dir.join("queued-model.toml"),
        "[[providers]]\nname = \"queued-provider\"\nargs = []\n",
    )
    .unwrap();
    fs::write(
        app_config.join("providers.toml"),
        format!(
            "[queued-provider]\ncommand = {}\nargs = []\nprompt_mode = \"arg\"\n",
            toml_string(&provider.display().to_string())
        ),
    )
    .unwrap();

    let mut child = fixture
        .runner_command()
        .env(
            "OULIPOLY_SESSION_ADMISSION_MIN_AVAILABLE_MEMORY_BYTES",
            u64::MAX.to_string(),
        )
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("queued-model")
        .arg("queued admission probe")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let sidecar = fixture.sidecar();
    let deadline = Instant::now() + Duration::from_secs(10);
    let state = loop {
        if sidecar.exists() {
            let connection = Connection::open(&sidecar).unwrap();
            if let Ok(state) =
                connection.query_row("SELECT state FROM session_admission_queue", [], |row| {
                    row.get::<_, String>(0)
                })
            {
                break state;
            }
        }
        assert!(
            Instant::now() < deadline,
            "request never entered admission queue"
        );
        thread::sleep(Duration::from_millis(25));
    };

    assert_eq!(state, "queued");
    thread::sleep(Duration::from_millis(500));
    assert!(child.try_wait().unwrap().is_none(), "queued request exited");
    assert!(
        !launched.exists(),
        "provider started while pressure blocked admission"
    );
    child.kill().unwrap();
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\"state\":\"queued\""), "{stderr}");
    assert!(
        stderr.contains("\"reason\":\"memory_pressure\""),
        "{stderr}"
    );
    assert!(stderr.contains("\"queue_position\":1"), "{stderr}");

    let output = fixture
        .runner_command()
        .env_remove("OULIPOLY_SESSION_ADMISSION_MIN_AVAILABLE_MEMORY_BYTES")
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("queued-model")
        .arg("successor admission probe")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(launched.exists(), "dead FIFO owner blocked its successor");

    let connection = Connection::open(&sidecar).unwrap();
    let states = connection
        .prepare("SELECT state FROM session_admission_queue ORDER BY queue_sequence")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(states, ["cancelled", "settled"]);
}

#[test]
fn invalid_admission_config_does_not_publish_a_fifo_owner() {
    let fixture = Fixture::new();
    let app_config = fixture.config_home.join("oulipoly-agent-runner");
    let launched = fixture.directory.path().join("provider-launched");
    let provider = fixture.directory.path().join("valid-successor-provider.sh");
    write_executable(
        &provider,
        &format!(
            "printf launched > {}\n",
            toml_string(&launched.display().to_string())
        ),
    );
    fs::write(
        fixture.models_dir.join("admission-model.toml"),
        "[[providers]]\nname = \"admission-provider\"\nargs = []\n",
    )
    .unwrap();
    fs::write(
        app_config.join("providers.toml"),
        format!(
            "[admission-provider]\ncommand = {}\nargs = []\nprompt_mode = \"arg\"\n",
            toml_string(&provider.display().to_string())
        ),
    )
    .unwrap();

    let invalid = fixture
        .runner_command()
        .env(
            "OULIPOLY_SESSION_ADMISSION_MIN_AVAILABLE_MEMORY_BYTES",
            "not-a-byte-count",
        )
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("admission-model")
        .arg("invalid admission config")
        .output()
        .unwrap();

    assert!(!invalid.status.success(), "{invalid:?}");
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains(
            "OULIPOLY_SESSION_ADMISSION_MIN_AVAILABLE_MEMORY_BYTES must be a positive byte count"
        ),
        "{invalid:?}"
    );
    let sidecar = fixture.sidecar();
    let connection = Connection::open(&sidecar).unwrap();
    let queue_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'session_admission_queue'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let queue_rows = if queue_tables == 0 {
        0
    } else {
        connection
            .query_row("SELECT COUNT(*) FROM session_admission_queue", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };
    assert_eq!(queue_rows, 0);

    let successor = fixture
        .runner_command()
        .env_remove("OULIPOLY_SESSION_ADMISSION_MIN_AVAILABLE_MEMORY_BYTES")
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("admission-model")
        .arg("valid successor")
        .output()
        .unwrap();
    assert!(successor.status.success(), "{successor:?}");
    assert!(launched.exists(), "valid successor did not reach provider");
}

#[test]
fn nested_and_parallel_launches_have_no_turn_budget_and_unrelated_launches_progress() {
    let fixture = Fixture::new();
    let app_config = fixture.config_home.join("oulipoly-agent-runner");
    let provider = fixture.directory.path().join("nested-provider.sh");
    let child_started = fixture.directory.path().join("child-started");
    let continue_child = fixture.directory.path().join("continue-child");
    let deepest_started = fixture.directory.path().join("deepest-started");
    let release_deepest = fixture.directory.path().join("release-deepest");
    let unrelated_started = fixture.directory.path().join("unrelated-started");
    let release_unrelated = fixture.directory.path().join("release-unrelated");
    let fanout_started = fixture.directory.path().join("fanout-started");
    let release_fanout = fixture.directory.path().join("release-fanout");
    let fanout_unrelated_started = fixture.directory.path().join("fanout-unrelated-started");
    let release_fanout_unrelated = fixture.directory.path().join("release-fanout-unrelated");
    fs::create_dir(&fanout_started).unwrap();
    write_executable(
        &provider,
        r#"run_nested() {
  "$AGE309_RUNNER" --models-dir "$AGE309_MODELS_DIR" --model nested-model "$1" &
  nested_pid=$!
  trap 'kill "$nested_pid" 2>/dev/null || true' EXIT TERM INT
  wait "$nested_pid"
  nested_status=$?
  trap - EXIT TERM INT
  return "$nested_status"
}

prompt="${!#}"
case "$prompt" in
  root)
    run_nested level-1
    ;;
  level-*)
    level="${prompt#level-}"
    if [[ "$level" == "1" ]]; then
      printf started > "$AGE309_CHILD_STARTED"
      while [[ ! -e "$AGE309_CONTINUE_CHILD" ]]; do sleep 0.02; done
    fi
    if (( level < 6 )); then
      run_nested "level-$((level + 1))"
    else
      printf started > "$AGE309_DEEPEST_STARTED"
      while [[ ! -e "$AGE309_RELEASE_DEEPEST" ]]; do sleep 0.02; done
    fi
    ;;
  unrelated)
    printf started > "$AGE309_UNRELATED_STARTED"
    while [[ ! -e "$AGE309_RELEASE_UNRELATED" ]]; do sleep 0.02; done
    ;;
  fanout-root)
    fanout_pids=()
    trap 'for pid in "${fanout_pids[@]}"; do kill "$pid" 2>/dev/null || true; done' EXIT TERM INT
    for index in 1 2 3 4 5 6; do
      "$AGE309_RUNNER" --models-dir "$AGE309_MODELS_DIR" --model nested-model "fanout-$index" &
      fanout_pids+=("$!")
    done
    for pid in "${fanout_pids[@]}"; do wait "$pid"; done
    trap - EXIT TERM INT
    ;;
  fanout-[1-6])
    printf started > "$AGE309_FANOUT_STARTED/$prompt"
    while [[ ! -e "$AGE309_RELEASE_FANOUT" ]]; do sleep 0.02; done
    ;;
  fanout-unrelated)
    printf started > "$AGE309_FANOUT_UNRELATED_STARTED"
    while [[ ! -e "$AGE309_RELEASE_FANOUT_UNRELATED" ]]; do sleep 0.02; done
    ;;
  *)
    printf 'unexpected prompt: %s\n' "$prompt" >&2
    exit 2
    ;;
esac
"#,
    );
    fs::write(
        fixture.models_dir.join("nested-model.toml"),
        "[[providers]]\nname = \"nested-provider\"\nargs = []\n",
    )
    .unwrap();
    fs::write(
        app_config.join("providers.toml"),
        format!(
            "[nested-provider]\ncommand = {}\nargs = []\nprompt_mode = \"arg\"\n",
            toml_string(&provider.display().to_string())
        ),
    )
    .unwrap();

    let configure = |command: &mut Command| {
        command
            .env("AGE309_RUNNER", env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
            .env("AGE309_MODELS_DIR", &fixture.models_dir)
            .env("AGE309_CHILD_STARTED", &child_started)
            .env("AGE309_CONTINUE_CHILD", &continue_child)
            .env("AGE309_DEEPEST_STARTED", &deepest_started)
            .env("AGE309_RELEASE_DEEPEST", &release_deepest)
            .env("AGE309_UNRELATED_STARTED", &unrelated_started)
            .env("AGE309_RELEASE_UNRELATED", &release_unrelated)
            .env("AGE309_FANOUT_STARTED", &fanout_started)
            .env("AGE309_RELEASE_FANOUT", &release_fanout)
            .env("AGE309_FANOUT_UNRELATED_STARTED", &fanout_unrelated_started)
            .env("AGE309_RELEASE_FANOUT_UNRELATED", &release_fanout_unrelated)
            .arg("--models-dir")
            .arg(&fixture.models_dir)
            .arg("--model")
            .arg("nested-model")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    };

    let mut root_command = fixture.runner_command();
    configure(&mut root_command);
    let mut root = root_command.arg("root").spawn().unwrap();
    if !wait_for_path_until(&child_started, Duration::from_secs(10)) {
        let _ = root.kill();
        let output = root.wait_with_output().unwrap();
        panic!("nested child did not enter its provider turn: {output:?}");
    }

    let mut unrelated_command = fixture.runner_command();
    configure(&mut unrelated_command);
    let mut unrelated = unrelated_command.arg("unrelated").spawn().unwrap();
    if !wait_for_path_until(&unrelated_started, Duration::from_secs(5)) {
        let _ = unrelated.kill();
        let _ = root.kill();
        let unrelated_output = unrelated.wait_with_output().unwrap();
        let root_output = root.wait_with_output().unwrap();
        panic!(
            "nested root and child removed unrelated launch service\nunrelated: {unrelated_output:?}\nroot: {root_output:?}"
        );
    }

    fs::write(&release_unrelated, "release").unwrap();
    let unrelated_output = wait_for_output(unrelated, "unrelated launch did not settle");
    assert!(unrelated_output.status.success(), "{unrelated_output:?}");

    fs::write(&continue_child, "continue").unwrap();
    if !wait_for_path_until(&deepest_started, Duration::from_secs(10)) {
        let _ = root.kill();
        let output = root.wait_with_output().unwrap();
        panic!("nested launch depth was treated as a provider-turn budget: {output:?}");
    }
    let nested_active_turns = active_turns(&fixture.sidecar());
    fs::write(&release_deepest, "release").unwrap();
    let root_output = wait_for_output(root, "nested dependency chain did not settle");
    assert!(root_output.status.success(), "{root_output:?}");
    assert_eq!(nested_active_turns, 7);

    let mut fanout_command = fixture.runner_command();
    configure(&mut fanout_command);
    let mut fanout = fanout_command.arg("fanout-root").spawn().unwrap();
    let fanout_markers = (1..=6)
        .map(|index| fanout_started.join(format!("fanout-{index}")))
        .collect::<Vec<_>>();
    if !wait_for_paths_until(&fanout_markers, Duration::from_secs(10)) {
        let _ = fanout.kill();
        let output = fanout.wait_with_output().unwrap();
        panic!("same-parent parallel launches encountered a turn budget: {output:?}");
    }

    let mut fanout_unrelated_command = fixture.runner_command();
    configure(&mut fanout_unrelated_command);
    let mut fanout_unrelated = fanout_unrelated_command
        .arg("fanout-unrelated")
        .spawn()
        .unwrap();
    if !wait_for_path_until(&fanout_unrelated_started, Duration::from_secs(5)) {
        let _ = fanout_unrelated.kill();
        let _ = fanout.kill();
        let unrelated_output = fanout_unrelated.wait_with_output().unwrap();
        let fanout_output = fanout.wait_with_output().unwrap();
        panic!(
            "parallel child launches removed unrelated launch service\nunrelated: {unrelated_output:?}\nfanout: {fanout_output:?}"
        );
    }
    let parallel_active_turns = active_turns(&fixture.sidecar());

    fs::write(&release_fanout_unrelated, "release").unwrap();
    let unrelated_output = wait_for_output(
        fanout_unrelated,
        "unrelated launch beside parallel children did not settle",
    );
    assert!(unrelated_output.status.success(), "{unrelated_output:?}");
    fs::write(&release_fanout, "release").unwrap();
    let fanout_output = wait_for_output(fanout, "parallel child launches did not settle");
    assert!(fanout_output.status.success(), "{fanout_output:?}");
    assert_eq!(parallel_active_turns, 8);
}

fn active_turns(sidecar: &Path) -> i64 {
    let connection = Connection::open(sidecar).unwrap();
    connection
        .query_row(
            "SELECT COUNT(*) FROM session_admission_queue
             WHERE state IN ('admitted', 'launching')",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn wait_for_path_until(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn wait_for_paths_until(paths: &[std::path::PathBuf], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if paths.iter().all(|path| path.exists()) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn wait_for_output(mut child: std::process::Child, failure: &str) -> std::process::Output {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    panic!("{failure}: {output:?}");
}

fn write_executable(path: &Path, body: &str) {
    fs::write(
        path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}"),
    )
    .unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}
