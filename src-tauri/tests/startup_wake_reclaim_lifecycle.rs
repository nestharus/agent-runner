#![cfg(target_os = "linux")]

mod provider_authority_fixture;

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{
    AdvanceRuntimeGenerationDrain, AgentBashCompleteEnqueue, BindRuntimeGenerationRunning,
    CreateRuntimeGeneration, DrainRequestId, MailboxDb, RequestRuntimeGenerationDrain,
    RuntimeGenerationFence, RuntimeGenerationId, SessionMetadataUpsert,
};
use oulipoly_state::pid_identity::read_live_process_identity;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

const CONCURRENCY: usize = 8;

#[test]
fn candidate_bearing_launches_bound_snapshot_helpers_and_leave_none() {
    // Keep the historical ID and native custody obligations. A legitimate native
    // claim can preempt snapshot selection; occurrence is independently tested below.
    concurrent_startup(true);
}

#[test]
fn incomplete_parent_concurrent_launches_exercise_snapshot_admission_and_cleanup() {
    // Existing incomplete-history refusal is the independent admission oracle:
    // the native driver stays enabled but cannot consume this pending candidate.
    concurrent_startup(false);
}

fn concurrent_startup(complete_parent: bool) {
    let _guard = integration_test_guard();
    let directory = FailureEvidenceDirectory(Some(tempfile::tempdir().unwrap()));
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
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$*\" >> {}\nif [[ \"$*\" == *'establish recovery recipient'* ]]; then\n  printf '%s\\n' '{{\"type\":\"session\",\"session_id\":\"candidate-bearing-session\"}}'\nfi\nif [[ \"$*\" == *'--resume candidate-bearing-session'* ]]; then\n  printf 'fixture-resume-accepted\\n'\nfi\nprintf 'fixture-ok\\n'\n",
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
        provider_authority_fixture::with_explicit_provider_authority_for_prompt_acceptance(&format!(
            "[fixture-provider]\ncommand = \"{}\"\nargs = []\nprompt_mode = \"arg\"\n\n[fixture-provider.resume]\nkind = \"flag\"\nflag = \"--resume\"\n\n[fixture-provider.session_capture]\nkind = \"stdout_json_event\"\njson_args = [\"--fixture-json\"]\nevent_type = \"session\"\nevent_id_path = \"session_id\"\n\n[fixture-provider.resume_acceptance]\naccepted_output_patterns = [\"fixture-resume-accepted\"]\n",
            provider.display()
        ), &["fixture-provider"]),
    )
    .unwrap();

    oulipoly_config::providers::ProvidersConfig::load(&app_config.join("providers.toml"))
        .expect("valid startup fixture provider contract");
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

    oulipoly_config::providers::ProvidersConfig::load(&app_config.join("providers.toml"))
        .expect("valid startup fixture provider contract");
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
    seed_recoverable_wake_candidate(
        directory.path(),
        &state_path,
        &mailbox_path,
        &models,
        complete_parent,
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
                let started = Instant::now();
                let output = runner_command(
                    &runner,
                    &models,
                    &config_home,
                    &data_home,
                    &home,
                    &snapshot_temp,
                    &format!("launch-{index}"),
                )
                .output()
                .unwrap();
                (output, started.elapsed())
            })
        })
        .collect::<Vec<_>>();
    let outputs = launches
        .into_iter()
        .map(|launch| launch.join().unwrap())
        .collect::<Vec<_>>();
    stop_sampling.store(true, Ordering::SeqCst);
    sampler.join().unwrap();

    for (index, (output, elapsed)) in outputs.iter().enumerate() {
        println!(
            "foreground index={index} elapsed={elapsed:?} status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for (output, elapsed) in outputs {
        assert!(
            output.status.success(),
            "candidate-bearing launch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "foreground launch waited for best-effort wake reclamation: {elapsed:?}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        assert!(!stderr.contains("database is locked"));
        assert!(!stderr.contains("database is busy"));
    }
    if complete_parent {
        assert_one_concurrent_wake(&starts, &state_path, &mailbox_path);
    } else {
        assert_incomplete_concurrent_parent(&starts, &state_path, &mailbox_path);
    }
    let helper_peak = helper_peak.load(Ordering::SeqCst);
    println!("complete_parent={complete_parent} snapshot_helper_peak={helper_peak}");
    assert!(
        helper_peak <= 2,
        "expected at most the expiring owner and its fenced successor, observed {helper_peak}"
    );
    if !complete_parent {
        assert!(
            helper_peak >= 1,
            "snapshot concurrency control must not be vacuous"
        );
    }
    wait_until(Duration::from_secs(5), || {
        snapshot_helper_count(directory.path()) == 0
            && directory_entries(&snapshot_temp) == baseline_temp_entries
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
               AND terminal_reason = 'recovered_dead'
               AND drain_request_uuid = '44444444-4444-4444-8444-444444444444'
               AND drain_requested_by_invocation_uuid = '33333333-3333-4333-8333-333333333333'
               AND draining_at IS NOT NULL",
        ),
        1,
        "the exact recorded-dead incumbent was not reconciled before candidate planning"
    );
    if complete_parent {
        assert_settled_wake(&state_path, &mailbox_path);
        println!("concurrent native original custody and end-state assertions reached");
    }
    println!("concurrent snapshot cleanup and exact dead-generation assertions reached");
}

fn assert_one_concurrent_wake(starts: &Path, state_path: &Path, mailbox_path: &Path) {
    wait_until(Duration::from_secs(10), || {
        std::fs::read_to_string(starts).unwrap().lines().count() > CONCURRENCY
            && sqlite_count(
                mailbox_path,
                "SELECT auto_wake_count FROM session_runtime WHERE session_id = 'candidate-bearing-session'",
            ) >= 1
            && sqlite_count(
                mailbox_path,
                "SELECT COUNT(*) FROM runtime_generation WHERE lifecycle_state != 'exited'",
            ) == 0
            && sqlite_count(
                state_path,
                "SELECT COUNT(*) FROM invocations WHERE status = 'running'",
            ) == 0
    });
    let starts_content = std::fs::read_to_string(starts).unwrap();
    assert!(
        starts_content.lines().count() > CONCURRENCY,
        "the recoverable pending session was not automatically woken: {starts_content}"
    );
    assert_eq!(
        sqlite_count(
            mailbox_path,
            "SELECT auto_wake_count FROM session_runtime WHERE session_id = 'candidate-bearing-session'",
        ),
        1,
        "wake claim single-flight was not preserved; provider starts: {starts_content}"
    );
    assert_eq!(
        starts_content
            .matches("handle: candidate-bearing-handle")
            .count(),
        1
    );
}

fn assert_incomplete_concurrent_parent(starts: &Path, state_path: &Path, mailbox_path: &Path) {
    wait_until(Duration::from_secs(10), || {
        sqlite_count(
            state_path,
            "SELECT COUNT(*) FROM invocations WHERE status = 'running'",
        ) == 0
            && sqlite_count(
                mailbox_path,
                "SELECT COUNT(*) FROM runtime_generation WHERE lifecycle_state != 'exited'",
            ) == 0
    });
    assert_eq!(
        std::fs::read_to_string(starts).unwrap().lines().count(),
        CONCURRENCY
    );
    let mailbox = MailboxDb::open(mailbox_path).unwrap();
    assert!(
        mailbox.completion_continuation_owner().unwrap().is_some(),
        "native owner remains enabled"
    );
    let runtime = mailbox
        .wake_session_reader()
        .session_metadata("candidate-bearing-session")
        .unwrap()
        .unwrap();
    assert_eq!(runtime.auto_wake_count, 0);
    assert!(runtime.invocation_uuid.as_deref().is_none_or(|id| {
        StateDb::open(state_path)
            .unwrap()
            .get_invocation_by_uuid(id)
            .unwrap()
            .is_none()
    }));
    assert!(
        mailbox
            .wake_session_reader()
            .wake_claim("candidate-bearing-session")
            .unwrap()
            .is_none()
    );
    let pending = mailbox.list_pending("candidate-bearing-session").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].handle, "candidate-bearing-handle");
    assert_eq!(
        sqlite_count(
            mailbox_path,
            "SELECT COUNT(*) FROM completion_continuation_attempt WHERE session_id='candidate-bearing-session'"
        ),
        0
    );
    println!("incomplete parent: actual owner, pending retained, no claim/activation/wake");
}

