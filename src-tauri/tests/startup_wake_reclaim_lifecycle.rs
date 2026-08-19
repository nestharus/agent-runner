#![cfg(target_os = "linux")]

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, BindRuntimeGenerationRunning, CreateRuntimeGeneration, MailboxDb,
    RuntimeGenerationFence, RuntimeGenerationId, SessionMetadataUpsert,
};
use oulipoly_state::pid_identity::read_live_process_identity;
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
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$*\" >> {}\nprintf 'fixture-ok\\n'\n",
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
            "[fixture-provider]\ncommand = \"{}\"\nargs = []\nprompt_mode = \"arg\"\n\n[fixture-provider.resume]\nkind = \"flag\"\nflag = \"--resume\"\n",
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
    seed_recoverable_wake_candidate(directory.path(), &state_path, &mailbox_path, &models);
    assert_eq!(
        sqlite_count(
            &mailbox_path,
            "SELECT selected_auto_wake_max FROM session_runtime WHERE session_id = 'candidate-bearing-session'",
        ),
        1
    );
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
    wait_until(Duration::from_secs(10), || {
        std::fs::read_to_string(&starts).unwrap().lines().count() > CONCURRENCY
            && sqlite_count(
                &mailbox_path,
                "SELECT auto_wake_count FROM session_runtime WHERE session_id = 'candidate-bearing-session'",
            ) >= 1
            && sqlite_count(
                &mailbox_path,
                "SELECT COUNT(*) FROM runtime_generation WHERE lifecycle_state != 'exited'",
            ) == 0
            && sqlite_count(
                &state_path,
                "SELECT COUNT(*) FROM invocations WHERE status = 'running'",
            ) == 0
    });
    let starts_content = std::fs::read_to_string(&starts).unwrap();
    assert!(
        starts_content.lines().count() > CONCURRENCY,
        "the recoverable pending session was not automatically woken: {starts_content}"
    );
    assert_eq!(
        sqlite_count(
            &mailbox_path,
            "SELECT auto_wake_count FROM session_runtime WHERE session_id = 'candidate-bearing-session'",
        ),
        1,
        "wake retry cap was not preserved; provider starts: {starts_content}"
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
    assert_eq!(
        sqlite_count(
            &mailbox_path,
            "SELECT COUNT(*) FROM runtime_generation
             WHERE generation_uuid = '22222222-2222-4222-8222-222222222222'
               AND lifecycle_state = 'exited'
               AND terminal_reason = 'recovered_dead'",
        ),
        1,
        "the exact recorded-dead incumbent was not reconciled before candidate planning"
    );
}

#[test]
fn detached_bootstrap_handoff_completes_one_wake_without_an_owner_lease() {
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
    let starts = directory.path().join("handoff-provider-starts.log");
    let provider = directory.path().join("handoff-fixture-provider.sh");
    std::fs::write(
        &provider,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$*\" >> {}\nprintf 'fixture-ok\\n'\n",
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
            "[fixture-provider]\ncommand = \"{}\"\nargs = []\nprompt_mode = \"arg\"\n\n[fixture-provider.resume]\nkind = \"flag\"\nflag = \"--resume\"\n",
            provider.display()
        ),
    )
    .unwrap();

    let data_root = data_home.join("oulipoly-agent-runner");
    std::fs::create_dir_all(&data_root).unwrap();
    let state_path = data_root.join("state.db");
    let mailbox_path = data_root.join("pid-identity.db");
    drop(StateDb::open(&state_path).unwrap());
    seed_recoverable_wake_candidate(directory.path(), &state_path, &mailbox_path, &models);
    std::fs::write(&starts, []).unwrap();
    let owner_token = "wake-reclaim-bootstrap";
    let handoff_token = "bootstrap-handoff-token";
    let lease_path = data_root.join("pid-identity.db.wake-reclaim-owner.json");
    assert!(!lease_path.exists());

    let output = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("HOME", &home)
        .env("TMPDIR", &snapshot_temp)
        .env("OULIPOLY_WAKE_RECLAIM_HANDOFF_OWNER", owner_token)
        .env("OULIPOLY_WAKE_RECLAIM_HANDOFF_TOKEN", handoff_token)
        .env_remove("OULIPOLY_DATA_DIR")
        .env_remove("OULIPOLY_PARENT_INVOCATION")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "handoff helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    wait_until(Duration::from_secs(10), || {
        std::fs::read_to_string(&starts)
            .map(|content| !content.is_empty())
            .unwrap_or(false)
            && sqlite_count(
                &mailbox_path,
                "SELECT COUNT(*) FROM runtime_generation WHERE lifecycle_state != 'exited'",
            ) == 0
    });

    let starts_content = std::fs::read_to_string(&starts).unwrap();
    assert_eq!(
        starts_content
            .matches("--resume candidate-bearing-session")
            .count(),
        1,
        "unexpected provider starts with auto_wake_count={} selected_auto_wake_max={}: {starts_content}",
        sqlite_count(
            &mailbox_path,
            "SELECT auto_wake_count FROM session_runtime WHERE session_id = 'candidate-bearing-session'",
        ),
        sqlite_count(
            &mailbox_path,
            "SELECT selected_auto_wake_max FROM session_runtime WHERE session_id = 'candidate-bearing-session'",
        ),
    );
    assert_eq!(
        sqlite_count(
            &mailbox_path,
            "SELECT auto_wake_count FROM session_runtime WHERE session_id = 'candidate-bearing-session'",
        ),
        1
    );
    assert_eq!(
        sqlite_count(
            &mailbox_path,
            "SELECT COUNT(*) FROM runtime_generation
             WHERE generation_uuid = '22222222-2222-4222-8222-222222222222'
               AND terminal_reason = 'recovered_dead'",
        ),
        1
    );
    assert!(!lease_path.exists());
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

