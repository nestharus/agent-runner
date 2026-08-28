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