#[test]
fn detached_bootstrap_handoff_completes_one_wake_without_an_owner_lease() {
    bootstrap_handoff(true);
}

#[test]
fn incomplete_parent_handoff_does_not_admit_a_native_wake() {
    bootstrap_handoff(false);
}

fn bootstrap_handoff(complete_parent: bool) {
    let _guard = integration_test_guard();
    let directory = FailureEvidenceDirectory(Some(tempfile::tempdir().unwrap()));
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
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$*\" >> {}\nif [[ \"$*\" == *'establish recovery recipient'* ]]; then\n  printf '%s\\n' '{{\"type\":\"session\",\"session_id\":\"candidate-bearing-session\"}}'\nfi\nif [[ \"$*\" == *'--resume candidate-bearing-session'* ]]; then\n  printf 'fixture-resume-accepted\\n'\nfi\nprintf 'fixture-ok\\n'\n",
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
        provider_authority_fixture::with_explicit_provider_authority_for_prompt_acceptance(&format!(
            "[fixture-provider]\ncommand = \"{}\"\nargs = []\nprompt_mode = \"arg\"\n\n[fixture-provider.resume]\nkind = \"flag\"\nflag = \"--resume\"\n\n[fixture-provider.session_capture]\nkind = \"stdout_json_event\"\njson_args = [\"--fixture-json\"]\nevent_type = \"session\"\nevent_id_path = \"session_id\"\n\n[fixture-provider.resume_acceptance]\naccepted_output_patterns = [\"fixture-resume-accepted\"]\n",
            provider.display()
        ), &["fixture-provider"]),
    )
    .unwrap();

    oulipoly_config::providers::ProvidersConfig::load(&app_config.join("providers.toml"))
        .expect("valid startup fixture provider contract");
    let data_root = data_home.join("oulipoly-agent-runner");
    std::fs::create_dir_all(&data_root).unwrap();
    let state_path = data_root.join("state.db");
    let mailbox_path = data_root.join("pid-identity.db");
    drop(StateDb::open(&state_path).unwrap());
    seed_recoverable_wake_candidate(
        directory.path(),
        &state_path,
        &mailbox_path,
        &models,
        complete_parent,
    );
    std::fs::write(&starts, []).unwrap();
    let owner_token = "wake-reclaim-bootstrap";
    let handoff_token = "wake-reclaim-bootstrap";
    let lease_path = data_root.join("pid-identity.db.wake-reclaim-owner.json");
    assert!(!lease_path.exists());

    let handoff_started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("HOME", &home)
        .env("TMPDIR", &snapshot_temp)
        .env("OULIPOLY_WAKE_RECLAIM_HANDOFF_OWNER", owner_token)
        .env("OULIPOLY_WAKE_RECLAIM_HANDOFF_TOKEN", handoff_token)
        .env("OULIPOLY_DATA_DIR", &data_root)
        .env_remove("OULIPOLY_PARENT_INVOCATION")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "handoff helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!(
        "handoff elapsed={:?} status={} stdout={} stderr={}",
        handoff_started.elapsed(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        handoff_started.elapsed() < Duration::from_secs(5),
        "handoff foreground exceeded five seconds"
    );
    if !complete_parent {
        // Bootstrap is real; neither an owner row nor an invocation UUID is
        // supplied by the test. Incomplete history must remain inadmissible.
        wait_until(Duration::from_secs(5), || {
            MailboxDb::open(&mailbox_path)
                .unwrap()
                .completion_continuation_owner()
                .unwrap()
                .is_some()
        });
        assert!(
            MailboxDb::open(&mailbox_path)
                .unwrap()
                .completion_continuation_owner()
                .unwrap()
                .is_some()
        );
        std::thread::sleep(Duration::from_secs(1));
        assert!(std::fs::read_to_string(&starts).unwrap().is_empty());
        assert_eq!(
            sqlite_count(&state_path, "SELECT COUNT(*) FROM invocations"),
            0
        );
        assert_eq!(
            sqlite_count(
                &mailbox_path,
                "SELECT auto_wake_count FROM session_runtime WHERE session_id='candidate-bearing-session'"
            ),
            0
        );
        assert_eq!(
            sqlite_count(
                &mailbox_path,
                "SELECT COUNT(*) FROM completion_continuation_attempt WHERE session_id='candidate-bearing-session'"
            ),
            0
        );
        let selected = MailboxDb::open(&mailbox_path)
            .unwrap()
            .wake_session_reader()
            .session_metadata("candidate-bearing-session")
            .unwrap()
            .unwrap()
            .invocation_uuid;
        assert!(selected.as_deref().is_none_or(|id| {
            StateDb::open(&state_path)
                .unwrap()
                .get_invocation_by_uuid(id)
                .unwrap()
                .is_none()
        }));
        println!(
            "incomplete parent: real owner, no parent invocation, no provider start/activation/count during bounded observation"
        );
        return;
    }
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
            .matches("handle: candidate-bearing-handle")
            .count(),
        1,
        "unexpected provider starts with auto_wake_count={}: {starts_content}",
        sqlite_count(
            &mailbox_path,
            "SELECT auto_wake_count FROM session_runtime WHERE session_id = 'candidate-bearing-session'",
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
    assert_settled_wake(&state_path, &mailbox_path);
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
        .env("OULIPOLY_DATA_DIR", data_home.join("oulipoly-agent-runner"))
        .env_remove("OULIPOLY_PARENT_INVOCATION");
    command
}

