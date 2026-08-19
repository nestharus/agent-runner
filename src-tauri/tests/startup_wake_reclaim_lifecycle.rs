#![cfg(target_os = "linux")]

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{AgentBashCompleteEnqueue, MailboxDb};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const CONCURRENCY: usize = 8;

#[test]
fn candidate_bearing_launches_single_flight_and_leave_no_snapshot_helpers() {
    let directory = tempfile::tempdir().unwrap();
    let config_home = directory.path().join("config");
    let data_home = directory.path().join("data");
    let home = directory.path().join("home");
    let snapshot_temp = directory.path().join("snapshot-temp");
    let app_config = config_home.join("oulipoly-agent-runner");
    let models = app_config.join("models");
    std::fs::create_dir_all(&models).unwrap();
    std::fs::create_dir_all(&data_home).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&snapshot_temp).unwrap();
    let starts = directory.path().join("provider-starts.log");
    let provider = directory.path().join("fixture-provider.sh");
    std::fs::write(
        &provider,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"${{1:-missing}}\" >> {}\nprintf 'fixture-ok\\n'\n",
            starts.display()
        ),
    )
    .unwrap();
    make_executable(&provider);
    std::fs::write(
        models.join("fixture.toml"),
        "[[providers]]\nname = \"fixture-provider\"\n",
    )
    .unwrap();
    std::fs::write(
        app_config.join("providers.toml"),
        format!(
            "[fixture-provider]\ncommand = \"{}\"\nargs = []\nprompt_mode = \"arg\"\n",
            provider.display()
        ),
    )
    .unwrap();

    let runner = Path::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    let warmup = runner_command(
        runner,
        &models,
        &config_home,
        &data_home,
        &home,
        &snapshot_temp,
        "warmup",
    )
    .output()
    .unwrap();
    assert!(
        warmup.status.success(),
        "{}",
        String::from_utf8_lossy(&warmup.stderr)
    );

    let data_root = data_home.join("oulipoly-agent-runner");
    let state_path = data_root.join("state.db");
    let mailbox_path = data_root.join("pid-identity.db");
    let state = StateDb::open(&state_path).unwrap();
    drop(state);
    let connection = rusqlite::Connection::open(&state_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE wake_reclaim_snapshot_fixture (payload BLOB NOT NULL);
             INSERT INTO wake_reclaim_snapshot_fixture VALUES (zeroblob(134217728));",
        )
        .unwrap();
    drop(connection);
    seed_pending_wake_candidate(directory.path(), &mailbox_path);
    std::fs::write(&starts, []).unwrap();
    let baseline_temp_entries = directory_entries(&snapshot_temp);

    let stop_sampling = Arc::new(AtomicBool::new(false));
    let helper_peak = Arc::new(AtomicUsize::new(0));
    let sampler_stop = Arc::clone(&stop_sampling);
    let sampler_peak = Arc::clone(&helper_peak);
    let sampled_root = directory.path().to_path_buf();
    let sampler = std::thread::spawn(move || {
        while !sampler_stop.load(Ordering::SeqCst) {
            sampler_peak.fetch_max(snapshot_helper_count(&sampled_root), Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(1));
        }
    });

    let barrier = Arc::new(Barrier::new(CONCURRENCY));
    let launches = (0..CONCURRENCY)
        .map(|index| {
            let barrier = Arc::clone(&barrier);
            let runner = runner.to_path_buf();
            let models = models.clone();
            let config_home = config_home.clone();
            let data_home = data_home.clone();
            let home = home.clone();
            let snapshot_temp = snapshot_temp.clone();
            std::thread::spawn(move || {
                barrier.wait();
                runner_command(
                    &runner,
                    &models,
                    &config_home,
                    &data_home,
                    &home,
                    &snapshot_temp,
                    &format!("launch-{index}"),
                )
                .output()
                .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let outputs = launches
        .into_iter()
        .map(|launch| launch.join().unwrap())
        .collect::<Vec<_>>();
    stop_sampling.store(true, Ordering::SeqCst);
    sampler.join().unwrap();

    for output in outputs {
        assert!(
            output.status.success(),
            "candidate-bearing launch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        assert!(!stderr.contains("database is locked"));
        assert!(!stderr.contains("database is busy"));
    }
    assert_eq!(
        std::fs::read_to_string(&starts).unwrap().lines().count(),
        CONCURRENCY
    );
    let helper_peak = helper_peak.load(Ordering::SeqCst);
    assert_eq!(
        helper_peak, 1,
        "expected exactly one admitted snapshot helper"
    );
    wait_until(Duration::from_secs(5), || {
        snapshot_helper_count(directory.path()) == 0
    });
    assert_eq!(snapshot_helper_count(directory.path()), 0);
    let snapshot_temp_entries = directory_entries(&snapshot_temp);
    assert_eq!(
        snapshot_temp_entries, baseline_temp_entries,
        "snapshot temp entries changed across settled launches"
    );
    assert_eq!(
        sqlite_count(
            &state_path,
            "SELECT COUNT(*) FROM invocations WHERE status = 'running'"
        ),
        0
    );
    assert_eq!(
        sqlite_count(
            &mailbox_path,
            "SELECT COUNT(*) FROM runtime_generation WHERE lifecycle_state != 'exited'",
        ),
        0
    );
}

fn runner_command(
    runner: &Path,
    models: &Path,
    config_home: &Path,
    data_home: &Path,
    home: &Path,
    snapshot_temp: &Path,
    prompt: &str,
) -> Command {
    let mut command = Command::new(runner);
    command
        .arg("--models-dir")
        .arg(models)
        .arg("--model")
        .arg("fixture")
        .arg(prompt)
        .env("XDG_CONFIG_HOME", config_home)
        .env("XDG_DATA_HOME", data_home)
        .env("HOME", home)
        .env("TMPDIR", snapshot_temp)
        .env_remove("OULIPOLY_DATA_DIR")
        .env_remove("OULIPOLY_PARENT_INVOCATION");
    command
}

fn seed_pending_wake_candidate(root: &Path, mailbox_path: &Path) {
    let payload_root = root.join("pending-payload");
    std::fs::create_dir(&payload_root).unwrap();
    let meta = payload_root.join("meta.json");
    let log = payload_root.join("log");
    let rc = payload_root.join("rc");
    std::fs::write(&meta, r#"{"caller_chain":[]}"#).unwrap();
    std::fs::write(&log, "pending\n").unwrap();
    std::fs::write(&rc, "0\n").unwrap();
    let mut mailbox = MailboxDb::open(mailbox_path).unwrap();
    mailbox
        .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id: "candidate-bearing-session",
            handle: "candidate-bearing-handle",
            payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
            owner_invocation_uuid: None,
            matched_os_pid: None,
            matched_os_boot_id: None,
            matched_os_pid_starttime_ticks: None,
            matched_chain_index: None,
            state_dir: payload_root.to_str().unwrap(),
            meta_path: meta.to_str().unwrap(),
            log_path: log.to_str().unwrap(),
            rc_path: rc.to_str().unwrap(),
            rc: 0,
        })
        .unwrap();
}

fn snapshot_helper_count(root: &Path) -> usize {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().parse::<u32>().is_ok())
        .filter_map(|entry| std::fs::read(entry.path().join("cmdline")).ok())
        .filter(|cmdline| {
            let command = String::from_utf8_lossy(cmdline);
            command.contains("__oulipoly-snapshot-helper")
                && command.contains(&root.to_string_lossy().to_string())
        })
        .count()
}

fn sqlite_count(path: &Path, query: &str) -> i64 {
    rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(query, [], |row| row.get(0))
        .unwrap()
}

fn directory_entries(path: &Path) -> Vec<PathBuf> {
    let mut entries = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}