fn seed_recoverable_wake_candidate(
    root: &Path,
    state_path: &Path,
    mailbox_path: &Path,
    models: &Path,
) {
    let state = rusqlite::Connection::open(state_path).unwrap();
    state
        .execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES ('11111111-1111-4111-8111-111111111111', '2026-08-19T00:00:00Z', '2026-08-19T00:00:00Z', 'fixture')",
            [],
        )
        .unwrap();
    state
        .execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES ('11111111-1111-4111-8111-111111111111', 'fixture-provider', 'candidate-bearing-session', '2026-08-19T00:00:00Z', 'initial')",
            [],
        )
        .unwrap();
    drop(state);

    let payload_root = root.join("pending-payload");
    std::fs::create_dir(&payload_root).unwrap();
    let meta = payload_root.join("meta.json");
    let log = payload_root.join("log");
    let rc = payload_root.join("rc");
    std::fs::write(&meta, r#"{"caller_chain":[]}"#).unwrap();
    std::fs::write(&log, "pending\n").unwrap();
    std::fs::write(&rc, "0\n").unwrap();
    let mut mailbox = MailboxDb::open(mailbox_path).unwrap();
    let models = models.to_str().unwrap();
    mailbox
        .wake_sessions()
        .upsert_session_metadata(SessionMetadataUpsert {
            session_id: "candidate-bearing-session",
            mode: "headless",
            invocation_uuid: None,
            provider_name: Some("fixture-provider"),
            model_name: Some("fixture"),
            models_dir: Some(models),
            effective_cwd: None,
            selected_auto_wake_max: Some(1),
        })
        .unwrap();
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

    let mut incumbent = Command::new("sh")
        .arg("-c")
        .arg("sleep 60")
        .spawn()
        .unwrap();
    let identity = read_live_process_identity(i64::from(incumbent.id()))
        .unwrap()
        .unwrap();
    let generation_id = RuntimeGenerationId::parse("22222222-2222-4222-8222-222222222222").unwrap();
    mailbox
        .runtime_lifecycle()
        .create_runtime_generation(CreateRuntimeGeneration {
            generation_id: &generation_id,
            spawn_invocation_uuid: "33333333-3333-4333-8333-333333333333",
            session_id: Some("candidate-bearing-session"),
            runtime_mode: "headless",
            provider_name: "fixture-provider",
            model_name: Some("fixture"),
            pty_control_path: None,
            models_dir: Some(models),
            effective_cwd: None,
        })
        .unwrap();
    mailbox
        .runtime_lifecycle()
        .bind_runtime_generation_running(BindRuntimeGenerationRunning {
            fence: RuntimeGenerationFence {
                generation_id: &generation_id,
                spawn_invocation_uuid: "33333333-3333-4333-8333-333333333333",
            },
            spawned_os_pid: identity.os_pid,
            exact_process_identity: Some(&identity),
            os_pgid: None,
        })
        .unwrap();
    incumbent.kill().unwrap();
    incumbent.wait().unwrap();
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