fn integration_test_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn seed_recoverable_wake_candidate(
    root: &Path,
    state_path: &Path,
    mailbox_path: &Path,
    models: &Path,
    complete_parent: bool,
) {
    let parent =
        complete_parent.then(|| establish_recovery_parent(root, state_path, mailbox_path, models));
    if !complete_parent {
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
        provider_authority_fixture::bind_session_authority_with_cwd(
            &state,
            "fixture-provider",
            "candidate-bearing-session",
            root,
        );
        drop(state);
    }

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
            invocation_uuid: parent.as_deref(),
            provider_name: Some("fixture-provider"),
            model_name: Some("fixture"),
            models_dir: Some(models),
            effective_cwd: None,
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
            exact_process_identity: &identity,
            os_pgid: None,
        })
        .unwrap();
    let drain_request_id = DrainRequestId::parse("44444444-4444-4444-8444-444444444444").unwrap();
    let fence = RuntimeGenerationFence {
        generation_id: &generation_id,
        spawn_invocation_uuid: "33333333-3333-4333-8333-333333333333",
    };
    mailbox
        .runtime_lifecycle()
        .request_runtime_generation_drain(RequestRuntimeGenerationDrain {
            fence,
            drain_request_id: &drain_request_id,
            requested_by_invocation_uuid: "33333333-3333-4333-8333-333333333333",
        })
        .unwrap();
    mailbox
        .runtime_lifecycle()
        .advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
            fence,
            drain_request_id: &drain_request_id,
        })
        .unwrap();
    incumbent.kill().unwrap();
    incumbent.wait().unwrap();
    // Creation records this still-live test process. Recovery requires both
    // creator and child to have ceased matching their recorded identities.
    let changed = rusqlite::Connection::open(mailbox.path())
        .unwrap()
        .execute(
            "UPDATE runtime_generation SET creator_identity_os_boot_id = 'fixture-previous-boot'
             WHERE generation_uuid = ?1",
            [generation_id.to_string()],
        )
        .unwrap();
    assert_eq!(changed, 1);
    if let Some(parent) = parent.as_deref() {
        // The historical incumbent's compatibility projection selects its
        // synthetic spawn UUID. Retain the separately proven real recipient,
        // just as the recovery-parent fixtures retain their earlier metadata.
        // This is history setup while no owner exists, not a native admission,
        // activation, claim, binding or custody grant to this test process.
        assert!(mailbox.completion_continuation_owner().unwrap().is_none());
        mailbox
            .wake_sessions()
            .upsert_session_metadata(SessionMetadataUpsert {
                session_id: "candidate-bearing-session",
                mode: "headless",
                invocation_uuid: Some(parent),
                provider_name: Some("fixture-provider"),
                model_name: Some("fixture"),
                models_dir: Some(models),
                effective_cwd: root.to_str(),
            })
            .unwrap();
    }
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

// The parent is published by the real Runner/provider binding path, following
// the retry-parent fixture precedent, before historical dead-incumbent inputs.
fn establish_recovery_parent(
    root: &Path,
    state_path: &Path,
    mailbox_path: &Path,
    models: &Path,
) -> String {
    let output = runner_command(
        Path::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner")),
        models,
        &root.join("config"),
        &root.join("data"),
        &root.join("home"),
        &root.join("snapshot-temp"),
        "establish recovery recipient",
    )
    .current_dir(root)
    .output()
    .unwrap();
    println!(
        "genuine parent output status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "genuine parent launch: {output:?}");
    let mailbox = MailboxDb::open(mailbox_path).unwrap();
    let runtime = mailbox
        .wake_session_reader()
        .session_metadata("candidate-bearing-session")
        .unwrap()
        .expect("Runner-published session");
    let id = runtime.invocation_uuid.expect("Runner-published parent");
    let state = StateDb::open(state_path).unwrap();
    let parent = state.get_invocation_by_uuid(&id).unwrap().unwrap();
    assert_eq!(
        parent.provider_session_id.as_deref(),
        Some("candidate-bearing-session")
    );
    assert!(parent.finished_at.is_some());
    let outcome: (String, bool, i64) = state
        .connection()
        .query_row(
            "SELECT status, success, exit_code FROM invocations WHERE invocation_uuid=?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(outcome, ("succeeded".to_string(), true, 0));
    wait_until(Duration::from_secs(5), || {
        MailboxDb::open(mailbox_path)
            .unwrap()
            .completion_continuation_owner()
            .unwrap()
            .is_none()
    });
    assert!(
        MailboxDb::open(mailbox_path)
            .unwrap()
            .completion_continuation_owner()
            .unwrap()
            .is_none(),
        "founding owner must retire before startup recovery"
    );
    println!(
        "genuine recovery parent={id} session=candidate-bearing-session finished={:?}",
        parent.finished_at
    );
    id
}

fn assert_settled_wake(state_path: &Path, mailbox_path: &Path) {
    // Source-custody and foreground success do not substitute for this original
    // native activation's independently collected ECHILD receipt and result.
    let settled = || {
        sqlite_count(
            mailbox_path,
            "SELECT COUNT(*) FROM completion_continuation_attempt WHERE session_id='candidate-bearing-session' AND phase='drained' AND integrated=1 AND drain_receipt LIKE '%ECHILD%'",
        ) == 1
    };
    wait_until(Duration::from_secs(5), settled);
    assert!(settled(), "original native wake custody did not drain");
    let mailbox = rusqlite::Connection::open(mailbox_path).unwrap();
    let row: (String, String, String, String, String) = mailbox.query_row(
        "SELECT runtime_generation_uuid,spawn_invocation_uuid,custodian_identity,launcher_identity,drain_receipt FROM completion_continuation_attempt WHERE session_id='candidate-bearing-session'",
        [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
    for identity in [&row.2, &row.3] {
        let value: serde_json::Value = serde_json::from_str(identity).unwrap();
        assert_ne!(value["pid"].as_u64(), Some(u64::from(std::process::id())));
    }
    let state = rusqlite::Connection::open(state_path).unwrap();
    let result: (String, bool, i64, Option<i64>) = state.query_row(
        "SELECT status,success,exit_code,parent_invocation_id FROM invocations WHERE invocation_uuid=?1",
        [&row.1], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!((&result.0[..], result.1, result.2), ("succeeded", true, 0));
    assert!(
        result.3.is_some(),
        "wake must descend from authentic bound parent"
    );
    assert_eq!(
        sqlite_count(
            mailbox_path,
            "SELECT COUNT(*) FROM session_wake_claim WHERE session_id='candidate-bearing-session'"
        ),
        0
    );
    println!("settled original wake={row:?} result={result:?}");
}

// Preserve only this fixture's private files on failure for the collecting root.
struct FailureEvidenceDirectory(Option<tempfile::TempDir>);
impl FailureEvidenceDirectory {
    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}
impl Drop for FailureEvidenceDirectory {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "startup failure evidence: {}",
                self.0.take().unwrap().keep().display()
            );
        }
    }
}
