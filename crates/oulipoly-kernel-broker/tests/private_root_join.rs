//! Source-only private user-namespace exercise of the actual broker launch and
//! opt-in Runner entry. It never exercises installed host-root sudo authority.
#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
use oulipoly_kernel_broker::protocol::{
    self, AcceptedWorkSpec, JoinSpec, Operation, ProcessWitness, SourceScope, SourceSocketWitness,
};
use oulipoly_kernel_broker::source_physical::{SourceObservation, SourcePhysicalRegistry};
use oulipoly_state::completion_continuation::AdmittedSourceBinding;
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, BrokerSidecar, EnqueueResult, FreshV30Lane, MailboxDb,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[track_caller]
fn eventually(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(
            Instant::now() < until,
            "private root join fixture timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn indexed_physical_account(provider_dir: &Path, physical: &str) -> serde_json::Value {
    for entry in fs::read_dir(provider_dir.join("index-v1/accounts")).unwrap() {
        let path = entry.unwrap().path();
        if let Ok(bytes) = fs::read(path)
            && let Ok(sealed) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && sealed["data"]["physical_key"] == physical
        {
            return sealed["data"].clone();
        }
    }
    panic!("indexed physical account {physical} absent");
}

struct SnapshotRestore {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
}

impl Drop for SnapshotRestore {
    fn drop(&mut self) {
        let _ = fs::write(&self.path, &self.bytes);
    }
}

fn inner() {
    let mode = std::env::var("AGE319_PRIVATE_JOIN_MODE").unwrap_or_else(|_| "help".into());
    let v3_quota = mode.starts_with("normal_model_provider_v3_quota");
    let manual_route = mode.starts_with("normal_model_provider_v3_quota_route_manual");
    let shared_manual = mode == "normal_model_provider_v3_quota_route_physical_shared_manual";
    let manual_setup = manual_route || shared_manual;
    let manual_physical = mode.starts_with("normal_model_provider_v3_quota_route_manual_physical");
    let manual_denied = matches!(
        mode.as_str(),
        "normal_model_provider_v3_quota_route_manual_invalid"
            | "normal_model_provider_v3_quota_route_manual_full"
            | "normal_model_provider_v3_quota_route_manual_failed"
            | "normal_model_provider_v3_quota_route_manual_pending"
            | "normal_model_provider_v3_quota_route_manual_unknown"
    );
    // Keep the K switch enabled for the invalid-quota route refusal too.
    let v3_physical = mode.starts_with("normal_model_provider_v3_quota_route_physical")
        || manual_physical
        || manual_denied
        || mode == "normal_model_provider_v3_quota_route_invalid";
    let provider_option = if mode == "normal_model_provider_auth_after_healthy" {
        Some("--auth")
    } else if mode.ends_with("physical_nonzero") {
        Some("--fail")
    } else if mode.ends_with("physical_capacity") {
        Some("--capacity")
    } else if mode.ends_with("physical_account_quota") {
        Some("--quota")
    } else {
        None
    };
    let v3_mode = mode == "normal_model_provider_v3_closed" || v3_quota;
    let provider_mode = mode.starts_with("normal_model_provider");
    let provider_negative = (v3_quota && !v3_physical)
        || manual_denied
        || mode == "normal_model_provider_v3_quota_route_invalid"
        || matches!(
            mode.as_str(),
            "normal_model_provider_bad_config"
                | "normal_model_provider_unsupported"
                | "normal_model_provider_v3_closed"
                | "normal_model_provider_v3_quota"
                | "normal_model_provider_v3_quota_reply_loss"
                | "normal_model_provider_v3_quota_post_k"
                | "normal_model_provider_v3_quota_restart"
                | "normal_model_provider_v3_quota_invalid"
                | "normal_model_provider_v3_quota_full"
                | "normal_model_provider_v3_quota_stale"
                | "normal_model_provider_quota"
                | "normal_model_provider_auth"
        );
    let model_mode = mode == "normal_model_held" || provider_mode;
    let native_mode = mode.starts_with("native_");
    let native_live = matches!(
        mode.as_str(),
        "native_cancel" | "native_drain" | "native_receipt_cancel" | "native_receipt_drain"
    );
    let release_mode = mode.starts_with("held_release") || native_mode;
    let normal_mode = mode.starts_with("normal_");
    let handoff_mode = v3_quota
        || matches!(
            mode.as_str(),
            "normal_handoff"
                | "normal_handoff_bash_child"
                | "normal_handoff_fsync"
                | "normal_handoff_effect_reply_loss"
                | "normal_help"
                | "normal_model_held"
                | "normal_model_provider"
                | "normal_model_provider_reply_loss"
                | "normal_model_provider_q_reply_loss"
                | "normal_model_provider_restart"
                | "normal_model_provider_bad_config"
                | "normal_model_provider_unsupported"
                | "normal_model_provider_v3_closed"
                | "normal_model_provider_v3_quota"
                | "normal_model_provider_v3_quota_restart"
                | "normal_model_provider_quota"
                | "normal_model_provider_auth"
                | "normal_model_provider_auth_recovery"
                | "normal_model_provider_auth_after_healthy"
                | "normal_model_provider_auth_success"
                | "normal_model_provider_auth_reply_loss"
                | "normal_model_provider_auth_restart"
                | "normal_model_provider_quota_available"
                | "normal_model_provider_quota_reply_loss"
                | "normal_model_provider_quota_restart"
                | "normal_model_provider_no_pin"
        );
    let recipient_mode = mode.starts_with("normal_recipient");
    let real_source = mode.starts_with("normal_bash_source");
    let nonzero_source = mode == "normal_bash_source_nonzero";
    let io_failure_source = mode == "normal_bash_source_capture_io_failure";
    let runner =
        std::env::var("OULIPOLY_AGE319_RUNNER_IMAGE").expect("built Runner image required");
    let bash = (mode == "normal_handoff_bash_child")
        .then(|| std::env::var("OULIPOLY_AGE319_BASH_IMAGE").expect("built Bash image required"));
    let bash_request = uuid::Uuid::new_v4().to_string();
    let provider_image = std::env::var("OULIPOLY_AGE319_PROVIDER_IMAGE").unwrap_or_default();
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    // Keep the source for the historical broker copy outside the current
    // Runner's writable entry domain. v29 remains an independent island.
    let historical_data = temp.path().join("historical-data");
    fs::create_dir(&historical_data).unwrap();
    let historical_sidecar = historical_data.join("pid-identity.db");
    let broker_state = temp.path().join("broker-state");
    let gate = temp.path().join("gate");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&broker_state).unwrap();
    fs::set_permissions(&broker_state, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(&gate).unwrap();
    let config_home = temp.path().join("config-home");
    if provider_mode {
        let config_dir = config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(config_dir.join("models")).unwrap();
        let selected_image = if mode.ends_with("physical_account_quota") {
            let path = gate.join("opencode-fixture");
            fs::copy(&provider_image, &path).unwrap();
            path.to_string_lossy().into_owned()
        } else {
            provider_image.clone()
        };
        let provider_command = serde_json::to_string(&selected_image).unwrap();
        let marker = serde_json::to_string(gate.join("provider-effect").to_str().unwrap()).unwrap();
        let unused_marker =
            serde_json::to_string(gate.join("provider-effect-unused").to_str().unwrap()).unwrap();
        let provider_name = if mode == "normal_model_provider_bad_config" {
            "wrong-account"
        } else {
            "local"
        };
        let prompt_mode = if mode == "normal_model_provider_unsupported" {
            "prompt_mode = \"arg\"\n"
        } else {
            ""
        };
        let quota = if v3_quota
            || matches!(
                mode.as_str(),
                "normal_model_provider_quota_available"
                    | "normal_model_provider_quota_reply_loss"
                    | "normal_model_provider_quota_restart"
            ) {
            let quota_bytes: &[u8] = match mode.as_str() {
                "normal_model_provider_v3_quota_route_manual_invalid"
                | "normal_model_provider_v3_quota_invalid"
                | "normal_model_provider_v3_quota_route_invalid"
                | "normal_model_provider_v3_quota_route_auth_failed" => b"invalid quota",
                "normal_model_provider_v3_quota_route_manual_full"
                | "normal_model_provider_v3_quota_full"
                | "normal_model_provider_v3_quota_route_full" => {
                    br#"{"used_percent":100,"resets_at":"2099-01-01T00:00:00Z"}"#
                }
                "normal_model_provider_v3_quota_stale"
                | "normal_model_provider_v3_quota_route_stale" => {
                    br#"{"used_percent":20,"resets_at":"2020-01-01T00:00:00Z"}"#
                }
                _ => br#"{"used_percent":20,"resets_at":"2099-01-01T00:00:00Z"}"#,
            };
            fs::write(gate.join("quota.json"), quota_bytes).unwrap();
            if mode.starts_with("normal_model_provider_v3_quota_auth")
                || mode == "normal_model_provider_v3_quota_route_auth_failed"
            {
                if matches!(
                    mode.as_str(),
                    "normal_model_provider_v3_quota_auth_restart"
                        | "normal_model_provider_v3_quota_auth_shared"
                ) {
                    format!(
                        "quota_script = 'if test -e {0}/auth-ok; then cat {0}/quota.json; else printf invalid; fi'\nauth_refresh_command = 'printf x > {0}/started-auth; while ! test -e {0}/finish-auth; do sleep 0.05; done; printf x >> {0}/auth-ok'\n",
                        gate.display()
                    )
                } else if mode == "normal_model_provider_v3_quota_auth_failed"
                    || mode == "normal_model_provider_v3_quota_route_auth_failed"
                {
                    format!("quota_script = 'printf invalid'\nauth_refresh_command = 'exit 9'\n")
                } else {
                    format!(
                        "quota_script = 'if test -e {0}/auth-ok; then cat {0}/quota.json; else printf invalid; fi'\nauth_refresh_command = 'printf x >> {0}/auth-ok'\n",
                        gate.display()
                    )
                }
            } else if mode == "normal_model_provider_v3_quota_route_manual_failed" {
                "quota_script = 'exit 7'\n".into()
            } else if mode == "normal_model_provider_v3_quota_route_manual_pending" {
                format!(
                    "quota_script = 'while ! test -e {0}/finish-manual-quota; do sleep 0.05; done; cat {0}/quota.json'\n",
                    gate.display()
                )
            } else if mode == "normal_model_provider_v3_quota_restart"
                || mode.ends_with("shared_pending")
            {
                format!(
                    "quota_script = 'printf x > {0}/started-quota; while ! test -e {0}/finish-quota; do sleep 0.05; done; cat {0}/quota.json'\n",
                    gate.display()
                )
            } else {
                format!(
                    "quota_script = 'cat {}'\n",
                    gate.join("quota.json").display()
                )
            }
        } else if mode == "normal_model_provider_quota" {
            "quota_script = \"quota-must-not-run\"\n".into()
        } else if mode == "normal_model_provider_auth_after_healthy" {
            fs::write(
                gate.join("quota.json"),
                br#"{"used_percent":20,"resets_at":"2099-01-01T00:00:00Z"}"#,
            )
            .unwrap();
            format!(
                "quota_script = 'cat {0}/quota.json'\nauth_refresh_command = 'printf x >> {0}/auth-ok'\n",
                gate.display()
            )
        } else if mode == "normal_model_provider_auth_recovery" {
            fs::write(
                gate.join("quota.json"),
                br#"{"used_percent":20,"resets_at":"2099-01-01T00:00:00Z"}"#,
            )
            .unwrap();
            format!(
                "quota_script = 'test -f {} && cat {}'\nauth_refresh_command = 'printf x >> {}'\n",
                gate.join("auth-refreshed").display(),
                gate.join("quota.json").display(),
                gate.join("auth-refreshed").display(),
            )
        } else if mode == "normal_model_provider_auth" {
            "quota_script = \"quota-must-not-run\"\nauth_refresh_command = \"auth-must-not-run\"\n"
                .into()
        } else if matches!(
            mode.as_str(),
            "normal_model_provider_auth_success"
                | "normal_model_provider_auth_reply_loss"
                | "normal_model_provider_auth_restart"
        ) {
            fs::write(
                gate.join("quota.json"),
                br#"{"used_percent":20,"resets_at":"2099-01-01T00:00:00Z"}"#,
            )
            .unwrap();
            format!(
                "quota_script = 'if test -e {0}/auth-ok; then cat {0}/quota.json; else exit 7; fi'\nauth_refresh_command = 'printf x >> {0}/auth-ok'\n",
                gate.display()
            )
        } else {
            String::new()
        };
        let local_args = provider_option.map_or_else(
            || marker.clone(),
            |option| format!("{marker}, \"{option}\""),
        );
        let selected_env = if v3_physical {
            "environment = { AGE319_SELECTED_ACCOUNT = 'physical-local' }\n"
        } else {
            ""
        };
        let authority = if manual_setup {
            format!(
                "settings_id = 'fixture'\nimplementation = {{ family = 'private-test', executable = {provider_command} }}\n"
            )
        } else {
            String::new()
        };
        let second_provider = if mode
            .starts_with("normal_model_provider_v3_quota_route_physical_shared")
            || mode == "normal_model_provider_v3_quota_auth_shared"
        {
            let second_marker =
                serde_json::to_string(gate.join("provider-effect-second").to_str().unwrap())
                    .unwrap();
            let second_quota = if mode.ends_with("shared_command_changed") {
                format!(
                    "quota_script = 'cat {}; true'\n",
                    gate.join("quota.json").display()
                )
            } else {
                quota.clone()
            };
            let second_identity = if mode.ends_with("shared_account_changed") {
                "physical-other"
            } else {
                "physical-local"
            };
            format!(
                "[local-two]\ncommand = {provider_command}\nargs = [{second_marker}]\nquota_account_id = '{second_identity}'\n{authority}{selected_env}{second_quota}"
            )
        } else {
            String::new()
        };
        fs::write(
            config_dir.join("providers.toml"),
            format!(
                "[unused]\ncommand = {provider_command}\nargs = [{unused_marker}]\nquota_account_id = 'physical-unused'\n{authority}[{provider_name}]\ncommand = {provider_command}\nargs = [{local_args}]\nquota_account_id = 'physical-local'\n{authority}{selected_env}{prompt_mode}{quota}{second_provider}"
            ),
        )
        .unwrap();
        fs::write(
            config_dir.join("models/configured-model.toml"),
            "[[providers]]\nname = \"unused\"\n[[providers]]\nname = \"local\"\n",
        )
        .unwrap();
        if mode == "normal_model_provider_v3_quota_route_manual_cross_model"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_cross_model"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_capacity"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_account_quota"
        {
            fs::write(
                config_dir.join("models/alias.toml"),
                "[[providers]]\nname = \"local\"\n",
            )
            .unwrap();
        }
        if mode.starts_with("normal_model_provider_v3_quota_route_physical_shared")
            || mode == "normal_model_provider_v3_quota_auth_shared"
        {
            fs::write(
                config_dir.join("models/configured-model-two.toml"),
                "[[providers]]\nname = \"unused\"\n[[providers]]\nname = \"local-two\"\n",
            )
            .unwrap();
        }
    }
    let mailbox =
        MailboxDb::open_completion_continuation_domain(&data.join("pid-identity.db")).unwrap();
    let pending_binding =
        (normal_mode && !recipient_mode && mode != "normal_empty" && !handoff_mode).then(|| {
            if real_source {
                let registration =
                    fs::read(std::env::var("AGE319_PRIVATE_BASH_REGISTRATION").unwrap()).unwrap();
                let source: oulipoly_state::completion_continuation::SourceRegistration =
                    serde_json::from_slice(&registration).unwrap();
                let state_path = std::env::var("AGE319_PRIVATE_BASH_STATE_DB").unwrap();
                let committed = rusqlite::Connection::open_with_flags(
                    &state_path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let bytes: Vec<u8> = committed
                    .query_row(
                        "SELECT completion_v2_binding FROM invocation_completion_obligations
                     WHERE event_id=?1 AND completion_v2_binding IS NOT NULL",
                        [&source.handle],
                        |row| row.get(0),
                    )
                    .unwrap();
                let binding: AdmittedSourceBinding = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(binding.registration_bytes(), registration);
                assert_eq!(binding.registration().unwrap(), source);
                assert!(
                    !binding.caller_admission_id().is_empty(),
                    "Bash source must have a genuine committed State admission"
                );
                binding
            } else {
                let fixture: serde_json::Value = serde_json::from_str(include_str!(
                    "../../oulipoly-state/tests/fixtures/age360-paired-wire.json"
                ))
                .unwrap();
                AdmittedSourceBinding::new(
                    "fixture-admission",
                    fixture["registration_bytes_utf8"]
                        .as_str()
                        .unwrap()
                        .as_bytes(),
                )
                .unwrap()
            }
        });
    let domain = pending_binding
        .as_ref()
        .map(|binding| binding.registration().unwrap().domain_id)
        .unwrap_or_else(|| mailbox.completion_continuation_domain().unwrap().unwrap());
    let sidecar_generation = mailbox.sidecar_generation().unwrap();
    drop(mailbox);
    rusqlite::Connection::open(data.join("pid-identity.db"))
        .unwrap()
        .execute("VACUUM INTO ?1", [historical_sidecar.to_str().unwrap()])
        .unwrap();
    let recipient_mail = recipient_mode.then(|| {
        let mut old_mailbox = MailboxDb::open(&historical_sidecar).unwrap();
        match old_mailbox
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: "fixture-recipient",
                handle: "fixture-completion",
                payload_json: r#"{"protocol":"source-retention-release-v1","body":"pending"}"#,
                owner_invocation_uuid: Some("fixture-owner"),
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: "fixture-state",
                meta_path: "fixture-meta",
                log_path: "fixture-log",
                rc_path: "fixture-rc",
                rc: 0,
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("recipient fixture enqueue: {other:?}"),
        }
    });
    let old = rusqlite::Connection::open(&historical_sidecar).unwrap();
    old.execute_batch(
        "DROP TRIGGER completion_uncertain_input_preserve;
         DROP TABLE completion_uncertain_input;
         DROP INDEX idx_mailbox_deliverable_session_live;
         DROP INDEX idx_mailbox_deliverable_target_live;
         DROP INDEX idx_mailbox_deliverable_global;
         PRAGMA user_version=29;",
    )
    .unwrap();
    old.execute_batch(include_str!(
        "../../oulipoly-state/src/mailbox/migrations/0022_live_history_barrier.sql"
    ))
    .unwrap();
    // VACUUM INTO creates a rollback-journal copy. Historical cutover source
    // validation requires the v29 island to retain the original WAL mode.
    old.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    drop(old);
    if native_mode {
        rusqlite::Connection::open(&historical_sidecar)
            .unwrap()
            .execute_batch(
                "INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('native-session','native-claim','2026-09-24T00:00:00Z','private-lineage',1);
                 INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('native-sibling-session','native-sibling-claim','2026-09-24T00:00:00Z','private-lineage',1);",
            )
            .unwrap();
    }
    if normal_mode {
        rusqlite::Connection::open(&historical_sidecar)
            .unwrap()
            .execute(
                "UPDATE completion_continuation_domain SET domain_id=?1",
                [&domain],
            )
            .unwrap();
    }
    if !recipient_mode {
        let mut state = oulipoly_state::StateDb::open(&data.join("state.db")).unwrap();
        if let Some(binding) = &pending_binding {
            state
                .seed_private_pending_completion_source(binding, &sidecar_generation)
                .unwrap();
        }
    }
    let broker_generation = if mode == "broker_state"
        || mode == "held_prepared"
        || mode == "held_guardian_death"
        || release_mode
        || normal_mode
    {
        let sidecar_dir = broker_state.join("sidecar");
        fs::create_dir(&sidecar_dir).unwrap();
        fs::set_permissions(&sidecar_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let proof = oulipoly_state::mailbox::QuiescedCutoverProof::private_fixture();
        let generation = if recipient_mode {
            fs::remove_dir(&sidecar_dir).unwrap();
            let source = historical_sidecar.clone();
            let stage =
                BrokerSidecar::stage_private_fixture_copy(&source, &broker_state, &proof).unwrap();
            let generation =
                BrokerSidecar::publish_private_fixture_copy(&source, &stage, &broker_state, &proof)
                    .unwrap();
            drop(oulipoly_state::StateDb::open(&data.join("state.db")).unwrap());
            BrokerSidecar::bind_private_fixture_state_source(
                &sidecar_dir.join("pid-identity.db"),
                &data.join("state.db"),
            )
            .unwrap();
            generation
        } else {
            let target = sidecar_dir.join("pid-identity.db");
            rusqlite::Connection::open(&historical_sidecar)
                .unwrap()
                .execute("VACUUM INTO ?1", [target.to_str().unwrap()])
                .unwrap();
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
            BrokerSidecar::activate_private_fixture_copy(&target, &broker_state, &proof).unwrap()
        };
        if normal_mode && !recipient_mode {
            let target = sidecar_dir.join("pid-identity.db");
            BrokerSidecar::bind_private_fixture_state_source(&target, &data.join("state.db"))
                .unwrap();
        }
        Some(generation)
    } else {
        None
    };
    if handoff_mode {
        rusqlite::Connection::open(broker_state.join("sidecar/pid-identity.db"))
            .unwrap()
            .execute(
                "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
                 state_dir,meta_path,log_path,rc_path,rc)
                 VALUES('old-pending','fixture','old-unacked','{}','2026-09-24T00:00:00Z',
                 '/fixture','/fixture/meta','/fixture/log','/fixture/rc',0)",
                [],
            )
            .unwrap();
    }
    let socket = temp.path().join("broker.sock");
    if handoff_mode {
        FreshV30Lane::initialize_at(&broker_state).unwrap();
    }
    if v3_mode {
        let mut bootstrap = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        eventually(|| {
            broker_state
                .join("v30/fresh-provider/index-v1/admission-protocol.json")
                .exists()
                || bootstrap.try_wait().unwrap().is_some()
        });
        assert!(bootstrap.try_wait().unwrap().is_none());
        stop(&mut bootstrap);
        let rebuilt = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .arg("--offline-rebuild-fresh-index-v3")
            .arg(config_home.join("oulipoly-agent-runner"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .output()
            .unwrap();
        assert!(
            rebuilt.status.success(),
            "{}",
            String::from_utf8_lossy(&rebuilt.stderr)
        );
    }
    let broker_log = temp.path().join("broker.log");
    let mut broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .envs(
            bash.as_ref()
                .map(|path| ("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", path)),
        )
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
        .envs(v3_mode.then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
            "1",
        )))
        .envs(v3_mode.then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
            config_home.join("oulipoly-agent-runner").to_str().unwrap(),
        )))
        .envs(
            mode.starts_with("normal_model_provider_v3_quota_route")
                .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")),
        )
        .envs(v3_physical.then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1", "1")))
        .envs(mode.ends_with("shared_reply_loss").then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_SHARED_Q_REPLY_V3_V1",
            "1",
        )))
        .envs(
            matches!(
                mode.as_str(),
                "normal_model_provider_v3_quota_route_reply_loss"
                    | "normal_model_provider_v3_quota_route_manual_route_reply_loss"
                    | "normal_model_provider_v3_quota_route_manual_physical_route_reply_loss"
            )
            .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ROUTE_V3_REPLY_V1", "1")),
        )
        .envs(
            (mode == "normal_model_provider_v3_quota_route_manual_reply_loss"
                || mode
                    == "normal_model_provider_v3_quota_route_manual_physical_manual_reply_loss")
                .then_some((
                    "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_MANUAL_BEGIN_REPLY_V3_V1",
                    "1",
                )),
        )
        .envs(
            (mode == "normal_model_provider_v3_quota_route_manual_unknown").then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_MANUAL_POST_K_CAS_V3_V1",
                "1",
            )),
        )
        .envs((mode == "normal_model_provider_reply_loss").then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_PROVIDER_K_REPLY_V1",
            "1",
        )))
        .envs(mode.ends_with("physical_reply_loss").then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_PROVIDER_K_REPLY_V1",
            "1",
        )))
        .envs(mode.ends_with("physical_post_k").then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_PROVIDER_POST_K_CAS_V3_V1",
            "1",
        )))
        .envs(
            matches!(
                mode.as_str(),
                "normal_model_provider_quota_reply_loss"
                    | "normal_model_provider_auth_reply_loss"
                    | "normal_model_provider_v3_quota_reply_loss"
            )
            .then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ACCOUNT_EFFECT_REPLY_V1",
                "1",
            )),
        )
        .envs(
            (mode == "normal_model_provider_v3_quota_auth_reply_loss").then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_AUTH_EFFECT_REPLY_V3_V1",
                "1",
            )),
        )
        .envs(native_mode.then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_NATIVE_GATE_V1", &gate)))
        .envs(
            (mode == "normal_model_provider_v3_quota_post_k").then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_QUOTA_POST_K_CAS_V3_V1",
                "1",
            )),
        )
        .envs(
            (mode == "normal_model_provider_v3_quota_auth_post_k").then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_AUTH_POST_K_CAS_V3_V1",
                "1",
            )),
        )
        .envs(
            (mode == "held_release_gate_fail")
                .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_GATE_WRITE_V1", "1")),
        )
        .envs(
            (mode == "normal_handoff_fsync")
                .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_HANDOFF_SYNC_V1", "1")),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&broker_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| {
        (if v3_mode {
            UnixStream::connect(&socket).is_ok()
        } else {
            socket.exists()
        }) || broker.try_wait().unwrap().is_some()
    });
    assert!(
        if v3_mode {
            broker.try_wait().unwrap().is_none() && UnixStream::connect(&socket).is_ok()
        } else {
            socket.exists()
        },
        "broker startup: {}",
        fs::read_to_string(&broker_log).unwrap()
    );
    if manual_setup {
        let run_manual = || {
            Command::new(&runner)
                .arg("--usage")
                .arg("--models-dir")
                .arg(config_home.join("oulipoly-agent-runner/models"))
                .env("OULIPOLY_DATA_DIR", &data)
                .env("OULIPOLY_CONFIG_HOME", &config_home)
                .env(
                    "OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1",
                    socket.with_file_name("v30.sock"),
                )
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
                .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
                .env("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")
                .env("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")
                .env("AGE319_PRIVATE_PROVIDER_IMAGE_V1", &provider_image)
                .env(
                    "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                    gate.join("provider-effect"),
                )
                .env_remove("LD_LIBRARY_PATH")
                .output()
                .unwrap()
        };
        let manual = run_manual();
        assert_eq!(
            manual.status.success(),
            !matches!(
                mode.as_str(),
                "normal_model_provider_v3_quota_route_manual_invalid"
                    | "normal_model_provider_v3_quota_route_manual_failed"
                    | "normal_model_provider_v3_quota_route_manual_pending"
                    | "normal_model_provider_v3_quota_route_manual_unknown"
            ),
            "manual stdout: {} stderr: {} broker: {}",
            String::from_utf8_lossy(&manual.stdout),
            String::from_utf8_lossy(&manual.stderr),
            fs::read_to_string(&broker_log).unwrap()
        );
        if mode == "normal_model_provider_v3_quota_route_manual_refresh"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_refresh"
        {
            fs::write(
                gate.join("quota.json"),
                br#"{"used_percent":24,"resets_at":"2099-01-01T00:00:00Z"}"#,
            )
            .unwrap();
            let refreshed = run_manual();
            assert!(
                refreshed.status.success(),
                "manual refresh: {}",
                String::from_utf8_lossy(&refreshed.stdout)
            );
            assert!(String::from_utf8_lossy(&refreshed.stdout).contains("24%"));
        }
        let operations = broker_state.join("v30/fresh-provider/manual-quota");
        if mode == "normal_model_provider_v3_quota_route_manual_cross_model"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_cross_model"
        {
            let source = fs::read_dir(&operations)
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| entry.path().join("k.json").exists())
                .unwrap();
            let intent: serde_json::Value =
                serde_json::from_slice(&fs::read(source.path().join("intent.json")).unwrap())
                    .unwrap();
            assert_eq!(intent["request"]["model"], "alias");
        }
        assert_eq!(
            fs::read_dir(&operations)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().join("k.json").exists())
                .count(),
            if mode == "normal_model_provider_v3_quota_route_manual_refresh"
                || mode == "normal_model_provider_v3_quota_route_manual_physical_refresh"
            {
                2
            } else {
                1
            },
            "manual call must spend one physical K per deliberate refresh"
        );
    }
    if normal_mode {
        let generation = broker_generation.unwrap();
        fs::write(data.join("pid-identity.db"), b"retired copied owner").unwrap();
        let out = temp.path().join("normal.out");
        let err = temp.path().join("normal.err");
        let mut entry = Command::new(&runner)
            .arg(if mode == "normal_help" {
                "--help"
            } else if model_mode {
                "--model"
            } else if handoff_mode {
                "__age319-private-root-handoff-v1"
            } else {
                "__age319-private-normal-v30"
            })
            .args(if provider_mode {
                if mode == "normal_model_provider_no_pin" {
                    vec!["configured-model", "hello fixture"]
                } else {
                    vec![
                        "configured-model",
                        "--pin-provider",
                        "local",
                        "hello fixture",
                    ]
                }
            } else if model_mode {
                vec!["fixture-model", "hello fixture"]
            } else {
                Vec::new()
            })
            .env("OULIPOLY_DATA_DIR", &data)
            .envs(provider_mode.then_some(("OULIPOLY_CONFIG_HOME", &config_home)))
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .envs(
                bash.as_ref()
                    .map(|path| ("AGE319_PRIVATE_BASH_IMAGE", path)),
            )
            .envs(bash.as_ref().map(|_| ("AGE319_PRIVATE_BASH_CHILD_V1", "1")))
            .envs(bash.as_ref().map(|_| {
                (
                    "AGE319_PRIVATE_BASH_EFFECT_MARKER",
                    gate.join("bash-effect"),
                )
            }))
            .envs(
                bash.as_ref()
                    .map(|_| ("AGE319_PRIVATE_BASH_REQUEST_KEY", &bash_request)),
            )
            .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
            .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
            .envs((mode == "normal_help").then_some(("AGE319_PRIVATE_OFFLINE_ROOT_V1", "1")))
            .envs(model_mode.then_some(("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")))
            .envs(provider_mode.then_some(("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")))
            .envs(
                provider_mode
                    .then_some(("AGE319_PRIVATE_PROVIDER_IMAGE_V1", provider_image.as_str())),
            )
            .envs(provider_mode.then_some((
                "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                gate.join("provider-effect").to_str().unwrap(),
            )))
            .envs(
                (mode == "normal_handoff").then_some(("AGE319_PRIVATE_HANDOFF_REPLY_LOSS_V1", "1")),
            )
            .envs(
                (mode == "normal_handoff_effect_reply_loss")
                    .then_some(("AGE319_PRIVATE_EFFECT_REPLY_LOSS_V1", "1")),
            )
            .envs(pending_binding.as_ref().map(|binding| {
                (
                    "AGE319_PRIVATE_EXPECT_SOURCE_DIGEST_V1",
                    binding.registration_digest(),
                )
            }))
            .envs(
                recipient_mode.then_some(("AGE319_PRIVATE_RECIPIENT_SELECTION_CHALLENGE_V1", "1")),
            )
            .envs(recipient_mail.as_ref().map(|row| {
                (
                    "AGE319_PRIVATE_EXPECT_RECIPIENT_SHA_V1",
                    row.payload_sha256.as_deref().unwrap(),
                )
            }))
            .envs(
                (mode == "normal_prepare_lost_reply")
                    .then_some(("AGE319_PRIVATE_NORMAL_PREPARE_REPLY_LOSS_V1", "1")),
            )
            .envs(
                (mode == "normal_release_lost_reply")
                    .then_some(("AGE319_PRIVATE_NORMAL_RELEASE_REPLY_LOSS_V1", "1")),
            )
            .envs(
                (mode == "normal_repair_lost_reply")
                    .then_some(("AGE319_PRIVATE_REPAIR_REPLY_LOSS_V1", "1")),
            )
            .envs(
                (mode == "normal_source_grant_lost_reply")
                    .then_some(("AGE319_PRIVATE_SOURCE_GRANT_REPLY_LOSS_V1", "1")),
            )
            .envs(real_source.then_some(("AGE319_PRIVATE_SOURCE_LAUNCH_REPLAY_V1", "1")))
            .envs(
                (mode == "normal_bash_source_lost_reply")
                    .then_some(("AGE319_PRIVATE_SOURCE_LAUNCH_REPLY_LOSS_V1", "1")),
            )
            .envs(
                (nonzero_source || io_failure_source)
                    .then_some(("AGE319_PRIVATE_SOURCE_PRELAUNCH_BARRIER_V1", "1")),
            )
            .env_remove("LD_LIBRARY_PATH")
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(&out).unwrap()))
            .stderr(Stdio::from(File::create(&err).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| gate.join("held").exists() || entry.try_wait().unwrap().is_some());
        assert!(
            gate.join("held").exists(),
            "normal held J: {} broker: {}",
            fs::read_to_string(&err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        assert!(!gate.join("prepared").exists());
        let held_db = rusqlite::Connection::open_with_flags(
            broker_state.join("sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let held_running: i64 = held_db
            .query_row(
                "SELECT count(*) FROM completion_continuation_owner",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(held_running, 0);
        let held_prepared: i64 = held_db
            .query_row("SELECT count(*) FROM broker_prepared_owner", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(held_prepared, 0, "held J must precede prepared W");
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            1
        );
        fs::write(gate.join("prepare"), b"yes").unwrap();
        eventually(|| gate.join("prepared").exists() || entry.try_wait().unwrap().is_some());
        assert!(
            gate.join("prepared").exists(),
            "normal entry: {} broker: {}",
            fs::read_to_string(&err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        let prepared: oulipoly_state::mailbox::PreparedBrokerOwner =
            serde_json::from_slice(&fs::read(gate.join("prepared")).unwrap()).unwrap();
        assert_eq!(prepared.source_generation, generation);
        assert_eq!(prepared.entry.host_pid, entry.id() as i32);
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let db = rusqlite::Connection::open_with_flags(
            broker_state.join("sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let running: i64 = db
            .query_row(
                "SELECT count(*) FROM completion_continuation_owner",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(running, 0, "prepared W must not mint running owner");
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(
                broker_state
                    .join("entries")
                    .join(format!("{}.json", prepared.root_id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(record["join_consumed"], true);
        assert_eq!(
            record["joined_child"]["host_pid"],
            prepared.joined_child.host_pid
        );
        assert_eq!(
            fs::read(data.join("pid-identity.db")).unwrap(),
            b"retired copied owner"
        );
        let prepared_read = protocol::StateReadSpec {
            protocol: "broker-prepared-read-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            attempt_id: None,
        };
        assert!(protocol::read_prepared_owner_at(&socket, &prepared_read).is_err());
        if mode == "normal_guardian_death" {
            unsafe { libc::kill(prepared.guardian.host_pid, libc::SIGKILL) };
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            assert!(!entry.wait().unwrap().success());
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        } else if mode == "normal_driver_death" {
            unsafe { libc::kill(prepared.driver.host_pid, libc::SIGKILL) };
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            assert!(!entry.wait().unwrap().success());
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        } else if mode == "normal_broker_death" {
            stop(&mut broker);
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            assert!(!entry.wait().unwrap().success());
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        } else {
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| gate.join("released").exists() || entry.try_wait().unwrap().is_some());
            assert!(
                gate.join("released").exists(),
                "{}",
                fs::read_to_string(&err).unwrap()
            );
            let released: oulipoly_state::mailbox::BrokerReleaseEvidence =
                serde_json::from_slice(&fs::read(gate.join("released")).unwrap()).unwrap();
            assert_eq!(released.prepared, prepared);
            if mode == "normal_handoff_fsync" {
                eventually(|| entry.try_wait().unwrap().is_some());
                assert!(!entry.wait().unwrap().success());
                assert!(!gate.join("child-handoff").exists());
                assert!(!gate.join("child-attested").exists());
                assert!(
                    fs::read_to_string(&err)
                        .unwrap()
                        .contains("private interrupted handoff before fsync")
                );
                assert_eq!(
                    fs::read_to_string(
                        broker_state
                            .join("released-handoffs")
                            .join(format!("{}.json", prepared.root_id))
                    )
                    .unwrap(),
                    "{"
                );
                let fresh = rusqlite::Connection::open_with_flags(
                    broker_state.join("v30/state.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let started: i64 = fresh
                    .query_row("SELECT count(*) FROM invocations", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(started, 0, "uncertain old receipt cannot create fresh D");
                stop(&mut broker);
                broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .stderr(Stdio::from(
                        File::create(temp.path().join("handoff-fsync-restart.log")).unwrap(),
                    ))
                    .spawn()
                    .unwrap();
                eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
                let old = rusqlite::Connection::open_with_flags(
                    broker_state.join("sidecar/pid-identity.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let pending: i64 = old.query_row(
                    "SELECT count(*) FROM mailbox WHERE handle='old-unacked' AND delivered_at IS NULL",
                    [], |row| row.get(0),
                ).unwrap();
                assert_eq!(pending, 1);
                assert!(fs::metadata(&out).unwrap().len() == 0);
                stop(&mut broker);
                return;
            }
            eventually(|| {
                gate.join("child-attested").exists() || entry.try_wait().unwrap().is_some()
            });
            assert!(
                gate.join("child-attested").exists(),
                "{}",
                fs::read_to_string(&err).unwrap()
            );
            if mode == "normal_handoff_bash_child" {
                let report: serde_json::Value =
                    serde_json::from_slice(&fs::read(gate.join("bash-child")).unwrap()).unwrap();
                let child: oulipoly_state::mailbox::FreshBashChild =
                    serde_json::from_value(report["child"].clone()).unwrap();
                let result: oulipoly_state::mailbox::FreshBashPrivateResult =
                    serde_json::from_value(report["result"].clone()).unwrap();
                let root: oulipoly_state::mailbox::FreshReleasedHandoff = serde_json::from_slice(
                    &fs::read(
                        broker_state
                            .join("released-handoffs")
                            .join(format!("{}.json", prepared.root_id)),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(child.request_id, bash_request);
                assert_eq!(child.parent_invocation_uuid, root.invocation_uuid);
                assert_eq!(child.root_id, prepared.root_id);
                assert_ne!(child.invocation_uuid, root.invocation_uuid);
                assert_ne!(child.d_key, root.d_key);
                assert!(child.handle.starts_with("ab30_"));
                assert_eq!(report["stdout"], "v30-child-output\n");
                assert_eq!(result.exit_code, 0);
                assert_eq!(
                    result.stdout_sha256,
                    format!("{:x}", Sha256::digest(b"v30-child-output\n"))
                );
                assert_eq!(report["no_new_privs"], 0);
                assert_eq!(report["seccomp"], 0);
                assert_eq!(fs::read(gate.join("bash-effect")).unwrap(), b"ran\n");
                let fresh = rusqlite::Connection::open_with_flags(
                    broker_state.join("v30/state.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let (parent, session, model): (i64,String,String) = fresh.query_row(
                    "SELECT parent_invocation_id,provider_session_id,model_name FROM invocations WHERE invocation_uuid=?1",
                    [&child.invocation_uuid], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
                let root_id: i64 = fresh
                    .query_row(
                        "SELECT id FROM invocations WHERE invocation_uuid=?1",
                        [&root.invocation_uuid],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(parent, root_id);
                assert_eq!(session, child.session.session_id);
                assert_eq!(model, "agent-bash-child");
                assert_eq!(
                    fresh
                        .query_row("SELECT count(*) FROM invocations", [], |r| r
                            .get::<_, i64>(0))
                        .unwrap(),
                    2
                );
                assert_eq!(
                    fresh
                        .query_row("SELECT count(*) FROM fresh_bash_private_work", [], |r| r
                            .get::<_, i64>(0))
                        .unwrap(),
                    1
                );
                assert_eq!(
                    fresh
                        .query_row("SELECT count(*) FROM fresh_bash_private_result", [], |r| {
                            r.get::<_, i64>(0)
                        })
                        .unwrap(),
                    1
                );
                assert_eq!(
                    oulipoly_kernel_broker::registry::RootRegistry::open(&broker_state)
                        .unwrap()
                        .live_roots()
                        .count(),
                    1,
                    "Bash child must not mint a supervisor/root"
                );
                let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                assert_eq!(
                    lane.read_private_bash_result(&bash_request).unwrap(),
                    Some(result)
                );
                let root_actor = oulipoly_state::mailbox::FreshRecipientIdentity {
                    host_pid: prepared.joined_child.host_pid,
                    boot_id: prepared.joined_child.boot_id.clone(),
                    starttime_ticks: prepared.joined_child.starttime_ticks,
                    pidns_dev: prepared.joined_child.pidns_dev,
                    pidns_ino: prepared.joined_child.pidns_ino,
                };
                let mut wrong_actor = child.actor.clone();
                wrong_actor.starttime_ticks += 1;
                assert!(
                    lane.require_bash_child(&child, &root, &root_actor, &wrong_actor)
                        .is_err()
                );
                wrong_actor = child.actor.clone();
                wrong_actor.pidns_ino += 1;
                assert!(
                    lane.require_bash_child(&child, &root, &root_actor, &wrong_actor)
                        .is_err()
                );
                let mut wrong_parent = child.clone();
                wrong_parent.parent_invocation_uuid = uuid::Uuid::new_v4().to_string();
                assert!(
                    lane.require_bash_child(&wrong_parent, &root, &root_actor, &child.actor)
                        .is_err()
                );
                let mut wrong_d = child.clone();
                wrong_d.session.request_id = root.d_key.clone();
                assert!(
                    lane.require_bash_child(&wrong_d, &root, &root_actor, &child.actor)
                        .is_err()
                );
                let mut partial = child.clone();
                partial.request_id = uuid::Uuid::new_v4().to_string();
                partial.d_key = uuid::Uuid::new_v4().to_string();
                partial.invocation_uuid = uuid::Uuid::new_v4().to_string();
                partial.handle = format!("ab30_{}", uuid::Uuid::new_v4().simple());
                partial.actor.starttime_ticks += 1;
                let fresh_write =
                    rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                fresh_write
                    .execute(
                        "INSERT INTO fresh_bash_child
                     (request_id,d_key,invocation_uuid,handle,root_handoff_id,root_id,
                      parent_invocation_uuid,actor_identity,receipt_json,admitted_at)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'2026-09-24T00:00:00Z')",
                        rusqlite::params![
                            partial.request_id,
                            partial.d_key,
                            partial.invocation_uuid,
                            partial.handle,
                            partial.root_handoff_id,
                            partial.root_id,
                            partial.parent_invocation_uuid,
                            serde_json::to_string(&partial.actor).unwrap(),
                            serde_json::to_string(&partial).unwrap()
                        ],
                    )
                    .unwrap();
                assert!(
                    lane.admit_private_bash_work(&partial).is_err(),
                    "partial child row acquired a work grant"
                );
                assert!(
                    fresh
                        .query_row(
                            "SELECT count(*) FROM fresh_bash_private_work WHERE request_id=?1",
                            [&partial.request_id],
                            |r| r.get::<_, i64>(0),
                        )
                        .unwrap()
                        == 0
                );
                let image_probe = raw_fresh_id_request(
                    &socket.with_file_name("v30.sock"),
                    b'C',
                    uuid::Uuid::new_v4(),
                );
                assert!(
                    image_probe.contains("Bash child image changed"),
                    "{image_probe}"
                );
                assert!(
                    !raw_fresh_id_request(&socket, b'C', uuid::Uuid::new_v4())
                        .starts_with("fresh-bash-child "),
                    "old socket admitted a fresh child"
                );
                // The same pinned Bash image with a copied root key remains
                // outside the root namespace when called by this sibling.
                let outside_marker = gate.join("outside-bash-effect");
                let sibling = Command::new(bash.as_ref().unwrap())
                    .arg("__age319-private-admit-child-v1")
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("AGE319_PRIVATE_BASH_EFFECT_MARKER", &outside_marker)
                    .env("AGE319_PRIVATE_BASH_REQUEST_KEY", &bash_request)
                    .env("AGENT_BASH_OWNER_INVOCATION_UUID", &root.invocation_uuid)
                    .env("AGENT_BASH_OWNER_SESSION_ID", &child.session.session_id)
                    .env(
                        "OULIPOLY_PARENT_INVOCATION",
                        format!("{{\"id\":\"{}\"}}", root.invocation_uuid),
                    )
                    .output()
                    .unwrap();
                assert!(
                    !sibling.status.success(),
                    "copied root data admitted sibling Bash"
                );
                assert!(!outside_marker.exists());
                stop(&mut broker);
                broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .env(
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                        bash.as_ref().unwrap(),
                    )
                    .stderr(Stdio::from(
                        File::create(temp.path().join("bash-child-restart.log")).unwrap(),
                    ))
                    .spawn()
                    .unwrap();
                eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
                let reopened = FreshV30Lane::open_at(&broker_state).unwrap();
                assert_eq!(
                    reopened.read_private_bash_result(&bash_request).unwrap(),
                    lane.read_private_bash_result(&bash_request).unwrap()
                );
                fs::write(gate.join("child-effect"), b"yes").unwrap();
                eventually(|| entry.try_wait().unwrap().is_some());
                assert!(
                    entry.wait().unwrap().success(),
                    "{}",
                    fs::read_to_string(&err).unwrap()
                );
                assert_eq!(
                    fs::read_to_string(&out).unwrap(),
                    format!("OULIPOLY_KERNEL_V30_CHILD_EFFECT={}\n", released.release_id)
                );
                let root_effect: String = fresh
                    .query_row(
                        "SELECT state FROM fresh_root_effect WHERE handoff_id=?1",
                        [&root.handoff_id],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(root_effect, "returned_success");
                stop(&mut broker);
                return;
            }
            if v3_quota
                || matches!(
                    mode.as_str(),
                    "normal_handoff"
                        | "normal_handoff_effect_reply_loss"
                        | "normal_help"
                        | "normal_model_held"
                        | "normal_model_provider"
                        | "normal_model_provider_reply_loss"
                        | "normal_model_provider_q_reply_loss"
                        | "normal_model_provider_restart"
                        | "normal_model_provider_bad_config"
                        | "normal_model_provider_unsupported"
                        | "normal_model_provider_v3_closed"
                        | "normal_model_provider_v3_quota"
                        | "normal_model_provider_quota"
                        | "normal_model_provider_auth"
                        | "normal_model_provider_auth_recovery"
                        | "normal_model_provider_auth_after_healthy"
                        | "normal_model_provider_auth_success"
                        | "normal_model_provider_auth_reply_loss"
                        | "normal_model_provider_auth_restart"
                        | "normal_model_provider_quota_available"
                        | "normal_model_provider_quota_reply_loss"
                        | "normal_model_provider_quota_restart"
                        | "normal_model_provider_no_pin"
                )
            {
                let marker: serde_json::Value =
                    serde_json::from_slice(&fs::read(gate.join("child-handoff")).unwrap()).unwrap();
                let receipt: oulipoly_state::mailbox::FreshReleasedHandoff =
                    serde_json::from_slice(
                        &fs::read(
                            broker_state
                                .join("released-handoffs")
                                .join(format!("{}.json", prepared.root_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                assert_eq!(marker["handoff_id"], receipt.handoff_id);
                assert_eq!(marker["d_key"], receipt.d_key);
                assert_eq!(marker["invocation_uuid"], receipt.invocation_uuid);
                assert_eq!(
                    marker["root_work_intent"],
                    serde_json::to_value(&receipt.root_work_intent).unwrap()
                );
                assert_eq!(
                    marker["no_new_privs"], 0,
                    "root child final exec must permit setuid"
                );
                assert_eq!(
                    marker["seccomp"], 0,
                    "root child final exec has no Runner seccomp filter"
                );
                assert!(receipt.old_release == released);
                assert_eq!(
                    receipt.root_work_intent,
                    if mode == "normal_help" {
                        oulipoly_state::mailbox::FreshRootWorkIntent::CliHelp(vec!["--help".into()])
                    } else if provider_mode {
                        let mut args = vec!["--model".into(), "configured-model".into()];
                        if mode != "normal_model_provider_no_pin" {
                            args.extend(["--pin-provider".into(), "local".into()]);
                        }
                        args.push("hello fixture".into());
                        oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(args)
                    } else if model_mode {
                        oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(vec![
                            "--model".into(),
                            "fixture-model".into(),
                            "hello fixture".into(),
                        ])
                    } else {
                        oulipoly_state::mailbox::FreshRootWorkIntent::PrivateProbe(vec![
                            "__age319-private-root-handoff-v1".into(),
                        ])
                    }
                );
                let receipt_json = serde_json::to_string(&receipt).unwrap();
                assert!(!receipt_json.contains("ab30_"));
                assert!(!receipt_json.contains("bash_handle"));
                let fresh_state = rusqlite::Connection::open_with_flags(
                    broker_state.join("v30/state.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let (session, provider): (String, String) = fresh_state.query_row(
                    "SELECT provider_session_id,provider_name FROM invocations WHERE invocation_uuid=?1",
                    [&receipt.invocation_uuid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                ).unwrap();
                assert_eq!(session, marker["session_id"].as_str().unwrap());
                assert_eq!(provider, "agent-runner");
                let root_model: String = fresh_state
                    .query_row(
                        "SELECT model_name FROM invocations WHERE invocation_uuid=?1",
                        [&receipt.invocation_uuid],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(root_model, "agent-runner-root");
                let invocation_count: i64 = fresh_state
                    .query_row("SELECT count(*) FROM invocations", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(
                    invocation_count, 1,
                    "root with no Bash child has one root invocation"
                );
                let bash_column_count: i64 = fresh_state
                    .query_row("SELECT count(*) FROM pragma_table_info('fresh_released_handoff') WHERE name='bash_handle'", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(
                    bash_column_count, 0,
                    "root State must not carry a Bash placeholder"
                );
                let bound: i64 = fresh_state.query_row(
                    "SELECT count(*) FROM fresh_released_handoff WHERE handoff_id=?1 AND d_key=?2",
                    rusqlite::params![receipt.handoff_id, receipt.d_key],
                    |row| row.get(0),
                ).unwrap();
                assert_eq!(bound, 1);
                let intent_kind: String = fresh_state
                    .query_row(
                        "SELECT root_intent_kind FROM fresh_released_handoff WHERE handoff_id=?1",
                        [&receipt.handoff_id],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    intent_kind,
                    if mode == "normal_help" {
                        "cli_help"
                    } else if model_mode {
                        "normal_cli"
                    } else {
                        "private_probe"
                    }
                );
                assert_eq!(
                    fs::metadata(&out).unwrap().len(),
                    0,
                    "handoff/D must not start Bash work"
                );
                let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                let child = &prepared.joined_child;
                let actor = oulipoly_state::mailbox::FreshRecipientIdentity {
                    host_pid: child.host_pid,
                    boot_id: child.boot_id.clone(),
                    starttime_ticks: child.starttime_ticks,
                    pidns_dev: child.pidns_dev,
                    pidns_ino: child.pidns_ino,
                };
                let session = lane.read_session(&receipt.d_key).unwrap().unwrap();
                assert!(
                    lane.read_root_effect(&receipt, &actor, &session)
                        .unwrap()
                        .is_none()
                );
                if model_mode {
                    assert!(
                        lane.read_normal_work(&receipt, &actor, &session)
                            .unwrap()
                            .is_none()
                    );
                    assert!(
                        protocol::prepare_fresh_normal_work_at(
                            &socket.with_file_name("v30.sock"),
                            &receipt.d_key
                        )
                        .is_err(),
                        "live same-image sibling prepared held normal work"
                    );
                    assert!(
                        protocol::prepare_fresh_normal_work_at(
                            &socket.with_file_name("v30.sock"),
                            &uuid::Uuid::new_v4().to_string()
                        )
                        .is_err(),
                        "wrong key prepared held normal work"
                    );
                }
                assert!(
                    protocol::begin_fresh_root_effect_at(
                        &socket.with_file_name("v30.sock"),
                        &receipt.d_key,
                    )
                    .is_err(),
                    "same-image sibling began root work"
                );
                assert!(
                    protocol::begin_fresh_root_effect_at(
                        &socket.with_file_name("v30.sock"),
                        &uuid::Uuid::new_v4().to_string(),
                    )
                    .is_err(),
                    "wrong D key began root work"
                );
                let mut reused_pid = actor.clone();
                reused_pid.starttime_ticks += 1;
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &receipt, &reused_pid)
                        .is_err()
                );
                if model_mode {
                    assert!(
                        lane.prepare_normal_work(&receipt, &reused_pid, &session)
                            .is_err()
                    );
                }
                let mut wrong_lane = receipt.clone();
                wrong_lane.fresh_lane.lane_id = uuid::Uuid::new_v4().to_string();
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &wrong_lane, &actor)
                        .is_err()
                );
                let mut wrong_domain = receipt.clone();
                wrong_domain.fresh_lane.domain_id = receipt.old_release.prepared.domain_id.clone();
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &wrong_domain, &actor)
                        .is_err()
                );
                let mut wrong_old_lane = receipt.clone();
                wrong_old_lane.old_release.prepared.source_generation =
                    receipt.fresh_lane.source_generation.clone();
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &wrong_old_lane, &actor)
                        .is_err()
                );
                let mut wrong_handle = receipt.clone();
                wrong_handle.root_work_intent =
                    oulipoly_state::mailbox::FreshRootWorkIntent::CliHelp(vec![
                        if mode == "normal_help" {
                            "-h"
                        } else {
                            "--help"
                        }
                        .into(),
                    ]);
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &wrong_handle, &actor)
                        .is_err()
                );
                let mut malformed_intent = receipt.clone();
                malformed_intent.root_work_intent =
                    oulipoly_state::mailbox::FreshRootWorkIntent::CliHelp(vec!["--new".into()]);
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &malformed_intent, &actor)
                        .is_err()
                );
                let mut wrong_invocation = receipt.clone();
                wrong_invocation.invocation_uuid = uuid::Uuid::new_v4().to_string();
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &wrong_invocation, &actor)
                        .is_err()
                );
                drop(lane);
                // A copied locator from this test process has the wrong image
                // and the wrong physical child. It cannot reserve a second U.
                assert!(
                    protocol::request_released_fresh_handoff_at(
                        &socket.with_file_name("v30.sock"),
                        protocol::StateReadSpec {
                            protocol: "broker-release-attest-v30".into(),
                            source_generation: generation.clone(),
                            root_id: prepared.root_id.clone(),
                            owner_generation: prepared.owner_generation.clone(),
                            attempt_id: None,
                        },
                    )
                    .is_err()
                );
                let release_spec = serde_json::to_string(&protocol::StateReadSpec {
                    protocol: "broker-release-attest-v30".into(),
                    source_generation: generation.clone(),
                    root_id: prepared.root_id.clone(),
                    owner_generation: prepared.owner_generation.clone(),
                    attempt_id: None,
                })
                .unwrap();
                assert!(
                    protocol::request_released_fresh_handoff_at(
                        &socket,
                        serde_json::from_str(&release_spec).unwrap(),
                    )
                    .is_err(),
                    "old v29 listener admitted a fresh U"
                );
                let sibling_u = Command::new(&runner)
                    .arg("__age319-private-handoff-probe-v1")
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("AGE319_PRIVATE_HANDOFF_PROBE_SPEC", &release_spec)
                    .status()
                    .unwrap();
                assert!(!sibling_u.success(), "same-image sibling U was admitted");
                let sibling_d = Command::new(&runner)
                    .arg("__age319-private-handoff-probe-v1")
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("AGE319_PRIVATE_HANDOFF_PROBE_D_KEY", &receipt.d_key)
                    .status()
                    .unwrap();
                assert!(!sibling_d.success(), "same-image sibling D was admitted");
                stop(&mut broker);
                broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                    .envs(v3_mode.then_some((
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                        "1",
                    )))
                    .envs(v3_mode.then_some((
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                        config_home.join("oulipoly-agent-runner").to_str().unwrap(),
                    )))
                    .envs(
                        mode.starts_with("normal_model_provider_v3_quota_route")
                            .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")),
                    )
                    .envs(
                        v3_physical
                            .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1", "1")),
                    )
                    .envs(mode.ends_with("shared_reply_loss").then_some((
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_SHARED_Q_REPLY_V3_V1",
                        "1",
                    )))
                    .envs(
                        matches!(
                            mode.as_str(),
                            "normal_model_provider_v3_quota_route_reply_loss"
                                | "normal_model_provider_v3_quota_route_manual_route_reply_loss"
                                | "normal_model_provider_v3_quota_route_manual_physical_route_reply_loss"
                        )
                        .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ROUTE_V3_REPLY_V1", "1")),
                    )
                    .envs((mode == "normal_model_provider_reply_loss").then_some((
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_PROVIDER_K_REPLY_V1",
                        "1",
                    )))
                    .envs(
                        mode.ends_with("physical_reply_loss")
                            .then_some((
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_PROVIDER_K_REPLY_V1",
                                "1",
                            )),
                    )
                    .envs(
                        mode.ends_with("physical_post_k").then_some(
                            (
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_PROVIDER_POST_K_CAS_V3_V1",
                                "1",
                            ),
                        ),
                    )
                    .envs(
                        matches!(
                            mode.as_str(),
                            "normal_model_provider_quota_reply_loss"
                                | "normal_model_provider_v3_quota_reply_loss"
                        )
                        .then_some((
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ACCOUNT_EFFECT_REPLY_V1",
                            "1",
                        )),
                    )
                    .envs(
                        (mode == "normal_model_provider_v3_quota_auth_reply_loss").then_some((
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_AUTH_EFFECT_REPLY_V3_V1",
                            "1",
                        )),
                    )
                    .envs(
                        (mode == "normal_model_provider_v3_quota_post_k").then_some((
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_QUOTA_POST_K_CAS_V3_V1",
                            "1",
                        )),
                    )
                    .envs(
                        (mode == "normal_model_provider_v3_quota_auth_post_k").then_some((
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_AUTH_POST_K_CAS_V3_V1",
                            "1",
                        )),
                    )
                    .stderr(Stdio::from(
                        File::create(temp.path().join("handoff-restart.log")).unwrap(),
                    ))
                    .spawn()
                    .unwrap();
                eventually(|| {
                    socket.with_file_name("v30.sock").exists()
                        && protocol::request_at(&socket, Operation::Classify).is_ok()
                });
                fs::write(gate.join("child-retry"), b"yes").unwrap();
                eventually(|| {
                    gate.join("child-retried").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("child-retried").exists(),
                    "retry: {}",
                    fs::read_to_string(&err).unwrap()
                );
                assert_eq!(
                    fs::read_dir(broker_state.join("released-handoffs"))
                        .unwrap()
                        .count(),
                    1
                );
                let old = rusqlite::Connection::open_with_flags(
                    broker_state.join("sidecar/pid-identity.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let pending: i64 = old
                    .query_row(
                        "SELECT count(*) FROM mailbox WHERE session_id='old-pending'
                     AND handle='old-unacked' AND delivered_at IS NULL",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(pending, 1, "old pending ACK debt must coexist with fresh D");
                let old_state_path = data.join("state.db");
                let old_state_before = fs::read(&old_state_path).unwrap();
                let old_wal_path = data.join("state.db-wal");
                let old_wal_before = fs::read(&old_wal_path).ok();
                let v29_main_before = fs::read(&historical_sidecar).unwrap();
                let v29_wal = historical_data.join("pid-identity.db-wal");
                let v29_wal_before = fs::read(&v29_wal).ok();
                fs::write(gate.join("child-effect"), b"yes").unwrap();
                if mode.ends_with("shared_pending") {
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    eventually(|| {
                        gate.join("started-quota").exists() || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("started-quota").exists(),
                        "first quota K absent: {}",
                        fs::read_to_string(&err).unwrap()
                    );
                    let second_err = temp.path().join("second-pending.err");
                    let mut second = Command::new(&runner)
                        .args([
                            "--model",
                            "configured-model-two",
                            "--pin-provider",
                            "local-two",
                            "hello second",
                        ])
                        .env("OULIPOLY_DATA_DIR", &data)
                        .env("OULIPOLY_CONFIG_HOME", &config_home)
                        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                        .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
                        .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
                        .env("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")
                        .env("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")
                        .env("AGE319_PRIVATE_PROVIDER_IMAGE_V1", &provider_image)
                        .env("AGE319_PRIVATE_SECOND_RESULT_V1", "1")
                        .env(
                            "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                            gate.join("provider-effect"),
                        )
                        .env_remove("LD_LIBRARY_PATH")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::from(File::create(&second_err).unwrap()))
                        .spawn()
                        .unwrap();
                    eventually(|| second.try_wait().unwrap().is_some());
                    assert!(!second.wait().unwrap().success());
                    let second_error = fs::read_to_string(&second_err).unwrap();
                    assert!(
                        second_error.contains("retained sidecar has one running owner"),
                        "{second_error}"
                    );
                    assert!(
                        entry.try_wait().unwrap().is_none(),
                        "first pending-Q root exited early"
                    );
                    assert_eq!(
                        fs::read_dir(broker_state.join("released-handoffs"))
                            .unwrap()
                            .count(),
                        1
                    );
                    let effects = provider_dir.join("account-effects");
                    assert_eq!(
                        fs::read_dir(&effects)
                            .unwrap()
                            .filter_map(Result::ok)
                            .filter(|item| item
                                .file_name()
                                .to_string_lossy()
                                .ends_with("-quota-first"))
                            .count(),
                        1
                    );
                    assert!(
                        !effects
                            .join(format!("{}-1-quota-first/result.json", receipt.handoff_id))
                            .exists()
                    );
                    fs::write(gate.join("finish-quota"), b"yes").unwrap();
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| {
                        gate.join("provider-runtime-result").exists()
                            || entry.try_wait().unwrap().is_some()
                    });
                    fs::write(gate.join("shared-first-release"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    stop(&mut broker);
                    return;
                }
                if mode == "normal_model_provider_v3_quota_auth_shared" {
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    eventually(|| {
                        gate.join("shared-auth-first-ready").exists()
                            || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("shared-auth-first-ready").exists(),
                        "first actor did not settle invalid quota Q: {}",
                        fs::read_to_string(&err).unwrap()
                    );
                    fs::write(gate.join("shared-auth-first-go"), b"yes").unwrap();
                    eventually(|| {
                        gate.join("started-auth").exists() || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("started-auth").exists(),
                        "first actor did not begin physical auth: {}",
                        fs::read_to_string(&err).unwrap()
                    );
                    let second_err = temp.path().join("second-auth.err");
                    let mut second = Command::new(&runner)
                        .args([
                            "--model",
                            "configured-model-two",
                            "--pin-provider",
                            "local-two",
                            "hello second",
                        ])
                        .env("OULIPOLY_DATA_DIR", &data)
                        .env("OULIPOLY_CONFIG_HOME", &config_home)
                        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                        .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
                        .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
                        .env("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")
                        .env("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")
                        .env("AGE319_PRIVATE_PROVIDER_IMAGE_V1", &provider_image)
                        .env("AGE319_PRIVATE_SECOND_RESULT_V1", "1")
                        .env(
                            "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                            gate.join("provider-effect"),
                        )
                        .env_remove("LD_LIBRARY_PATH")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::from(File::create(&second_err).unwrap()))
                        .spawn()
                        .unwrap();
                    eventually(|| second.try_wait().unwrap().is_some());
                    assert!(!second.wait().unwrap().success());
                    let second_error = fs::read_to_string(&second_err).unwrap();
                    assert!(
                        second_error.contains("retained sidecar has one running owner"),
                        "second original root: {second_error}"
                    );
                    assert!(
                        entry.try_wait().unwrap().is_none(),
                        "first root exited early"
                    );
                    assert_eq!(
                        fs::read_dir(broker_state.join("released-handoffs"))
                            .unwrap()
                            .count(),
                        1,
                        "refused second root acquired D"
                    );
                    fs::write(gate.join("finish-auth"), b"yes").unwrap();
                    eventually(|| {
                        gate.join("shared-auth-first-refreshed").exists()
                            || entry.try_wait().unwrap().is_some()
                    });
                    fs::write(gate.join("shared-auth-first-retry-go"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    let first_error = fs::read_to_string(&err).unwrap();
                    assert!(
                        first_error.contains("shared quota retry readback refused")
                            && first_error.contains("auth already spent after rejection"),
                        "{first_error}"
                    );
                    assert!(
                        !provider_dir
                            .join(format!("{}.fresh-grant.json", receipt.handoff_id))
                            .exists()
                    );
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    stop(&mut broker);
                    return;
                }
                if matches!(
                    mode.as_str(),
                    "normal_model_provider_v3_quota_restart"
                        | "normal_model_provider_v3_quota_auth_restart"
                ) {
                    let auth_restart = mode == "normal_model_provider_v3_quota_auth_restart";
                    let effect_dir = broker_state
                        .join("v30/fresh-provider/account-effects")
                        .join(format!(
                            "{}-1-{}",
                            receipt.handoff_id,
                            if auth_restart {
                                "auth-refresh"
                            } else {
                                "quota-first"
                            }
                        ));
                    eventually(|| {
                        effect_dir.exists()
                            && fs::read_dir(&effect_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .any(|entry| {
                                    entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with(".consumed.json")
                                })
                            && gate
                                .join(if auth_restart {
                                    "started-auth"
                                } else {
                                    "started-quota"
                                })
                                .exists()
                    });
                    assert!(!effect_dir.join("result.json").exists());
                    stop(&mut broker);
                    let restart_log = temp.path().join("v3-quota-inflight-restart.log");
                    broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                            "1",
                        )
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                            config_home.join("oulipoly-agent-runner"),
                        )
                        .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                        .spawn()
                        .unwrap();
                    let fresh_socket = socket.with_file_name("v30.sock");
                    eventually(|| {
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                            || broker.try_wait().unwrap().is_some()
                    });
                    assert!(
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok(),
                        "v3 pending quota restart refused: {}",
                        fs::read_to_string(&restart_log).unwrap()
                    );
                    fs::write(
                        gate.join(if auth_restart {
                            "finish-auth"
                        } else {
                            "finish-quota"
                        }),
                        b"yes",
                    )
                    .unwrap();
                }
                if matches!(
                    mode.as_str(),
                    "normal_model_provider_v3_quota_route_reply_loss"
                        | "normal_model_provider_v3_quota_route_manual_route_reply_loss"
                        | "normal_model_provider_v3_quota_route_manual_physical_route_reply_loss"
                ) {
                    let route = broker_state
                        .join("v30/fresh-provider")
                        .join(format!("{}.route-selection.json", receipt.handoff_id));
                    eventually(|| {
                        gate.join("route-reply-dropped").exists() && route.exists()
                            || entry.try_wait().unwrap().is_some()
                            || broker.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("route-reply-dropped").exists() && route.exists(),
                        "route reply did not drop: runner: {}; broker: {}",
                        fs::read_to_string(&err).unwrap(),
                        fs::read_to_string(&broker_log).unwrap()
                    );
                    let before = fs::read(&route).unwrap();
                    stop(&mut broker);
                    let restart_log = temp.path().join("v3-route-inflight-restart.log");
                    broker =
                        Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                                "1",
                            )
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                                config_home.join("oulipoly-agent-runner"),
                            )
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")
                            .envs(v3_physical.then_some((
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1",
                                "1",
                            )))
                            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                            .spawn()
                            .unwrap();
                    let fresh_socket = socket.with_file_name("v30.sock");
                    eventually(|| {
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                            || broker.try_wait().unwrap().is_some()
                    });
                    assert!(
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok(),
                        "v3 route restart refused: {}",
                        fs::read_to_string(&restart_log).unwrap()
                    );
                    assert_eq!(fs::read(&route).unwrap(), before);
                    fs::write(gate.join("route-restarted"), b"yes").unwrap();
                }
                if mode.ends_with("physical_source_changed") {
                    eventually(|| gate.join("v3-provider-ready").exists());
                    fs::write(
                        config_home.join("oulipoly-agent-runner/models/configured-model.toml"),
                        "[[providers]]\nname = \"local\"\n",
                    )
                    .unwrap();
                    fs::write(gate.join("v3-provider-continue"), b"yes").unwrap();
                }
                if mode.ends_with("physical_bad_plan")
                    || mode.ends_with("physical_bad_actor")
                    || mode.ends_with("physical_source_changed")
                {
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    assert!(
                        !provider_dir
                            .join(format!("{}.fresh-grant.json", receipt.handoff_id))
                            .exists()
                    );
                    assert!(!gate.join("provider-effect").exists());
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    stop(&mut broker);
                    return;
                }
                if provider_negative {
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    let stderr = fs::read_to_string(&err).unwrap();
                    if manual_route {
                        let provider_dir = broker_state.join("v30/fresh-provider");
                        let route_file = provider_dir
                            .join(format!("{}.route-selection.json", receipt.handoff_id));
                        let healthy = matches!(
                            mode.as_str(),
                            "normal_model_provider_v3_quota_route_manual_healthy"
                                | "normal_model_provider_v3_quota_route_manual_cross_model"
                                | "normal_model_provider_v3_quota_route_manual_refresh"
                                | "normal_model_provider_v3_quota_route_manual_reply_loss"
                                | "normal_model_provider_v3_quota_route_manual_route_reply_loss"
                        );
                        assert_eq!(route_file.exists(), healthy, "runner: {stderr}");
                        if healthy {
                            let route: serde_json::Value =
                                serde_json::from_slice(&fs::read(&route_file).unwrap()).unwrap();
                            assert_eq!(route["selection"]["account"], "local");
                            assert_eq!(route["selection"]["account_identity"], "physical-local");
                            assert_eq!(route["sequence"], 0);
                            if mode == "normal_model_provider_v3_quota_route_manual_refresh" {
                                assert_eq!(
                                    route["selection"]["quota_remaining_basis_points"],
                                    7600
                                );
                            }
                            assert!(
                                stderr.contains(
                                    "v3 route, cancellation and provider K writers are closed"
                                ),
                                "{stderr}"
                            );
                            let actor_selection: serde_json::Value = serde_json::from_slice(
                                &fs::read(gate.join("v3-route-selection.json")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(actor_selection, route["selection"]);
                        } else {
                            assert!(
                                stderr.contains(
                                    if matches!(
                                        mode.as_str(),
                                        "normal_model_provider_v3_quota_route_manual_pending"
                                            | "normal_model_provider_v3_quota_route_manual_unknown"
                                    ) {
                                        "v3 route has unknown physical-account debt"
                                    } else {
                                        "v3 route has no eligible account or pin"
                                    }
                                ),
                                "{stderr}"
                            );
                        }
                        let effect_parent = provider_dir.join("account-effects");
                        let physical_effects: Vec<_> = if effect_parent.exists() {
                            fs::read_dir(&effect_parent)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry.path().join("intent.json").exists())
                                .map(|entry| entry.file_name())
                                .collect()
                        } else {
                            Vec::new()
                        };
                        assert!(
                            physical_effects.is_empty(),
                            "route actor spent a duplicate physical Q K: {physical_effects:?}"
                        );
                        assert!(
                            !provider_dir
                                .join(format!("{}.fresh-grant.json", receipt.handoff_id))
                                .exists()
                        );
                        assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                        assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                        assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                        assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                        stop(&mut broker);
                        let restart_log = temp.path().join("manual-route-restart.log");
                        broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                                "1",
                            )
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                                config_home.join("oulipoly-agent-runner"),
                            )
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")
                            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                            .spawn()
                            .unwrap();
                        let fresh_socket = socket.with_file_name("v30.sock");
                        eventually(|| {
                            protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                                || broker.try_wait().unwrap().is_some()
                        });
                        assert!(
                            protocol::request_at(&fresh_socket, Operation::ObserveEntryGate)
                                .is_ok(),
                            "manual-route restart refused: {}",
                            fs::read_to_string(&restart_log).unwrap()
                        );
                        stop(&mut broker);
                        return;
                    }
                    let reason = if matches!(
                        mode.as_str(),
                        "normal_model_provider_v3_quota_post_k"
                            | "normal_model_provider_v3_quota_auth_post_k"
                    ) {
                        "fresh account effect unknown"
                    } else if matches!(
                        mode.as_str(),
                        "normal_model_provider_v3_quota_route_invalid"
                            | "normal_model_provider_v3_quota_route_full"
                            | "normal_model_provider_v3_quota_route_stale"
                            | "normal_model_provider_v3_quota_route_auth_failed"
                    ) {
                        "v3 route has no eligible account or pin"
                    } else if v3_quota {
                        "v3 route, cancellation and provider K writers are closed"
                    } else {
                        match mode.as_str() {
                            "normal_model_provider_bad_config" => {
                                "fresh provider \"local\" absent before K"
                            }
                            "normal_model_provider_unsupported" => {
                                "fresh pool has incompatible prompt modes before K"
                            }
                            "normal_model_provider_v3_closed" => {
                                "v3 route, cancellation and provider K writers are closed"
                            }
                            "normal_model_provider_quota" => {
                                "fresh route has no eligible account or pin"
                            }
                            "normal_model_provider_auth" => {
                                "fresh route has no eligible account or pin"
                            }
                            _ => unreachable!(),
                        }
                    };
                    assert!(
                        stderr.contains(reason),
                        "runner: {stderr}; broker status: {:?}; broker: {}; restarted: {}",
                        broker.try_wait().unwrap(),
                        fs::read_to_string(&broker_log).unwrap(),
                        fs::read_to_string(temp.path().join("handoff-restart.log")).unwrap()
                    );
                    assert!(!gate.join("provider-effect").exists());
                    assert!(!gate.join("provider-runtime-result").exists());
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_v3_quota_route"
                            | "normal_model_provider_v3_quota_route_reply_loss"
                    ) {
                        let route: serde_json::Value = serde_json::from_slice(
                            &fs::read(
                                provider_dir
                                    .join(format!("{}.route-selection.json", receipt.handoff_id)),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(route["selection"]["account"], "local");
                        assert_eq!(route["selection"]["index"], 1);
                        assert_eq!(route["sequence"], 0, "pinned choice advanced RR");
                        let actor_selection: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("v3-route-selection.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(actor_selection, route["selection"]);
                    }
                    if v3_quota && !manual_setup {
                        let effect_dir = provider_dir
                            .join("account-effects")
                            .join(format!("{}-1-quota-first", receipt.handoff_id));
                        let intent: serde_json::Value = serde_json::from_slice(
                            &fs::read(effect_dir.join("intent.json")).unwrap(),
                        )
                        .unwrap();
                        if mode != "normal_model_provider_v3_quota_post_k" {
                            let result: serde_json::Value = serde_json::from_slice(
                                &fs::read(effect_dir.join("result.json")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(result["effect_id"], intent["id"]);
                            assert_eq!(
                                result["outcome"],
                                if mode == "normal_model_provider_v3_quota_invalid"
                                    || mode == "normal_model_provider_v3_quota_route_invalid"
                                    || mode == "normal_model_provider_v3_quota_route_auth_failed"
                                    || mode.starts_with("normal_model_provider_v3_quota_auth")
                                {
                                    "invalid"
                                } else {
                                    "valid_windows"
                                }
                            );
                            assert_eq!(result["state"], "drained");
                            if mode == "normal_model_provider_v3_quota_full"
                                || mode == "normal_model_provider_v3_quota_route_full"
                            {
                                assert_eq!(
                                    result["windows"][0]["used_percent"].as_f64(),
                                    Some(100.0)
                                );
                            }
                            if mode == "normal_model_provider_v3_quota_stale"
                                || mode == "normal_model_provider_v3_quota_route_stale"
                            {
                                assert_eq!(
                                    result["windows"][0]["resets_at"],
                                    "2020-01-01T00:00:00Z"
                                );
                            }
                        } else {
                            assert!(!effect_dir.join("result.json").exists());
                        }
                        assert_eq!(
                            fs::read_dir(&effect_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1,
                            "v3 quota probe spent more than one K"
                        );
                        if mode.starts_with("normal_model_provider_v3_quota_auth") {
                            let auth_dir = provider_dir
                                .join("account-effects")
                                .join(format!("{}-1-auth-refresh", receipt.handoff_id));
                            if mode == "normal_model_provider_v3_quota_auth_post_k" {
                                assert!(!auth_dir.join("result.json").exists());
                                assert!(!gate.join("auth-ok").exists());
                                assert_eq!(
                                    fs::read_dir(&auth_dir)
                                        .unwrap()
                                        .filter_map(Result::ok)
                                        .filter(|entry| entry
                                            .file_name()
                                            .to_string_lossy()
                                            .ends_with(".consumed.json"))
                                        .count(),
                                    1
                                );
                                assert_eq!(
                                    fs::read_dir(&auth_dir)
                                        .unwrap()
                                        .filter_map(Result::ok)
                                        .filter(|entry| entry
                                            .file_name()
                                            .to_string_lossy()
                                            .ends_with(".drain.json"))
                                        .count(),
                                    0
                                );
                            } else {
                                let auth: serde_json::Value = serde_json::from_slice(
                                    &fs::read(auth_dir.join("result.json")).unwrap(),
                                )
                                .unwrap();
                                assert_eq!(auth["state"], "drained");
                                assert_eq!(
                                    auth["outcome"],
                                    if mode == "normal_model_provider_v3_quota_auth_failed" {
                                        "failed"
                                    } else {
                                        "refreshed"
                                    }
                                );
                                if mode != "normal_model_provider_v3_quota_auth_failed" {
                                    assert_eq!(fs::read(gate.join("auth-ok")).unwrap(), b"x");
                                }
                                assert_eq!(
                                    fs::read_dir(&auth_dir)
                                        .unwrap()
                                        .filter_map(Result::ok)
                                        .filter(|entry| entry
                                            .file_name()
                                            .to_string_lossy()
                                            .ends_with(".consumed.json"))
                                        .count(),
                                    1
                                );
                                assert_eq!(
                                    fs::read_dir(&auth_dir)
                                        .unwrap()
                                        .filter_map(Result::ok)
                                        .filter(|entry| entry
                                            .file_name()
                                            .to_string_lossy()
                                            .ends_with(".drain.json"))
                                        .count(),
                                    1
                                );
                                let retry_dir = provider_dir
                                    .join("account-effects")
                                    .join(format!("{}-1-quota-retry", receipt.handoff_id));
                                if mode == "normal_model_provider_v3_quota_auth_failed" {
                                    assert!(!retry_dir.exists());
                                } else {
                                    let retry: serde_json::Value = serde_json::from_slice(
                                        &fs::read(retry_dir.join("result.json")).unwrap(),
                                    )
                                    .unwrap();
                                    assert_eq!(retry["outcome"], "valid_windows");
                                }
                            }
                        }
                        assert_eq!(
                            fs::read_dir(&effect_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".drain.json"))
                                .count(),
                            usize::from(mode != "normal_model_provider_v3_quota_post_k"),
                            "v3 quota Q state differs"
                        );
                        assert!(provider_dir.join("index-v1/manifest.json").exists());
                        if matches!(
                            mode.as_str(),
                            "normal_model_provider_v3_quota_reply_loss"
                                | "normal_model_provider_v3_quota_auth_reply_loss"
                        ) {
                            assert!(gate.join("account-effect-reply-dropped").exists());
                        }
                    }
                    assert!(
                        !provider_dir
                            .join(format!("{}.fresh-grant.json", receipt.handoff_id))
                            .exists(),
                        "negative quota authorized provider K"
                    );
                    if !mode.starts_with("normal_model_provider_v3_quota_auth")
                        && !matches!(
                            mode.as_str(),
                            "normal_model_provider_quota"
                                | "normal_model_provider_auth"
                                | "normal_model_provider_v3_closed"
                                | "normal_model_provider_v3_quota"
                                | "normal_model_provider_v3_quota_reply_loss"
                                | "normal_model_provider_v3_quota_post_k"
                                | "normal_model_provider_v3_quota_restart"
                                | "normal_model_provider_v3_quota_invalid"
                                | "normal_model_provider_v3_quota_full"
                                | "normal_model_provider_v3_quota_stale"
                                | "normal_model_provider_v3_quota_route"
                                | "normal_model_provider_v3_quota_route_reply_loss"
                                | "normal_model_provider_v3_quota_route_invalid"
                                | "normal_model_provider_v3_quota_route_full"
                                | "normal_model_provider_v3_quota_route_stale"
                                | "normal_model_provider_v3_quota_route_auth_failed"
                                | "normal_model_provider_v3_quota_auth"
                        )
                    {
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry.file_name() != "index-v1")
                                .count(),
                            0,
                            "pre-K refusal created provider grant/effect"
                        );
                    }
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                    assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                    if v3_quota {
                        stop(&mut broker);
                        let restart_log = temp.path().join("v3-quota-restart.log");
                        broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                                "1",
                            )
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                                config_home.join("oulipoly-agent-runner"),
                            )
                            .envs(
                                mode.starts_with("normal_model_provider_v3_quota_route")
                                    .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")),
                            )
                            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                            .spawn()
                            .unwrap();
                        let fresh_socket = socket.with_file_name("v30.sock");
                        eventually(|| {
                            protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                                || broker.try_wait().unwrap().is_some()
                        });
                        assert!(
                            protocol::request_at(&fresh_socket, Operation::ObserveEntryGate)
                                .is_ok(),
                            "v3 quota restart refused: {}",
                            fs::read_to_string(&restart_log).unwrap()
                        );
                        let effect_dir = provider_dir
                            .join("account-effects")
                            .join(format!("{}-1-quota-first", receipt.handoff_id));
                        assert_eq!(
                            fs::read_dir(&effect_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1,
                            "v3 quota restart replayed physical K"
                        );
                        if mode.starts_with("normal_model_provider_v3_quota_auth") {
                            let auth_dir = provider_dir
                                .join("account-effects")
                                .join(format!("{}-1-auth-refresh", receipt.handoff_id));
                            assert_eq!(
                                fs::read_dir(&auth_dir)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with(".consumed.json"))
                                    .count(),
                                1,
                                "v3 auth restart replayed physical K"
                            );
                        }
                    }
                    stop(&mut broker);
                    return;
                }
                if mode.ends_with("physical_post_k") {
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    let grant: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            provider_dir.join(format!("{}.fresh-grant.json", receipt.handoff_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    let id = grant["id"].as_str().unwrap();
                    assert!(provider_dir.join(format!("{id}.consumed.json")).exists());
                    assert!(!provider_dir.join(format!("{id}.attach.json")).exists());
                    assert!(!provider_dir.join(format!("{id}.drain.json")).exists());
                    assert!(!gate.join("provider-effect").exists());
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    stop(&mut broker);
                    let restart_log = temp.path().join("v3-provider-post-k-restart.log");
                    broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                            "1",
                        )
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                            config_home.join("oulipoly-agent-runner"),
                        )
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1", "1")
                        .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                        .spawn()
                        .unwrap();
                    let fresh_socket = socket.with_file_name("v30.sock");
                    eventually(|| {
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                            || broker.try_wait().unwrap().is_some()
                    });
                    assert!(
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok(),
                        "post-K debt restart refused: {}",
                        fs::read_to_string(&restart_log).unwrap()
                    );
                    assert!(provider_dir.join(format!("{id}.consumed.json")).exists());
                    assert!(!provider_dir.join(format!("{id}.drain.json")).exists());
                    stop(&mut broker);
                    return;
                }
                if mode == "normal_model_provider_auth_after_healthy" {
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    eventually(|| {
                        gate.join("provider-effect").exists() || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("provider-effect").exists(),
                        "runner: {}; broker: {}",
                        fs::read_to_string(&err).unwrap(),
                        fs::read_to_string(temp.path().join("handoff-restart.log")).unwrap()
                    );
                    let route: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            provider_dir
                                .join(format!("{}.route-selection.json", receipt.handoff_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(route["selection"]["account"], "local");
                    let grant: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            provider_dir.join(format!("{}.fresh-grant.json", receipt.handoff_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    let grant_id = grant["id"].as_str().unwrap();
                    eventually(|| provider_dir.join(format!("{grant_id}.exit.json")).exists());
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    assert!(
                        provider_dir
                            .join(format!("{grant_id}.terminal.json"))
                            .exists(),
                        "runner stderr: {}; witness: {:?}",
                        fs::read_to_string(&err).unwrap(),
                        fs::read_to_string(gate.join("provider-runtime-result"))
                    );
                    let terminal: serde_json::Value = serde_json::from_slice(
                        &fs::read(provider_dir.join(format!("{grant_id}.terminal.json"))).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(terminal["outcome"], "auth_rejected");
                    assert!(provider_dir.join(format!("{grant_id}.drain.json")).exists());
                    let effects = provider_dir.join("account-effects");
                    for (kind, outcome) in [
                        ("quota-first", "valid_windows"),
                        ("auth-refresh", "refreshed"),
                        ("quota-retry", "valid_windows"),
                    ] {
                        let dir = effects.join(format!("{}-1-{kind}", receipt.handoff_id));
                        let result: serde_json::Value =
                            serde_json::from_slice(&fs::read(dir.join("result.json")).unwrap())
                                .unwrap();
                        assert_eq!(result["outcome"], outcome);
                        assert_eq!(
                            fs::read_dir(dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1
                        );
                    }
                    assert_eq!(fs::read(gate.join("auth-ok")).unwrap(), b"x");
                    assert_eq!(
                        fs::read_dir(&provider_dir)
                            .unwrap()
                            .filter_map(Result::ok)
                            .filter(|entry| entry
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".consumed.json"))
                            .count(),
                        1
                    );
                    let mapped: serde_json::Value = serde_json::from_slice(
                        &fs::read(gate.join("provider-runtime-result")).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(mapped["auth_after_provider_q"], true);
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                    assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                    stop(&mut broker);
                    return;
                }
                if provider_mode {
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    if mode == "normal_model_provider_no_pin"
                        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_READER_PROBE_V1")
                            .is_some()
                    {
                        eventually(|| entry.try_wait().unwrap().is_some());
                        assert!(!entry.wait().unwrap().success());
                        let stderr = fs::read_to_string(&err).unwrap();
                        assert!(stderr.contains("source census"), "{stderr}");
                        assert!(
                            !provider_dir
                                .join(format!("{}.route-selection.json", receipt.handoff_id))
                                .exists()
                        );
                        assert!(
                            !provider_dir
                                .join(format!("{}.fresh-grant.json", receipt.handoff_id))
                                .exists()
                        );
                        let broker_reads = format!(
                            "{}{}",
                            fs::read_to_string(&broker_log).unwrap(),
                            fs::read_to_string(temp.path().join("handoff-restart.log")).unwrap()
                        );
                        assert!(
                            broker_reads.contains("age319 indexed route-choice read:"),
                            "{broker_reads}"
                        );
                        assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                        assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                        stop(&mut broker);
                        return;
                    }
                    let selected_marker = if mode == "normal_model_provider_no_pin" {
                        gate.join("provider-effect-unused")
                    } else {
                        gate.join("provider-effect")
                    };
                    eventually(|| selected_marker.exists() || entry.try_wait().unwrap().is_some());
                    assert!(
                        selected_marker.exists(),
                        "{}",
                        fs::read_to_string(&err).unwrap()
                    );
                    let grant_file =
                        provider_dir.join(format!("{}.fresh-grant.json", receipt.handoff_id));
                    eventually(|| grant_file.exists());
                    let grant: serde_json::Value =
                        serde_json::from_slice(&fs::read(&grant_file).unwrap()).unwrap();
                    let route_file =
                        provider_dir.join(format!("{}.route-selection.json", receipt.handoff_id));
                    let route: serde_json::Value =
                        serde_json::from_slice(&fs::read(route_file).unwrap()).unwrap();
                    let selected_account = if mode == "normal_model_provider_no_pin" {
                        "unused"
                    } else {
                        "local"
                    };
                    assert_eq!(route["selection"]["account"], selected_account);
                    assert_eq!(
                        route["selection"]["index"],
                        if mode == "normal_model_provider_no_pin" {
                            0
                        } else {
                            1
                        }
                    );
                    assert_eq!(route["selection"]["plan_sha256"], grant["plan_sha256"]);
                    assert_eq!(route["binding"]["handoff_id"], receipt.handoff_id);
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_quota_available"
                            | "normal_model_provider_quota_reply_loss"
                            | "normal_model_provider_quota_restart"
                    ) {
                        let effect_dir = provider_dir
                            .join("account-effects")
                            .join(format!("{}-1-quota-first", receipt.handoff_id));
                        let effect: serde_json::Value = serde_json::from_slice(
                            &fs::read(effect_dir.join("result.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(effect["state"], "drained");
                        assert_eq!(effect["outcome"], "valid_windows");
                        if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_INDEX_V1")
                            .is_some()
                        {
                            let account = indexed_physical_account(&provider_dir, "physical-local");
                            let intent: serde_json::Value = serde_json::from_slice(
                                &fs::read(effect_dir.join("intent.json")).unwrap(),
                            )
                            .unwrap();
                            let indexed = &account["effects"][intent["id"].as_str().unwrap()];
                            assert_eq!(indexed["decision_handoff"], receipt.handoff_id);
                            assert!(indexed["route_source"].is_object());
                            assert!(indexed["candidate"].is_object());
                            assert!(indexed["consumed_k"].is_object());
                            assert!(indexed["certified_q"].is_object());
                            assert!(indexed["result"].is_object());
                        }
                        if mode == "normal_model_provider_quota_reply_loss" {
                            assert!(
                                gate.join("account-effect-reply-dropped").exists(),
                                "quota effect reply was not actually dropped"
                            );
                        }
                        assert_eq!(route["selection"]["quota_remaining_basis_points"], 8000);
                        assert_eq!(
                            route["selection"]["eligible_accounts"],
                            serde_json::json!(["unused", "local"])
                        );
                        assert_eq!(
                            fs::read_dir(&effect_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1,
                            "quota reply loss caused a second effect K"
                        );
                    }
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_auth_recovery"
                            | "normal_model_provider_auth_success"
                            | "normal_model_provider_auth_reply_loss"
                            | "normal_model_provider_auth_restart"
                    ) {
                        let effects = provider_dir.join("account-effects");
                        for (kind, expected) in [
                            ("quota-first", "failed"),
                            ("auth-refresh", "refreshed"),
                            ("quota-retry", "valid_windows"),
                        ] {
                            let effect_dir =
                                effects.join(format!("{}-1-{kind}", receipt.handoff_id));
                            let effect: serde_json::Value = serde_json::from_slice(
                                &fs::read(effect_dir.join("result.json")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(effect["state"], "drained");
                            assert_eq!(effect["outcome"], expected);
                            if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_INDEX_V1")
                                .is_some()
                            {
                                let account =
                                    indexed_physical_account(&provider_dir, "physical-local");
                                let intent: serde_json::Value = serde_json::from_slice(
                                    &fs::read(effect_dir.join("intent.json")).unwrap(),
                                )
                                .unwrap();
                                let indexed = &account["effects"][intent["id"].as_str().unwrap()];
                                assert!(indexed["consumed_k"].is_object());
                                assert!(indexed["certified_q"].is_object());
                                assert!(indexed["result"].is_object());
                            }
                            assert_eq!(
                                fs::read_dir(&effect_dir)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with(".consumed.json"))
                                    .count(),
                                1,
                                "auth reply loss caused a duplicate {kind} K"
                            );
                        }
                        let auth_marker = if mode == "normal_model_provider_auth_recovery" {
                            "auth-refreshed"
                        } else {
                            "auth-ok"
                        };
                        assert_eq!(fs::read(gate.join(auth_marker)).unwrap(), b"x");
                        assert_eq!(route["selection"]["quota_remaining_basis_points"], 8000);
                    }
                    let grant_id = grant["id"].as_str().unwrap();
                    eventually(|| provider_dir.join(format!("{grant_id}.exit.json")).exists());
                    assert!(
                        !provider_dir.join(format!("{grant_id}.drain.json")).exists(),
                        "provider exit with adopted child falsely settled Q"
                    );
                    assert!(
                        protocol::private_fresh_provider_at(
                            &socket.with_file_name("v30.sock"),
                            &receipt.d_key,
                            b'6',
                            None
                        )
                        .is_err(),
                        "sibling observed provider K"
                    );
                    assert!(
                        protocol::private_fresh_route_at(
                            &socket.with_file_name("v30.sock"),
                            &protocol::FreshRouteRequest {
                                protocol_version: 4,
                                d_key: receipt.d_key.clone(),
                                model: "configured-model".into(),
                                config_sha256: route["selection"]["config_sha256"]
                                    .as_str()
                                    .unwrap()
                                    .into(),
                                account: None,
                                account_identity: None,
                                index: None,
                                total: 2,
                                pin: (mode != "normal_model_provider_no_pin")
                                    .then(|| "local".into()),
                                quota_script: None,
                                auth_refresh_command: None,
                                environment_sha256: None,
                            },
                            b'f',
                            &[std::fs::File::open("/").unwrap().as_raw_fd()],
                        )
                        .is_err(),
                        "sibling read back fresh route"
                    );
                    assert!(
                        protocol::private_fresh_provider_at(
                            &socket.with_file_name("v30.sock"),
                            &uuid::Uuid::new_v4().to_string(),
                            b'6',
                            None
                        )
                        .is_err(),
                        "wrong D key observed provider K"
                    );
                    assert_eq!(
                        fs::read(&selected_marker).unwrap(),
                        b"one-provider-effect\n"
                    );
                    assert!(
                        !gate.join("provider-runtime-result").exists(),
                        "runtime mapped a provider result before physical Q"
                    );
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_restart"
                            | "normal_model_provider_quota_restart"
                            | "normal_model_provider_auth_restart"
                            | "normal_model_provider_v3_quota_route_physical_restart"
                            | "normal_model_provider_v3_quota_route_manual_physical_restart"
                    ) {
                        let fresh_socket = socket.with_file_name("v30.sock");
                        stop(&mut broker);
                        assert!(
                            !provider_dir.join(format!("{grant_id}.drain.json")).exists(),
                            "broker death falsely settled adopted work"
                        );
                        broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .envs(v3_physical.then_some((
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                                "1",
                            )))
                            .envs(v3_physical.then_some((
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                                config_home.join("oulipoly-agent-runner").to_str().unwrap(),
                            )))
                            .envs(
                                v3_physical
                                    .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")),
                            )
                            .envs(v3_physical.then_some((
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1",
                                "1",
                            )))
                            .stdout(Stdio::null())
                            .stderr(Stdio::from(
                                File::create(temp.path().join("broker-restart.log")).unwrap(),
                            ))
                            .spawn()
                            .unwrap();
                        let until = Instant::now() + Duration::from_secs(15);
                        while protocol::request_at(
                            &fresh_socket,
                            protocol::Operation::ObserveEntryGate,
                        )
                        .ok()
                        .as_deref()
                            != Some("entry-gate-v1 fresh-v30-closed\n")
                            && broker.try_wait().unwrap().is_none()
                            && Instant::now() < until
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(
                            protocol::request_at(
                                &fresh_socket,
                                protocol::Operation::ObserveEntryGate
                            )
                            .ok()
                            .as_deref()
                                == Some("entry-gate-v1 fresh-v30-closed\n"),
                            "fresh socket did not rebind; broker status {:?}; log {}",
                            broker.try_wait().unwrap(),
                            fs::read_to_string(temp.path().join("broker-restart.log")).unwrap()
                        );
                        assert!(
                            broker.try_wait().unwrap().is_none(),
                            "restarted broker exited: {}",
                            fs::read_to_string(temp.path().join("broker-restart.log")).unwrap()
                        );
                    }
                    if mode == "normal_model_provider_q_reply_loss"
                        || mode.ends_with("physical_q_reply_loss")
                    {
                        fs::write(gate.join("provider-drop-q-reply"), b"yes").unwrap();
                    }
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    let shared_mode =
                        mode.starts_with("normal_model_provider_v3_quota_route_physical_shared");
                    eventually(|| {
                        if shared_mode {
                            gate.join("provider-runtime-result").exists()
                        } else {
                            entry.try_wait().unwrap().is_some()
                        }
                    });
                    if !shared_mode {
                        assert!(
                            !entry.wait().unwrap().success(),
                            "private provider fixture became ordinary CLI success"
                        );
                    }
                    if mode == "normal_model_provider_q_reply_loss"
                        || mode.ends_with("physical_q_reply_loss")
                    {
                        let stderr = fs::read_to_string(&err).unwrap();
                        assert!(stderr.contains("fresh provider unknown:"), "{stderr}");
                        assert!(gate.join("provider-q-reply-dropped").exists());
                        assert!(stderr.contains("\"automatic_replay\":false"), "{stderr}");
                        assert!(stderr.contains(grant_id), "{stderr}");
                        assert!(!gate.join("provider-runtime-result").exists());
                        assert!(provider_dir.join(format!("{grant_id}.drain.json")).exists());
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1,
                            "lost Q reply caused a second K"
                        );
                        assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                        assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                        assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                        assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                        stop(&mut broker);
                        return;
                    }
                    if !shared_mode {
                        assert!(
                            fs::read_to_string(&err)
                                .unwrap()
                                .contains("private provider runtime result mapped after Q; root terminal publication closed"),
                            "{}",
                            fs::read_to_string(&err).unwrap()
                        );
                    }
                    let mapped: serde_json::Value = serde_json::from_slice(
                        &fs::read(gate.join("provider-runtime-result")).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(mapped["mapped_after_q"], true);
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_quota_restart"
                            | "normal_model_provider_auth_restart"
                    ) {
                        assert_eq!(
                            mapped["quota_restart_readback"], true,
                            "quota result was not read back through restarted broker"
                        );
                    }
                    assert_eq!(
                        mapped["exit_code"],
                        if mode.ends_with("physical_nonzero") {
                            9
                        } else if mode.ends_with("physical_capacity")
                            || mode.ends_with("physical_account_quota")
                        {
                            1
                        } else {
                            0
                        }
                    );
                    assert_eq!(
                        mapped["provider_index"],
                        if mode == "normal_model_provider_no_pin" {
                            0
                        } else {
                            1
                        }
                    );
                    assert_eq!(mapped["model"], "configured-model");
                    assert_eq!(
                        mapped["provider"],
                        if mode == "normal_model_provider_no_pin" {
                            "unused"
                        } else {
                            "local"
                        }
                    );
                    assert_eq!(mapped["route_observed_live"], 0);
                    assert_eq!(mapped["route_observed_invocations"], 0);
                    if mode == "normal_model_provider_reply_loss"
                        || mode.ends_with("physical_reply_loss")
                    {
                        assert!(gate.join("provider-k-reply-dropped").exists());
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1,
                            "lost K reply caused duplicate provider launch"
                        );
                    }
                    let short_output = mode.ends_with("physical_capacity")
                        || mode.ends_with("physical_account_quota");
                    if !short_output {
                        assert_eq!(mapped["stdout"], "provider-stdout:hello fixture");
                    }
                    if !v3_physical || !(short_output) {
                        assert_eq!(mapped["stderr"], "provider-stderr\n");
                    }
                    if v3_physical {
                        if manual_route {
                            let account_effects = provider_dir.join("account-effects");
                            let physical_effects: Vec<_> = if account_effects.exists() {
                                fs::read_dir(&account_effects)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry.path().join("intent.json").exists())
                                    .map(|entry| entry.file_name())
                                    .collect()
                            } else {
                                Vec::new()
                            };
                            assert!(
                                physical_effects.is_empty(),
                                "manual Q route spent a duplicate QuotaFirst K: {physical_effects:?}"
                            );
                            let selected: serde_json::Value = serde_json::from_slice(
                                &fs::read(gate.join("v3-route-selection.json")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(selected["account_identity"], "physical-local");
                            if mode.ends_with("physical_refresh") {
                                assert_eq!(selected["quota_remaining_basis_points"], 7600);
                            }
                        }
                        let process: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("provider-effect.process.json")).unwrap(),
                        )
                        .unwrap();
                        let expected: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("v3-provider-expected-plan.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(expected["account_identity"], "physical-local");
                        assert_eq!(process["argv"], expected["argv"]);
                        assert_eq!(process["cwd"], expected["cwd"]);
                        assert_eq!(process["env"], expected["env"]);
                        let mut expected_argv =
                            vec![gate.join("provider-effect").to_string_lossy().into_owned()];
                        if let Some(option) = provider_option {
                            expected_argv.push(option.into());
                        }
                        assert_eq!(process["argv"], serde_json::json!(expected_argv));
                        assert_eq!(
                            process["cwd"],
                            serde_json::json!(std::env::current_dir().unwrap())
                        );
                        assert_eq!(process["selected_account"], "physical-local");
                        assert_eq!(process["uid"], serde_json::json!(unsafe { libc::getuid() }));
                        assert_eq!(process["no_new_privs"], 0);
                        assert_eq!(process["seccomp"], 0);
                        assert_eq!(process["forbidden_environment"], false);
                        let terminal: serde_json::Value = serde_json::from_slice(
                            &fs::read(provider_dir.join(format!("{grant_id}.terminal.json")))
                                .unwrap(),
                        )
                        .unwrap();
                        let expected_outcome = if mode.ends_with("physical_nonzero") {
                            "generic_failure"
                        } else if mode.ends_with("physical_capacity") {
                            "model_at_capacity"
                        } else if mode.ends_with("physical_account_quota") {
                            "quota_rejected"
                        } else {
                            "cancelled"
                        };
                        assert_eq!(terminal["outcome"], expected_outcome);
                        assert_eq!(terminal["selection"]["account_identity"], "physical-local");
                        assert_eq!(terminal["grant_id"], grant_id);
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1
                        );
                    }
                    if !short_output {
                        assert_eq!(
                            fs::read(provider_dir.join(format!("{grant_id}.stdout"))).unwrap(),
                            b"provider-stdout:hello fixture"
                        );
                        assert_eq!(
                            fs::read(provider_dir.join(format!("{grant_id}.stderr"))).unwrap(),
                            b"provider-stderr\n"
                        );
                    }
                    for suffix in [
                        "consumed.json",
                        "attach.json",
                        "exit.json",
                        "drain.json",
                        "pid1-wait.json",
                    ] {
                        assert!(
                            provider_dir.join(format!("{grant_id}.{suffix}")).exists(),
                            "missing {suffix}"
                        );
                    }
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_root_effect",
                                [],
                                |r| r.get(0)
                            )
                            .unwrap(),
                        0
                    );
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_normal_work_preparation",
                                [],
                                |r| r.get(0)
                            )
                            .unwrap(),
                        1
                    );
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                    assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                    if shared_mode {
                        if mode.ends_with("shared_config_changed") {
                            fs::write(
                                config_home
                                    .join("oulipoly-agent-runner/models/configured-model-two.toml"),
                                "[[providers]]\nname = \"unused\"\n",
                            )
                            .unwrap();
                        }
                        if mode.ends_with("shared_restart") {
                            stop(&mut broker);
                            let restart_log = temp.path().join("shared-broker-restart.log");
                            broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                                .env(
                                    "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                                    "1",
                                )
                                .env(
                                    "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                                    config_home.join("oulipoly-agent-runner"),
                                )
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1", "1")
                                .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                                .spawn()
                                .unwrap();
                            eventually(|| {
                                protocol::request_at(&socket, Operation::Classify).is_ok()
                                    || broker.try_wait().unwrap().is_some()
                            });
                            assert!(
                                broker.try_wait().unwrap().is_none(),
                                "shared broker restart: {}",
                                fs::read_to_string(&restart_log).unwrap()
                            );
                        }
                        let second_err = temp.path().join("second.err");
                        let mut second = Command::new(&runner)
                            .args([
                                "--model",
                                "configured-model-two",
                                "--pin-provider",
                                "local-two",
                                "hello second",
                            ])
                            .env("OULIPOLY_DATA_DIR", &data)
                            .env("OULIPOLY_CONFIG_HOME", &config_home)
                            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
                            .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
                            .env("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")
                            .env("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")
                            .env("AGE319_PRIVATE_PROVIDER_IMAGE_V1", &provider_image)
                            .env("AGE319_PRIVATE_SECOND_RESULT_V1", "1")
                            .envs(
                                mode.ends_with("shared_env_changed")
                                    .then_some(("AGE319_SHARED_ENV_CHANGED_V1", "1")),
                            )
                            .env(
                                "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                                gate.join("provider-effect"),
                            )
                            .env_remove("LD_LIBRARY_PATH")
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::from(File::create(&second_err).unwrap()))
                            .spawn()
                            .unwrap();
                        if mode
                            != "normal_model_provider_v3_quota_route_physical_shared_concurrent_red"
                        {
                            eventually(|| second.try_wait().unwrap().is_some());
                            assert!(!second.wait().unwrap().success());
                            let second_error = fs::read_to_string(&second_err).unwrap();
                            assert!(
                                second_error.contains("retained sidecar has one running owner"),
                                "second original root: {second_error}"
                            );
                            assert!(
                                entry.try_wait().unwrap().is_none(),
                                "first root exited early"
                            );
                            assert_eq!(
                                fs::read_dir(broker_state.join("released-handoffs"))
                                    .unwrap()
                                    .count(),
                                1,
                                "refused second root acquired D"
                            );
                            assert!(!gate.join("provider-effect-second").exists());
                            assert!(!gate.join("provider-runtime-result-second").exists());
                            let sidecar = rusqlite::Connection::open(
                                broker_state.join("sidecar/pid-identity.db"),
                            )
                            .unwrap();
                            let owner: String = sidecar.query_row(
                                "SELECT kernel_root_id FROM completion_continuation_owner WHERE phase='running'",
                                [],
                                |row| row.get(0),
                            ).unwrap();
                            assert_eq!(owner, prepared.root_id);
                            fs::write(gate.join("shared-first-release"), b"yes").unwrap();
                            eventually(|| entry.try_wait().unwrap().is_some());
                            assert!(!entry.wait().unwrap().success());
                            assert!(fs::read_to_string(&err).unwrap().contains(
                                "private provider runtime result mapped after Q; root terminal publication closed"
                            ));
                            assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                            assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                            stop(&mut broker);
                            return;
                        }
                        assert!(
                            entry.try_wait().unwrap().is_none(),
                            "first root must remain live during second root K/Q"
                        );
                        eventually(|| second.try_wait().unwrap().is_some());
                        assert!(!second.wait().unwrap().success());
                        let second_error = fs::read_to_string(&second_err).unwrap();
                        if !matches!(
                            mode.as_str(),
                            "normal_model_provider_v3_quota_route_physical_shared"
                                | "normal_model_provider_v3_quota_route_physical_shared_manual"
                                | "normal_model_provider_v3_quota_route_physical_shared_reply_loss"
                                | "normal_model_provider_v3_quota_route_physical_shared_restart"
                        ) && !mode.ends_with("shared_account_changed")
                        {
                            assert!(
                                second_error.contains("shared physical quota readback refused")
                                    || second_error.contains("config source changed")
                                    || second_error.contains("pin")
                                    || second_error.contains("candidate"),
                                "second actor: {second_error}"
                            );
                            let effects = provider_dir.join("account-effects");
                            assert_eq!(
                                fs::read_dir(&effects)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with("-quota-first"))
                                    .count(),
                                1,
                                "incompatible source announced a second quota K"
                            );
                            assert!(!gate.join("provider-effect-second").exists());
                            fs::write(gate.join("shared-first-release"), b"yes").unwrap();
                            eventually(|| entry.try_wait().unwrap().is_some());
                            assert!(!entry.wait().unwrap().success());
                            assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                            assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                            stop(&mut broker);
                            return;
                        }
                        assert!(
                            second_error.contains("private provider runtime result mapped after Q"),
                            "second actor: {second_error}; broker status: {:?}; broker: {}",
                            broker.try_wait().unwrap(),
                            fs::read_to_string(temp.path().join("handoff-restart.log"))
                                .unwrap_or_default()
                        );
                        let second_mapped: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("provider-runtime-result-second")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(second_mapped["model"], "configured-model-two");
                        if mode.ends_with("shared_reply_loss") {
                            assert!(gate.join("account-effect-reply-dropped").exists());
                        }
                        if mode.ends_with("shared_account_changed") {
                            assert_eq!(
                                second_mapped["shared_quota_receipts"]
                                    .as_array()
                                    .unwrap()
                                    .len(),
                                0,
                                "a different physical account reused Q"
                            );
                            assert_eq!(second_mapped["provider"], "local-two");
                            let effects = provider_dir.join("account-effects");
                            assert_eq!(
                                fs::read_dir(&effects)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with("-quota-first"))
                                    .count(),
                                2,
                                "different physical account needs its own quota K"
                            );
                            fs::write(gate.join("shared-first-release"), b"yes").unwrap();
                            eventually(|| entry.try_wait().unwrap().is_some());
                            assert!(!entry.wait().unwrap().success());
                            assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                            assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                            stop(&mut broker);
                            return;
                        }
                        assert_eq!(
                            second_mapped["shared_quota_receipts"]
                                .as_array()
                                .unwrap()
                                .len(),
                            1
                        );
                        assert_eq!(
                            second_mapped["route_config_sha256"].as_str().unwrap().len(),
                            64
                        );
                        let effects = provider_dir.join("account-effects");
                        assert_eq!(
                            fs::read_dir(&effects)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with("-quota-first"))
                                .count(),
                            if shared_manual { 0 } else { 1 },
                            "second actor announced another quota effect"
                        );
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            2,
                            "second actor did not consume its own provider K"
                        );
                        let receipts: Vec<_> = fs::read_dir(broker_state.join("released-handoffs"))
                            .unwrap()
                            .filter_map(Result::ok)
                            .collect();
                        assert_eq!(
                            receipts.len(),
                            2,
                            "two original roots need separate receipts"
                        );
                        let released: Vec<oulipoly_state::mailbox::FreshReleasedHandoff> = receipts
                            .iter()
                            .map(|entry| {
                                serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap()
                            })
                            .collect();
                        assert_ne!(released[0].d_key, released[1].d_key);
                        let second_receipt = released
                            .iter()
                            .find(|value| value.d_key != receipt.d_key)
                            .unwrap();
                        assert_ne!(
                            second_receipt.old_release.prepared.root_id,
                            receipt.old_release.prepared.root_id
                        );
                        let second_route: serde_json::Value = serde_json::from_slice(
                            &fs::read(provider_dir.join(format!(
                                "{}.route-selection.json",
                                second_receipt.handoff_id
                            )))
                            .unwrap(),
                        )
                        .unwrap();
                        let second_grant: serde_json::Value =
                            serde_json::from_slice(
                                &fs::read(provider_dir.join(format!(
                                    "{}.fresh-grant.json",
                                    second_receipt.handoff_id
                                )))
                                .unwrap(),
                            )
                            .unwrap();
                        assert_eq!(
                            second_route["binding"]["handoff_id"],
                            second_receipt.handoff_id
                        );
                        assert_eq!(second_route["selection"]["account"], "local-two");
                        assert_eq!(
                            second_route["selection"]["plan_sha256"],
                            second_grant["plan_sha256"]
                        );
                        assert_eq!(
                            second_route["selection"]["config_sha256"],
                            second_mapped["route_config_sha256"]
                        );
                        let second_process: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("provider-effect-second.process.json")).unwrap(),
                        )
                        .unwrap();
                        let expected_plan: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("v3-provider-expected-plan.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(second_process["argv"], expected_plan["argv"]);
                        assert_eq!(second_process["cwd"], expected_plan["cwd"]);
                        assert_eq!(second_process["env"], expected_plan["env"]);
                        assert_eq!(second_process["selected_account"], "physical-local");
                        assert_eq!(second_process["no_new_privs"], 0);
                        assert_eq!(second_process["seccomp"], 0);
                        assert_eq!(second_process["forbidden_environment"], false);
                        assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                        assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                        fs::write(gate.join("shared-first-release"), b"yes").unwrap();
                        eventually(|| entry.try_wait().unwrap().is_some());
                        assert!(!entry.wait().unwrap().success());
                        assert!(fs::read_to_string(&err).unwrap().contains(
                            "private provider runtime result mapped after Q; root terminal publication closed"
                        ));
                    }
                    stop(&mut broker);
                    return;
                }
                eventually(|| entry.try_wait().unwrap().is_some());
                let completed = entry.wait().unwrap().success();
                if mode == "normal_model_held" {
                    assert!(!completed, "normal provider route must refuse before spawn");
                    assert!(
                        fs::read_to_string(&err)
                            .unwrap()
                            .contains("normal provider route held")
                    );
                    let held: (String, String) = fresh_state.query_row(
                        "SELECT state,intent_json FROM fresh_normal_work_preparation WHERE handoff_id=?1",
                        [&receipt.handoff_id],
                        |row| Ok((row.get(0)?,row.get(1)?)),
                    ).unwrap();
                    assert_eq!(held.0, "held");
                    assert_eq!(
                        serde_json::from_str::<oulipoly_state::mailbox::FreshRootWorkIntent>(
                            &held.1
                        )
                        .unwrap(),
                        receipt.root_work_intent
                    );
                    assert_eq!(fs::metadata(&out).unwrap().len(), 0);
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_root_effect",
                                [],
                                |row| row.get(0)
                            )
                            .unwrap(),
                        0
                    );
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_normal_work_preparation",
                                [],
                                |row| row.get(0)
                            )
                            .unwrap(),
                        1
                    );
                    assert!(
                        protocol::prepare_fresh_normal_work_at(
                            &socket.with_file_name("v30.sock"),
                            &receipt.d_key
                        )
                        .is_err(),
                        "same-image sibling prepared normal work"
                    );
                    assert!(
                        protocol::observe_fresh_normal_work_at(
                            &socket.with_file_name("v30.sock"),
                            &receipt.d_key
                        )
                        .is_err(),
                        "same-image sibling observed normal work"
                    );
                    assert!(
                        protocol::prepare_fresh_normal_work_at(
                            &socket.with_file_name("v30.sock"),
                            &uuid::Uuid::new_v4().to_string()
                        )
                        .is_err(),
                        "wrong D key prepared normal work"
                    );
                } else {
                    let effect: String = fresh_state
                        .query_row(
                            "SELECT state FROM fresh_root_effect WHERE handoff_id=?1",
                            [&receipt.handoff_id],
                            |row| row.get(0),
                        )
                        .unwrap();
                    if mode == "normal_handoff_effect_reply_loss" {
                        assert!(!completed, "lost start reply executed root work");
                        assert_eq!(effect, "started", "lost start is durable unknown");
                        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
                        assert!(
                            fs::read_to_string(&err)
                                .unwrap()
                                .contains("root effect start reply lost; execution refused")
                        );
                    } else {
                        assert!(
                            completed,
                            "handoff child: {}",
                            fs::read_to_string(&err).unwrap()
                        );
                        assert_eq!(effect, "returned_success");
                        if mode == "normal_help" {
                            let help = fs::read_to_string(&out).unwrap();
                            assert!(help.contains("Usage:"), "real CLI help absent: {help}");
                            assert!(!help.contains("OULIPOLY_KERNEL_V30_CHILD_EFFECT"));
                        } else {
                            assert_eq!(
                                fs::read_to_string(&out).unwrap(),
                                format!(
                                    "OULIPOLY_KERNEL_V30_CHILD_EFFECT={}\n",
                                    released.release_id
                                )
                            );
                        }
                    }
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_root_effect",
                                [],
                                |row| row.get(0),
                            )
                            .unwrap(),
                        1
                    );
                }
                stop(&mut broker);
                return;
            }
            let repair_read = protocol::StateReadSpec {
                protocol: "broker-repair-read-v30".into(),
                source_generation: generation.clone(),
                root_id: prepared.root_id.clone(),
                owner_generation: prepared.owner_generation.clone(),
                attempt_id: None,
            };
            assert!(protocol::read_bounded_repair_at(&socket, &repair_read).is_err());
            let source_read = protocol::StateReadSpec {
                protocol: "broker-source-selection-v30".into(),
                source_generation: generation.clone(),
                root_id: prepared.root_id.clone(),
                owner_generation: prepared.owner_generation.clone(),
                attempt_id: None,
            };
            assert!(protocol::read_bounded_source_selection_at(&socket, &source_read).is_err());
            let grant_read = protocol::StateReadSpec {
                protocol: "broker-source-grant-read-v30".into(),
                ..source_read
            };
            assert!(protocol::read_source_effect_grant_at(&socket, &grant_read).is_err());
            let recipient_read = protocol::StateReadSpec {
                protocol: "broker-recipient-selection-v30".into(),
                source_generation: generation.clone(),
                root_id: prepared.root_id.clone(),
                owner_generation: prepared.owner_generation.clone(),
                attempt_id: None,
            };
            assert!(
                protocol::read_bounded_recipient_selection_at(&socket, &recipient_read).is_err()
            );
            let repair_write = protocol::StateWriteSpec {
                protocol: "broker-repair-write-v30".into(),
                source_generation: generation.clone(),
                root_id: prepared.root_id.clone(),
                owner_generation: prepared.owner_generation.clone(),
                action: protocol::StateWriteAction::Repair {
                    expected_ordinal: 0,
                },
            };
            assert!(protocol::write_bounded_repair_at(&socket, &repair_write).is_err());
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            let post_death = matches!(
                mode.as_str(),
                "normal_guardian_post"
                    | "normal_driver_post"
                    | "normal_broker_post"
                    | "normal_recipient_broker_post"
                    | "normal_recipient_driver_post"
            );
            let mut damaged_snapshot = None;
            if mode == "normal_guardian_post" {
                unsafe { libc::kill(prepared.guardian.host_pid, libc::SIGKILL) };
            } else if mode == "normal_driver_post" || mode == "normal_recipient_driver_post" {
                unsafe { libc::kill(prepared.driver.host_pid, libc::SIGKILL) };
            } else if mode == "normal_broker_post" || mode == "normal_recipient_broker_post" {
                stop(&mut broker);
                let restart_log = temp.path().join("normal-post-restart.log");
                broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                    .spawn()
                    .unwrap();
                eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
            }
            if !post_death && pending_binding.is_some() {
                eventually(|| {
                    gate.join("source-grant-ready").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-grant-ready").exists(),
                    "source grant boundary: {}",
                    fs::read_to_string(&err).unwrap_or_default()
                );
                // A sibling caller cannot turn the driver's reserved grant
                // into a physical source effect by naming its root/owner.
                let launch = protocol::StateWriteSpec {
                    protocol: "broker-source-effect-launch-v30".into(),
                    source_generation: generation.clone(),
                    root_id: prepared.root_id.clone(),
                    owner_generation: prepared.owner_generation.clone(),
                    action: protocol::StateWriteAction::LaunchSourceGrant,
                };
                assert!(protocol::launch_source_effect_grant_at(&socket, &launch).is_err());
                let mut stale = launch;
                stale.owner_generation = uuid::Uuid::new_v4().to_string();
                assert!(protocol::launch_source_effect_grant_at(&socket, &stale).is_err());
                if nonzero_source {
                    let source = pending_binding.as_ref().unwrap().registration().unwrap();
                    let path = Path::new(&source.handle_dir).join(&source.snapshot_relative);
                    let original = fs::read(&path).unwrap();
                    fs::write(&path, b"invalid v2 snapshot").unwrap();
                    damaged_snapshot = Some(SnapshotRestore {
                        path,
                        bytes: original,
                    });
                    fs::write(gate.join("source-allow-launch"), b"yes").unwrap();
                    // The original guardian must stay pinned until W has
                    // consumed the grant and acknowledged the held worker.
                    eventually(|| gate.join("source-launched").exists());
                } else if io_failure_source {
                    // Active capture races the test after terminal. Occupy
                    // the create-new name before W can release the worker.
                    use std::os::unix::fs::OpenOptionsExt;
                    let grant_id = fs::read_to_string(gate.join("source-grant-ready")).unwrap();
                    let evidence_path = broker_state
                        .join("source-physical")
                        .join(format!("{grant_id}.evidence.json"));
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(evidence_path)
                        .unwrap();
                    fs::write(gate.join("source-allow-launch"), b"yes").unwrap();
                    eventually(|| gate.join("source-launched").exists());
                } else if real_source {
                    eventually(|| gate.join("source-grant-consumed-readback").exists());
                }
            }
            if (recipient_mode || mode == "normal_empty") && !post_death {
                // Keep the exact joined owner live until the driver's
                // broker read has completed. Releasing the child first can
                // legitimately turn that read into dead-peer refusal.
                eventually(|| {
                    fs::read_to_string(&err).is_ok_and(|message| message.contains("v30 "))
                });
            }
            fs::write(gate.join("child-effect"), b"yes").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            if (real_source || recipient_mode || mode == "normal_empty") && !post_death {
                // The joined child can exit before the outside driver finishes
                // W, especially when the private prelaunch gate is held.
                let deadline = Instant::now() + Duration::from_secs(20);
                while !fs::read_to_string(&err).is_ok_and(|message| message.contains("v30 ")) {
                    assert!(
                        Instant::now() < deadline,
                        "driver did not report boundary: mode={mode} entry={} broker={} driver={:?}",
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default(),
                        prepared.driver,
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            if post_death {
                assert!(!entry.wait().unwrap().success());
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            } else {
                let entry_error = fs::read_to_string(&err).unwrap_or_default();
                let expected_stop = if recipient_mode {
                    Some(
                        "v30 recipient effect closed: no broker-authenticated live recipient or exact wake successor, durable one-use work grant, or pinned provider K/physical child tree",
                    )
                } else if mode == "normal_empty" {
                    Some("v30 no pending broker recipient; wake effect refused")
                } else if real_source && mode != "normal_bash_source_lost_reply" {
                    Some("v30 source physically launched; v2 acceptance remains closed")
                } else if real_source {
                    Some("v30 source launch uncertain or refused")
                } else {
                    None
                };
                if let Some(expected_stop) = expected_stop {
                    assert!(
                        entry_error.contains(expected_stop),
                        "normal repair boundary: entry={entry_error} broker={} status={:?} launched={} physical={:?}",
                        fs::read_to_string(&broker_log).unwrap_or_default(),
                        entry.try_wait().unwrap(),
                        gate.join("source-launched").exists(),
                        fs::read_dir(broker_state.join("source-physical"))
                            .unwrap()
                            .filter_map(Result::ok)
                            .map(|entry| entry.file_name())
                            .collect::<Vec<_>>(),
                    );
                } else {
                    assert!(!gate.join("source-launched").exists());
                }
                if recipient_mode || mode == "normal_empty" {
                    assert!(
                        fs::read(&out).unwrap().is_empty(),
                        "a refused recipient/wake path cannot create child effect"
                    );
                } else {
                    assert_eq!(
                        fs::read_to_string(&out).unwrap(),
                        format!("OULIPOLY_KERNEL_V30_CHILD_EFFECT={}\n", released.release_id)
                    );
                }
                let projected = rusqlite::Connection::open_with_flags(
                    broker_state.join("sidecar/pid-identity.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let ordinal: i64 = projected
                    .query_row(
                        "SELECT COALESCE((SELECT authority_ordinal FROM completion_authority_continuity),0)",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    ordinal,
                    if recipient_mode || mode == "normal_empty" {
                        0
                    } else {
                        1
                    }
                );
                if let Some(binding) = &pending_binding {
                    let source_id = &binding.registration().unwrap().registration_id;
                    let phase: String = projected
                        .query_row(
                            "SELECT phase FROM completion_continuation_source WHERE registration_id=?1",
                            [source_id],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(phase, "registered");
                }
                let attempts: i64 = projected
                    .query_row(
                        "SELECT count(*) FROM completion_continuation_attempt WHERE operation='source_recovery'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(attempts, 0, "source preview cannot reserve or launch");
                if let Some(binding) = &pending_binding {
                    let source_id = &binding.registration().unwrap().registration_id;
                    let grant: (String, String, String, Vec<u8>, String, i64) = projected
                        .query_row(
                            "SELECT phase,registration_id,registration_digest,registration_bytes,
                         owner_generation,revision
                         FROM broker_source_effect_grant",
                            [],
                            |row| {
                                Ok((
                                    row.get(0)?,
                                    row.get(1)?,
                                    row.get(2)?,
                                    row.get(3)?,
                                    row.get(4)?,
                                    row.get(5)?,
                                ))
                            },
                        )
                        .unwrap();
                    assert_eq!(grant.0, if real_source { "consumed" } else { "reserved" });
                    assert_eq!(grant.1, *source_id);
                    assert_eq!(grant.2, binding.registration_digest());
                    assert_eq!(grant.3, binding.registration_bytes());
                    assert_eq!(grant.4, prepared.owner_generation);
                    assert_eq!(grant.5, if real_source { 2 } else { 1 });
                    if real_source {
                        assert_eq!(
                            gate.join("source-launched").exists(),
                            mode != "normal_bash_source_lost_reply"
                        );
                        let physical = broker_state.join("source-physical");
                        let grant_id = fs::read_to_string(gate.join("source-grant-ready")).unwrap();
                        eventually(|| {
                            fs::read_dir(&physical)
                                .unwrap()
                                .filter_map(Result::ok)
                                .any(|entry| {
                                    entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with(".terminal.json")
                                })
                        });
                        eventually(|| {
                            matches!(
                                SourcePhysicalRegistry::open(&physical)
                                    .unwrap()
                                    .observe(&grant_id)
                                    .unwrap(),
                                SourceObservation::Drained { .. }
                            )
                        });
                        let observation = SourcePhysicalRegistry::open(&physical)
                            .unwrap()
                            .observe(&grant_id)
                            .unwrap();
                        assert!(matches!(observation, SourceObservation::Drained {
                        worker_wait_status, cancel_requested: false, ..
                    } if (worker_wait_status == 0) != nonzero_source));
                        let mut retained = BrokerSidecar::open_existing(
                            &broker_state.join("sidecar/pid-identity.db"),
                            &broker_state,
                        )
                        .unwrap();
                        let custody = SourcePhysicalRegistry::open(&physical).unwrap();
                        eventually(|| {
                            retained
                                .read_source_evidence(&custody.records()[0].grant)
                                .unwrap()
                                .is_some()
                        });
                        if nonzero_source {
                            assert!(
                                oulipoly_kernel_broker::source_acceptance::assess_v2_candidate(
                                    &retained, &custody, &grant_id
                                )
                                .is_err()
                            );
                            assert!(oulipoly_kernel_broker::source_acceptance::capture_and_stage_v2_evidence(
                            &mut retained, &custody, &grant_id
                        ).is_err());
                            assert_eq!(
                                retained
                                    .read_source_evidence(&custody.records()[0].grant)
                                    .unwrap()
                                    .unwrap()
                                    .phase,
                                "unknown"
                            );
                            drop(damaged_snapshot.take().unwrap());
                        } else if io_failure_source {
                            assert!(oulipoly_kernel_broker::source_acceptance::capture_and_stage_v2_evidence(
                            &mut retained, &custody, &grant_id
                        ).is_err(), "create-new evidence write must fail closed");
                            assert_eq!(
                                retained
                                    .read_source_evidence(&custody.records()[0].grant)
                                    .unwrap()
                                    .unwrap()
                                    .phase,
                                "unknown"
                            );
                        } else {
                            let assessed =
                                oulipoly_kernel_broker::source_acceptance::assess_v2_candidate(
                                    &retained, &custody, &grant_id,
                                )
                                .unwrap();
                            assert_eq!(
                                assessed.registration_id,
                                pending_binding
                                    .as_ref()
                                    .unwrap()
                                    .registration()
                                    .unwrap()
                                    .registration_id
                            );
                            let captured =
                                oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                                    &retained, &custody, &grant_id
                                ).unwrap();
                            assert_eq!(captured.candidate, assessed);
                            assert_eq!(
                                retained
                                    .read_source_evidence(&custody.records()[0].grant)
                                    .unwrap()
                                    .unwrap()
                                    .phase,
                                "captured"
                            );
                            assert!(oulipoly_kernel_broker::source_acceptance::capture_and_stage_v2_evidence(
                            &mut retained, &custody, &grant_id
                        ).is_err(), "duplicate capture cannot replace broker-owned bytes");
                            assert!(
                                oulipoly_kernel_broker::source_acceptance::commit_v2_evidence(
                                    &mut retained,
                                    &custody,
                                    &grant_id
                                )
                                .is_err(),
                                "re-admitted old v29 source has no fresh v30 authority"
                            );
                            let reply: serde_json::Value = serde_json::from_slice(
                                &fs::read(physical.join(format!("{grant_id}.stdout"))).unwrap(),
                            )
                            .unwrap();
                            let verified = oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                        pending_binding.as_ref().unwrap()).unwrap();
                            verified.validate_source_reply(&reply).unwrap();
                            let source = pending_binding.as_ref().unwrap().registration().unwrap();
                            let snapshot =
                                Path::new(&source.handle_dir).join(&source.snapshot_relative);
                            let original = fs::read(&snapshot).unwrap();
                            let mut changed = original.clone();
                            changed.push(b' ');
                            fs::write(&snapshot, &changed).unwrap();
                            let changed_evidence = oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                        pending_binding.as_ref().unwrap()).unwrap();
                            assert!(
                                changed_evidence.validate_source_reply(&reply).is_err(),
                                "changed original snapshot must not match physical Bash reply"
                            );
                            assert!(
                                oulipoly_kernel_broker::source_acceptance::assess_v2_candidate(
                                    &retained, &custody, &grant_id
                                )
                                .is_err(),
                                "changed original snapshot must not pass broker candidate assessment"
                            );
                            assert!(
                            oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                                &retained, &custody, &grant_id
                            )
                            .is_err(),
                            "changed original snapshot must fail captured readback"
                        );
                            fs::write(&snapshot, original).unwrap();
                            assert_eq!(
                                oulipoly_kernel_broker::source_acceptance::assess_v2_candidate(
                                    &retained, &custody, &grant_id
                                )
                                .unwrap(),
                                assessed
                            );
                            assert_eq!(
                            oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                                &retained, &custody, &grant_id
                            )
                            .unwrap(),
                            captured
                        );
                            let evidence_path = physical.join(format!("{grant_id}.evidence.json"));
                            let owned_bytes = fs::read(&evidence_path).unwrap();
                            let mut damaged = owned_bytes.clone();
                            damaged.push(b' ');
                            fs::write(&evidence_path, &damaged).unwrap();
                            assert!(
                            oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                                &retained, &custody, &grant_id
                            )
                            .is_err(),
                            "same-inode mutation of broker evidence must fail"
                        );
                            fs::write(&evidence_path, owned_bytes).unwrap();
                            assert_eq!(
                            oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                                &retained, &custody, &grant_id
                            )
                            .unwrap(),
                            captured
                        );
                        }
                    }
                } else {
                    let grants: i64 = projected
                        .query_row(
                            "SELECT count(*) FROM broker_source_effect_grant",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(grants, 0, "recipient preview cannot reserve a source grant");
                }
                if let Some(mail) = &recipient_mail {
                    // A human CLI's arbitrary audit label and exact row
                    // selectors cannot turn the retired copy into ACK
                    // authority while the retained broker route is active.
                    for session in ["fixture-recipient", "sibling-recipient"] {
                        for _ in 0..2 {
                            let ack = Command::new(&runner)
                                .args([
                                    "mailbox",
                                    "ack",
                                    "--session-id",
                                    session,
                                    "--from-seq",
                                    &mail.seq.to_string(),
                                    "--to-seq",
                                    &mail.seq.to_string(),
                                    "--delivered-by",
                                    "forged-recipient",
                                ])
                                .env("OULIPOLY_DATA_DIR", &data)
                                .env("OULIPOLY_CONFIG_HOME", &data)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                                .output()
                                .unwrap();
                            assert!(!ack.status.success());
                            assert!(
                                String::from_utf8_lossy(&ack.stderr).contains(
                                    "v30 recipient write requires broker-authenticated recipient grant; retired sidecar refused"
                                ),
                                "{}",
                                String::from_utf8_lossy(&ack.stderr)
                            );
                        }
                    }
                    let pause = Command::new(&runner)
                        .args(["mailbox", "pause", "--session-id", "fixture-recipient"])
                        .env("OULIPOLY_DATA_DIR", &data)
                        .env("OULIPOLY_CONFIG_HOME", &data)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .output()
                        .unwrap();
                    assert!(!pause.status.success());
                    assert!(
                        String::from_utf8_lossy(&pause.stderr).contains(
                            "v30 recipient write requires broker-authenticated recipient grant"
                        ),
                        "{}",
                        String::from_utf8_lossy(&pause.stderr)
                    );
                    let (session, digest, delivered): (String, String, Option<String>) = projected
                        .query_row(
                            "SELECT session_id,payload_sha256,delivered_at FROM mailbox WHERE seq=?1",
                            [mail.seq],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .unwrap();
                    assert_eq!(session, "fixture-recipient");
                    assert_eq!(digest, mail.payload_sha256.as_deref().unwrap());
                    assert!(delivered.is_none(), "selection must not autoACK");
                    let paused: i64 = projected
                        .query_row(
                            "SELECT count(*) FROM mailbox_notification_control WHERE session_id='fixture-recipient' AND paused=1",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(paused, 0, "retired pause cannot change retained control");
                    let claims: i64 = projected
                        .query_row("SELECT count(*) FROM session_wake_claim", [], |row| {
                            row.get(0)
                        })
                        .unwrap();
                    assert_eq!(claims, 0, "selection must not claim or launch");
                }
            }
            assert_eq!(
                fs::read(data.join("pid-identity.db")).unwrap(),
                b"retired copied owner"
            );
        }
        if matches!(mode.as_str(), "normal_release" | "normal_recipient") {
            // Restart the actual old sidecar/source or pending recipient
            // authority after an independent empty fresh publication.
            FreshV30Lane::initialize_at(&broker_state).unwrap();
        }
        if mode != "normal_broker_death" {
            stop(&mut broker);
        }
        let restart_log = temp.path().join("normal-restart.log");
        let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
        if mode == "normal_recipient" {
            let mail = recipient_mail.as_ref().unwrap();
            let retained = rusqlite::Connection::open_with_flags(
                broker_state.join("sidecar/pid-identity.db"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let (digest, delivered): (String, Option<String>) = retained
                .query_row(
                    "SELECT payload_sha256,delivered_at FROM mailbox WHERE seq=?1",
                    [mail.seq],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(digest, mail.payload_sha256.as_deref().unwrap());
            assert!(
                delivered.is_none(),
                "restart must retain pending old ACK debt"
            );
        }
        let second = Command::new(&runner)
            .arg("__age319-private-normal-v30")
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .output()
            .unwrap();
        assert!(!second.status.success());
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            1
        );
        if matches!(
            mode.as_str(),
            "normal_release"
                | "normal_prepare_lost_reply"
                | "normal_release_lost_reply"
                | "normal_repair_lost_reply"
                | "normal_source_grant_lost_reply"
        ) {
            let retained = rusqlite::Connection::open_with_flags(
                broker_state.join("sidecar/pid-identity.db"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let ordinal: i64 = retained
                .query_row(
                    "SELECT authority_ordinal FROM completion_authority_continuity",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(ordinal, 1, "restart must retain exact repair cursor");
            let grant: (String, i64, i64) = retained
                .query_row(
                    "SELECT phase,revision,count(*) FROM broker_source_effect_grant",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                grant,
                ("unknown".into(), 2, 1),
                "lost custody must retain one unknown grant"
            );
        }
        if real_source {
            let retained = rusqlite::Connection::open_with_flags(
                broker_state.join("sidecar/pid-identity.db"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let grant: (String, i64, i64) = retained
                .query_row(
                    "SELECT phase,revision,count(*) FROM broker_source_effect_grant",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(grant, ("consumed".into(), 2, 1));
            let grant_id = fs::read_to_string(gate.join("source-grant-ready")).unwrap();
            let registration_id = pending_binding
                .as_ref()
                .unwrap()
                .registration()
                .unwrap()
                .registration_id;
            let source_phase: String = retained
                .query_row(
                    "SELECT phase FROM completion_continuation_source WHERE registration_id=?1",
                    [&registration_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                source_phase, "registered",
                "evidence debt must not release the source"
            );
            assert!(matches!(
                SourcePhysicalRegistry::open(broker_state.join("source-physical"))
                    .unwrap()
                    .observe(&grant_id)
                    .unwrap(),
                SourceObservation::Drained { .. }
            ));
            let reopened = BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap();
            let physical =
                SourcePhysicalRegistry::open(broker_state.join("source-physical")).unwrap();
            let debt = reopened
                .read_source_evidence(&physical.records()[0].grant)
                .unwrap()
                .unwrap();
            assert_eq!(
                debt.phase,
                if nonzero_source || io_failure_source {
                    "unknown"
                } else {
                    "captured"
                }
            );
            if !nonzero_source && !io_failure_source {
                assert!(
                    oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                        &reopened, &physical, &grant_id
                    )
                    .is_ok()
                );
            }
        }
        stop(&mut restarted);
        unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
        return;
    }
    if mode == "held_prepared" || mode == "held_guardian_death" || release_mode {
        let generation = broker_generation.unwrap();
        // A copied v29 path is unusable before E and throughout W/R.
        fs::write(data.join("pid-identity.db"), b"retired copied owner").unwrap();
        let out = temp.path().join("held.out");
        let err = temp.path().join("held.err");
        let mut entry = Command::new(&runner)
            .arg("__age319-private-held-prepared-v30")
            .env("OULIPOLY_DATA_DIR", &data)
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .envs(
                (mode == "held_guardian_death")
                    .then_some(("AGE319_PRIVATE_EXPECT_GUARDIAN_DEATH_V1", "1")),
            )
            .envs(release_mode.then_some(("AGE319_PRIVATE_RELEASE_V30", "1")))
            .envs(
                (mode == "held_release_prepare_lost_reply")
                    .then_some(("AGE319_PRIVATE_PREPARE_LOST_REPLY_V1", "1")),
            )
            .envs(
                (mode == "held_release_driver_route" || native_mode)
                    .then_some(("AGE319_PRIVATE_DRIVER_ROUTE_V30", "1")),
            )
            .envs(native_mode.then_some(("AGE319_PRIVATE_NATIVE_LINEAGE_V30", "1")))
            .envs(
                mode.starts_with("native_receipt_")
                    .then_some(("AGE319_PRIVATE_RECEIPT_HELPER_PROBE_V1", "1")),
            )
            .envs(
                (mode == "held_release_exec_driver")
                    .then_some(("AGE319_PRIVATE_EXEC_DRIVER_ROUTE_V30", "1")),
            )
            .envs(
                (mode == "held_release_lost_reply")
                    .then_some(("AGE319_PRIVATE_RELEASE_LOST_REPLY_V1", "1")),
            )
            .env_remove("LD_LIBRARY_PATH")
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(&out).unwrap()))
            .stderr(Stdio::from(File::create(&err).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| gate.join("prepared").exists() || entry.try_wait().unwrap().is_some());
        assert!(
            gate.join("prepared").exists(),
            "entry: {} broker: {}",
            fs::read_to_string(&err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        let prepared: oulipoly_state::mailbox::PreparedBrokerOwner =
            serde_json::from_slice(&fs::read(gate.join("prepared")).unwrap()).unwrap();
        assert_eq!(prepared.source_generation, generation);
        assert_eq!(prepared.entry.host_pid, entry.id() as i32);
        assert_eq!(prepared.domain_id, domain);
        assert_ne!(prepared.entry.pidns_ino, prepared.joined_child.pidns_ino);
        let entry_record: serde_json::Value = serde_json::from_slice(
            &fs::read(
                broker_state
                    .join("entries")
                    .join(format!("{}.json", prepared.root_id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            entry_record["prepared_driver"]["host_pid"],
            prepared.driver.host_pid
        );
        assert_eq!(
            entry_record["prepared_driver"]["starttime_ticks"],
            prepared.driver.starttime_ticks
        );
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        assert!(entry.try_wait().unwrap().is_none());
        let persisted = BrokerSidecar::open_existing(
            &broker_state.join("sidecar/pid-identity.db"),
            &broker_state,
        )
        .unwrap()
        .read_exact_prepared_owner(&generation, &prepared.root_id, &prepared.owner_generation)
        .unwrap();
        assert_eq!(persisted, prepared);
        assert!(
            BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap()
            .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
            .is_err()
        );
        let db = rusqlite::Connection::open_with_flags(
            broker_state.join("sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let running: i64 = db
            .query_row(
                "SELECT count(*) FROM completion_continuation_owner",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(running, 0);
        let read = protocol::StateReadSpec {
            protocol: "broker-prepared-read-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            attempt_id: None,
        };
        assert!(protocol::read_prepared_owner_at(&socket, &read).is_err()); // wrong actor
        let child_attest = protocol::StateReadSpec {
            protocol: "broker-release-attest-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            attempt_id: None,
        };
        assert!(protocol::attest_released_child_at(&socket, &child_attest).is_err());
        let forged = protocol::StateWriteSpec {
            protocol: "broker-prepared-write-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            action: protocol::StateWriteAction::Prepare {
                driver_pid: prepared.driver.host_pid,
                endpoint: prepared.endpoint.clone(),
            },
        };
        assert!(protocol::prepare_owner_at(&socket, &forged).is_err());
        let mut wrong = read;
        wrong.source_generation = uuid::Uuid::new_v4().to_string();
        assert!(protocol::read_prepared_owner_at(&socket, &wrong).is_err());
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let wrong_release = protocol::StateWriteSpec {
            protocol: "broker-held-release-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            action: protocol::StateWriteAction::Release,
        };
        assert!(protocol::release_prepared_owner_at(&socket, &wrong_release).is_err());
        if mode == "held_guardian_death" {
            unsafe {
                libc::kill(prepared.guardian.host_pid, libc::SIGKILL);
            }
            fs::write(gate.join("guardian-dead"), b"yes").unwrap();
            eventually(|| {
                gate.join("death-refused").exists() || entry.try_wait().unwrap().is_some()
            });
            assert!(
                gate.join("death-refused").exists(),
                "{}",
                fs::read_to_string(&err).unwrap()
            );
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        }
        if release_mode {
            if mode == "held_release_commit_fail" {
                rusqlite::Connection::open(broker_state.join("sidecar/pid-identity.db"))
                    .unwrap()
                    .execute_batch("CREATE TRIGGER age319_release_fault BEFORE INSERT ON broker_owner_release BEGIN SELECT RAISE(ABORT, 'fixture commit fault'); END;")
                    .unwrap();
            }
            if mode == "held_release_child_predeath" {
                unsafe {
                    libc::kill(prepared.joined_child.host_pid, libc::SIGKILL);
                }
            }
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| gate.join("released").exists() || entry.try_wait().unwrap().is_some());
            if mode == "held_release_commit_fail"
                || mode == "held_release_child_predeath"
                || mode == "held_release_gate_fail"
            {
                assert!(!gate.join("released").exists());
                let errors = fs::read_to_string(&err).unwrap();
                if mode == "held_release_commit_fail" {
                    assert!(errors.contains("fixture commit fault"), "{errors}");
                }
                if mode == "held_release_gate_fail" {
                    assert!(
                        errors.contains("Broken pipe") || errors.contains("os error 32"),
                        "{errors}"
                    );
                }
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
                assert!(
                    BrokerSidecar::open_existing(
                        &broker_state.join("sidecar/pid-identity.db"),
                        &broker_state
                    )
                    .unwrap()
                    .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
                    .is_err()
                );
                let still_zero: i64 = db
                    .query_row(
                        "SELECT count(*) FROM completion_continuation_owner",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(still_zero, 0);
                stop(&mut broker);
                unsafe {
                    libc::kill(prepared.root_init.host_pid, libc::SIGKILL);
                }
                return;
            }
            assert!(
                gate.join("released").exists(),
                "entry: {} broker: {}",
                fs::read_to_string(&err).unwrap(),
                fs::read_to_string(&broker_log).unwrap()
            );
            let evidence: oulipoly_state::mailbox::BrokerReleaseEvidence =
                serde_json::from_slice(&fs::read(gate.join("released")).unwrap()).unwrap();
            assert_eq!(evidence.prepared, prepared);
            assert_eq!(evidence.owner.owner_generation, prepared.owner_generation);
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            assert_eq!(
                db.query_row::<i64, _, _>(
                    "SELECT count(*) FROM completion_continuation_owner",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
                1
            );
            eventually(|| {
                gate.join("child-attested").exists() || entry.try_wait().unwrap().is_some()
            });
            assert!(
                gate.join("child-attested").exists(),
                "child: {} broker: {}",
                fs::read_to_string(&err).unwrap(),
                fs::read_to_string(&broker_log).unwrap()
            );
            assert_eq!(
                fs::read_to_string(gate.join("child-attested")).unwrap(),
                evidence.release_id
            );
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            let durable = BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap()
            .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
            .unwrap();
            assert_eq!(durable, evidence);
            if native_mode {
                eventually(|| {
                    gate.join("native-prepared").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("native-prepared").exists(),
                    "native N: entry={} broker={}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                );
                let grant = fs::read_to_string(gate.join("native-prepared")).unwrap();
                let native_db = broker_state.join("sidecar/pid-identity.db");
                let read_native = || {
                    rusqlite::Connection::open_with_flags(
                        &native_db,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap()
                };
                let state = read_native();
                let (attempt, phase, protocol, bound): (String, String, String, String) = state
                    .query_row(
                        "SELECT a.attempt_id,a.phase,b.protocol,b.grant_id FROM completion_continuation_attempt a JOIN completion_native_grant_binding b ON b.attempt_id=a.attempt_id WHERE a.session_id='native-session'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .unwrap();
                assert_eq!(phase, "accepted");
                // The State binding schema uses its original v1 field; the
                // broker's exact v5 grant and owner provenance select v30.
                assert_eq!(protocol, "native-continuation-v1");
                assert_eq!(bound, grant);
                let sibling_grant = fs::read_to_string(gate.join("native-sibling-grant")).unwrap();
                assert_ne!(sibling_grant, grant);
                assert_eq!(
                    state
                        .query_row::<i64, _, _>(
                            "SELECT count(*) FROM completion_native_grant_binding",
                            [],
                            |row| row.get(0)
                        )
                        .unwrap(),
                    2
                );
                assert_eq!(
                    state
                        .query_row::<i64, _, _>(
                            "SELECT count(*) FROM completion_native_worker_attach",
                            [],
                            |row| row.get(0)
                        )
                        .unwrap(),
                    0
                );
                assert!(!gate.join("native-effect").exists(), "effect before K");
                fs::write(gate.join("native-k"), b"yes").unwrap();
                eventually(|| {
                    gate.join("native-attached").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("native-attached").exists(),
                    "native attach: entry={} broker={}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                );
                assert_eq!(
                    fs::read_to_string(gate.join("native-attached")).unwrap(),
                    grant
                );
                assert!(
                    !gate.join("native-effect").exists(),
                    "effect before State-attached gate release"
                );
                let state = read_native();
                let pid1_json: String = state.query_row(
                    "SELECT pid1_identity FROM completion_native_worker_attach WHERE attempt_id=?1 AND grant_id=?2",
                    [&attempt, &grant],
                    |row| row.get(0),
                ).unwrap();
                let pid1: serde_json::Value = serde_json::from_str(&pid1_json).unwrap();
                let pid1 = pid1["pid"].as_i64().unwrap() as i32;
                assert!(pid1 > 0);
                if matches!(
                    mode.as_str(),
                    "native_pid1_loss" | "native_receipt_pid1_loss"
                ) {
                    assert_eq!(unsafe { libc::kill(pid1, libc::SIGKILL) }, 0);
                    fs::write(gate.join("native-pid1-loss"), b"yes").unwrap();
                }
                fs::write(gate.join("native-release"), b"yes").unwrap();
                eventually(|| {
                    gate.join("native-t-sent").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(gate.join("native-t-sent").exists());
                let mut provider_descendant = None;
                if native_live {
                    // The provider creates the marker before writing its
                    // physical-custody line; observe content, not just the
                    // directory entry.
                    eventually(|| {
                        fs::metadata(gate.join("native-effect"))
                            .is_ok_and(|metadata| metadata.len() > 0)
                    });
                    if mode.starts_with("native_receipt_") {
                        assert_eq!(
                            fs::read(gate.join("native-effect").with_extension("helper-entry"))
                                .unwrap(),
                            b"receipt-helper nnp=0 seccomp=0"
                        );
                    }
                    let provider = fs::read_to_string(gate.join("native-effect")).unwrap();
                    assert!(
                        provider.starts_with(
                            "nnp=0 seccomp=0 setsid=ok unshare=ok adopted=1 host_pid="
                        ),
                        "{provider}"
                    );
                    let pid: i32 = provider.trim().rsplit_once('=').unwrap().1.parse().unwrap();
                    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
                    assert!(
                        fd >= 0,
                        "provider descendant disappeared before cancellation"
                    );
                    provider_descendant = Some(unsafe { File::from_raw_fd(fd) });
                    assert_eq!(
                        fs::read_to_string(gate.join("native-effect"))
                            .unwrap()
                            .lines()
                            .count(),
                        1
                    );
                } else {
                    assert!(!gate.join("native-effect").exists());
                }
                // A completed t handler is required before killing its socket;
                // the lost reply has no authority to trigger a second K.
                eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
                if mode == "native_cancel" {
                    // The same old broker must recover pending native/State Q
                    // and physical debt with both published fresh storage and
                    // an abandoned initializer stage in its root directory.
                    FreshV30Lane::initialize_at(&broker_state).unwrap();
                    let stage =
                        broker_state.join(format!(".v30-fresh-{}", uuid::Uuid::new_v4().simple()));
                    fs::create_dir(&stage).unwrap();
                    fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
                }
                stop(&mut broker);
                assert!(
                    !socket.exists() || protocol::request_at(&socket, Operation::Classify).is_err()
                );
                let restart_log = temp.path().join("native-restart.log");
                let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                    .spawn()
                    .unwrap();
                eventually(|| {
                    protocol::request_at(&socket, Operation::Classify).is_ok()
                        || restarted.try_wait().unwrap().is_some()
                });
                assert!(
                    restarted.try_wait().unwrap().is_none(),
                    "restart: {}",
                    fs::read_to_string(&restart_log).unwrap()
                );
                fs::write(gate.join("native-restarted"), b"yes").unwrap();
                if native_live {
                    eventually(|| {
                        gate.join("native-q-pending").exists()
                            || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("native-q-pending").exists(),
                        "{}",
                        fs::read_to_string(&err).unwrap()
                    );
                    std::thread::sleep(Duration::from_secs(6));
                    assert_eq!(
                        fs::read_to_string(gate.join("native-effect"))
                            .unwrap()
                            .lines()
                            .count(),
                        1
                    );
                    assert_eq!(
                        state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM completion_native_kernel_q",
                                [],
                                |row| row.get(0)
                            )
                            .unwrap(),
                        0
                    );
                    if matches!(mode.as_str(), "native_drain" | "native_receipt_drain") {
                        fs::write(gate.join("native-drain"), b"yes").unwrap();
                        fs::write(gate.join("native-effect.release"), b"yes").unwrap();
                    }
                    fs::write(gate.join("native-cancel"), b"yes").unwrap();
                }
                eventually(|| {
                    gate.join("native-q-readback").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("native-q-readback").exists(),
                    "q: entry={} broker={} restart={}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap(),
                    fs::read_to_string(&restart_log).unwrap()
                );
                let q = fs::read_to_string(gate.join("native-q-readback")).unwrap();
                let state = read_native();
                let settled: i64 = state.query_row("SELECT count(*) FROM completion_native_kernel_q WHERE attempt_id=?1 AND grant_id=?2", [&attempt, &grant], |row| row.get(0)).unwrap();
                let incarnation: String = state.query_row(
                    "SELECT work_incarnation_id FROM completion_native_worker_attach WHERE attempt_id=?1 AND grant_id=?2",
                    [&attempt, &grant],
                    |row| row.get(0),
                ).unwrap();
                let spent: serde_json::Value = serde_json::from_slice(
                    &fs::read(broker_state.join("grants").join(format!("{grant}.json"))).unwrap(),
                )
                .unwrap();
                assert_eq!(spent["state"], "consumed");
                let sibling: serde_json::Value = serde_json::from_slice(
                    &fs::read(
                        broker_state
                            .join("grants")
                            .join(format!("{sibling_grant}.json")),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(sibling["state"], "prepared");
                assert!(!gate.join("native-sibling-effect").exists());
                assert_eq!(fs::read_dir(broker_state.join("works")).unwrap().count(), 1);
                if native_live {
                    assert!(q.starts_with("native-q-settled "), "{q}");
                    let mut exit = libc::pollfd {
                        fd: provider_descendant.as_ref().unwrap().as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    assert_eq!(unsafe { libc::poll(&mut exit, 1, 0) }, 1);
                    assert_ne!(exit.revents & libc::POLLIN, 0, "Q preceded descendant exit");
                    assert_eq!(settled, 1);
                    assert_eq!(
                        fs::read_to_string(gate.join("native-effect"))
                            .unwrap()
                            .lines()
                            .count(),
                        1
                    );
                    let terminal = fs::read(
                        broker_state
                            .join("terminals")
                            .join(format!("{incarnation}.native.json")),
                    )
                    .unwrap();
                    let physical: serde_json::Value = serde_json::from_slice(&terminal).unwrap();
                    assert_eq!(physical["physical_tree_drained"], true);
                    assert_eq!(
                        physical["cancellation_observed"],
                        matches!(mode.as_str(), "native_cancel" | "native_receipt_cancel")
                    );
                    if matches!(mode.as_str(), "native_drain" | "native_receipt_drain") {
                        assert_eq!(physical["worker_wait_status"], 0);
                    }
                    assert!(
                        physical["output"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|item| item["name"] == "launcher.stdout")
                    );
                    let digest: String = state.query_row("SELECT terminal_receipt_sha256 FROM completion_native_kernel_q WHERE attempt_id=?1", [&attempt], |row| row.get(0)).unwrap();
                    assert_eq!(digest, format!("{:x}", Sha256::digest(&terminal)));
                    let wait: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            broker_state
                                .join("terminals")
                                .join(format!("{grant}.native-pid1-wait.json")),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(wait["reaped"], true);
                } else {
                    assert!(
                        q.starts_with("native-unknown terminal-absent-after-PID1-loss "),
                        "{q}"
                    );
                    assert_eq!(settled, 0);
                    assert!(
                        !broker_state
                            .join("terminals")
                            .join(format!("{incarnation}.native.json"))
                            .exists()
                    );
                }
                let (phase, integrated): (String, i64) = state.query_row("SELECT phase,integrated FROM completion_continuation_attempt WHERE attempt_id=?1", [&attempt], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
                assert_eq!(
                    (phase.as_str(), integrated),
                    ("accepted", 0),
                    "Q must not stand in for Runner result"
                );
                assert_eq!(state.query_row::<i64, _, _>("SELECT count(*) FROM session_wake_claim WHERE session_id='native-session' AND claim_token='native-claim'", [], |row| row.get(0)).unwrap(), 1);
                assert_eq!(
                    fs::read(data.join("pid-identity.db")).unwrap(),
                    b"retired copied owner"
                );
                stop(&mut restarted);
                fs::write(gate.join("finish"), b"done").unwrap();
                eventually(|| entry.try_wait().unwrap().is_some());
                let _ = entry.wait();
                unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                return;
            }
            if mode == "held_release_driver_route" {
                eventually(|| {
                    gate.join("driver-routed").exists() || entry.try_wait().unwrap().is_some()
                });
                let attempt_id =
                    fs::read_to_string(gate.join("driver-routed")).unwrap_or_else(|_| {
                        panic!(
                            "driver route: {} broker: {}",
                            fs::read_to_string(&err).unwrap(),
                            fs::read_to_string(&broker_log).unwrap()
                        )
                    });
                let sidecar = rusqlite::Connection::open_with_flags(
                    broker_state.join("sidecar/pid-identity.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let (count, phase, revision): (i64, String, i64) = sidecar.query_row(
                    "SELECT (SELECT count(*) FROM completion_continuation_attempt),phase,revision FROM completion_continuation_attempt WHERE attempt_id=?1",
                    [&attempt_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                ).unwrap();
                assert_eq!((count, phase.as_str(), revision), (1, "accepted", 2));
                let read = protocol::StateReadSpec {
                    protocol: "broker-state-read-v1".into(),
                    source_generation: generation.clone(),
                    root_id: prepared.root_id.clone(),
                    owner_generation: prepared.owner_generation.clone(),
                    attempt_id: Some(attempt_id),
                };
                assert!(protocol::read_state_at(&socket, &read).is_err()); // entry is not guardian/driver
            }
            if mode == "held_release_exec_driver" {
                eventually(|| {
                    fs::read_to_string(&err)
                        .unwrap_or_default()
                        .contains("v30 driver bounded State repair and wake route is not available")
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    fs::read_to_string(&err).unwrap().contains(
                        "v30 driver bounded State repair and wake route is not available"
                    ),
                    "execed driver did not reach broker running-owner readback: {}",
                    fs::read_to_string(&err).unwrap()
                );
                assert_eq!(
                    fs::read(data.join("pid-identity.db")).unwrap(),
                    b"retired copied owner"
                );
                assert_eq!(
                    db.query_row::<i64, _, _>(
                        "SELECT count(*) FROM completion_continuation_attempt",
                        [],
                        |row| row.get(0)
                    )
                    .unwrap(),
                    0
                );
            }
            if mode == "held_release_guardian_death" {
                unsafe {
                    libc::kill(prepared.guardian.host_pid, libc::SIGKILL);
                }
            }
            if mode == "held_release_child_death" {
                unsafe {
                    libc::kill(prepared.joined_child.host_pid, libc::SIGKILL);
                }
            }
            if mode == "held_release_broker_death" {
                stop(&mut broker);
                let premature_restart_log = temp.path().join("released-child-restart.log");
                let mut premature_restart =
                    Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                        .stderr(Stdio::from(File::create(&premature_restart_log).unwrap()))
                        .spawn()
                        .unwrap();
                eventually(|| {
                    protocol::request_at(&socket, Operation::Classify).is_ok()
                        || premature_restart.try_wait().unwrap().is_some()
                });
                assert!(
                    premature_restart.try_wait().unwrap().is_none(),
                    "{}",
                    fs::read_to_string(&premature_restart_log).unwrap()
                );
                fs::write(gate.join("child-effect"), b"yes").unwrap();
                std::thread::sleep(Duration::from_millis(200));
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
                stop(&mut premature_restart);
            }
            if mode != "held_release_broker_death" {
                fs::write(gate.join("child-effect"), b"yes").unwrap();
            }
            if mode == "held_release"
                || mode == "held_release_lost_reply"
                || mode == "held_release_prepare_lost_reply"
                || mode == "held_release_driver_route"
            {
                eventually(|| {
                    fs::metadata(&out).unwrap().len() > 0 || entry.try_wait().unwrap().is_some()
                });
                assert_eq!(
                    fs::read_to_string(&out).unwrap(),
                    format!("OULIPOLY_KERNEL_V30_CHILD_EFFECT={}\n", evidence.release_id)
                );
            } else {
                std::thread::sleep(Duration::from_millis(200));
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            }
            if mode != "held_release_broker_death" {
                stop(&mut broker);
            }
            fs::write(gate.join("finish"), b"done").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            let _ = entry.wait();
            let restart_log = temp.path().join("released-restart.log");
            let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                .spawn()
                .unwrap();
            eventually(|| {
                protocol::request_at(&socket, Operation::Classify).is_ok()
                    || restarted.try_wait().unwrap().is_some()
            });
            assert!(restarted.try_wait().unwrap().is_none());
            let second = Command::new(&runner)
                .arg("__age319-private-held-prepared-v30")
                .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                .output()
                .unwrap();
            assert!(!second.status.success());
            assert_eq!(
                fs::read_dir(broker_state.join("entries")).unwrap().count(),
                1
            );
            stop(&mut restarted);
            unsafe {
                libc::kill(prepared.root_init.host_pid, libc::SIGKILL);
            }
            return;
        }
        stop(&mut broker);
        fs::write(gate.join("finish"), b"done").unwrap();
        eventually(|| entry.try_wait().unwrap().is_some());
        assert!(!entry.wait().unwrap().success());
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let records: Vec<_> = fs::read_dir(broker_state.join("entries"))
            .unwrap()
            .collect();
        assert_eq!(records.len(), 1);
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(records[0].as_ref().unwrap().path()).unwrap())
                .unwrap();
        assert_eq!(record["join_consumed"], true);
        assert_eq!(
            record["joined_child"]["host_pid"],
            prepared.joined_child.host_pid
        );
        let restart_log = temp.path().join("prepared-restart.log");
        let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| {
            protocol::request_at(&socket, Operation::Classify).is_ok()
                || restarted.try_wait().unwrap().is_some()
        });
        assert!(
            restarted.try_wait().unwrap().is_none(),
            "restart: {}",
            fs::read_to_string(&restart_log).unwrap()
        );
        let retained = BrokerSidecar::open_existing(
            &broker_state.join("sidecar/pid-identity.db"),
            &broker_state,
        )
        .unwrap()
        .read_exact_prepared_owner(&generation, &prepared.root_id, &prepared.owner_generation)
        .unwrap();
        assert_eq!(retained, prepared);
        assert!(
            BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap()
            .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
            .is_err()
        );
        let second = Command::new(&runner)
            .arg("__age319-private-held-prepared-v30")
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .output()
            .unwrap();
        assert!(!second.status.success());
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            1
        );
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        stop(&mut restarted);
        unsafe {
            libc::kill(prepared.root_init.host_pid, libc::SIGKILL);
        }
        return;
    }
    if let Some(_generation) = broker_generation {
        // The test executable shares UID and broker access but is not the
        // fixed Runner image. It cannot inspect the route or select a path.
        assert!(protocol::state_route_at(&socket).is_err());
        // The user-owned source is now stale and even invalid. The Runner's
        // first production preflight must still learn v30 from the live
        // broker, before parsing any copied row or reserving an entry.
        fs::write(data.join("pid-identity.db"), b"retired copied sidecar").unwrap();
        let invoke = || {
            Command::new(&runner)
                .arg("--help")
                .env("OULIPOLY_DATA_DIR", &data)
                .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                .env_remove("LD_LIBRARY_PATH")
                .output()
                .unwrap()
        };
        let first = invoke();
        assert!(!first.status.success());
        assert!(
            String::from_utf8_lossy(&first.stderr).contains("installed Runner has no v30 route"),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
        stop(&mut broker);
        let unavailable = invoke();
        assert!(!unavailable.status.success());
        assert!(
            String::from_utf8_lossy(&unavailable.stderr)
                .contains("installed broker entry gate unavailable")
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
        let restart_log = temp.path().join("state-restart.log");
        let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| {
            restarted.try_wait().unwrap().is_some()
                || protocol::request_at(&socket, Operation::Classify).is_ok()
        });
        assert!(restarted.try_wait().unwrap().is_none());
        let second = invoke();
        assert!(!second.status.success());
        assert!(
            String::from_utf8_lossy(&second.stderr).contains("installed Runner has no v30 route")
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
        stop(&mut restarted);
        return;
    }
    // The old source is a live, exact v29 island. A current Runner must refuse
    // it at read-only domain preflight, including for syntactically admitted
    // NormalCli requests, before E/J or a workload effect.
    drop(oulipoly_state::StateDb::open(&historical_data.join("state.db")).unwrap());
    let old_writer = rusqlite::Connection::open(&historical_sidecar).unwrap();
    let current_version: i64 = rusqlite::Connection::open(data.join("pid-identity.db"))
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let historical_version: i64 = old_writer
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!((current_version, historical_version), (31, 29));
    old_writer
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA wal_autocheckpoint=0;
             INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
               state_dir,meta_path,log_path,rc_path,rc)
             VALUES('old-session','fixture','old-pending','{}','2026-09-24T00:00:00Z',
               '/old','/old/meta','/old/log','/old/rc',0);",
        )
        .unwrap();
    let old_wal = historical_data.join("pid-identity.db-wal");
    let old_main_before = fs::read(&historical_sidecar).unwrap();
    let old_wal_before = fs::read(&old_wal).unwrap();
    assert!(!old_wal_before.is_empty());
    let old_pending = || -> i64 {
        old_writer
            .query_row(
                "SELECT count(*) FROM mailbox WHERE handle='old-pending'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(old_pending(), 1);
    let normal_syntax = [
        vec!["--model", "age319-missing-model", "hello"],
        vec!["--new", "local provider fixture"],
        vec!["resume", "local-fixture-session"],
    ];
    for args in normal_syntax {
        assert!(protocol::supported_entry_args(
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        ));
        let refused = Command::new(&runner)
            .args(&args)
            .env("OULIPOLY_DATA_DIR", &historical_data)
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env_remove("LD_LIBRARY_PATH")
            .output()
            .unwrap();
        assert!(!refused.status.success());
        assert!(
            String::from_utf8_lossy(&refused.stderr)
                .contains("completion domain schema lineage differs"),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
    }
    let old_help = Command::new(&runner)
        .arg("--help")
        .env("OULIPOLY_DATA_DIR", &historical_data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap();
    assert!(!old_help.status.success());
    assert!(
        String::from_utf8_lossy(&old_help.stderr)
            .contains("completion domain schema lineage differs")
    );
    assert_eq!(
        fs::read_dir(broker_state.join("entries")).unwrap().count(),
        0
    );
    assert_eq!(old_pending(), 1);
    assert_eq!(fs::read(&historical_sidecar).unwrap(), old_main_before);
    assert_eq!(fs::read(&old_wal).unwrap(), old_wal_before);
    assert_eq!(
        fs::read_dir(broker_state.join("entries")).unwrap().count(),
        0
    );
    assert_eq!(
        fs::read_dir(broker_state.join("grants")).unwrap().count(),
        0
    );
    assert!(!gate.join("provider-effect").exists());
    assert!(!gate.join("bash-effect").exists());
    old_writer
        .execute(
            "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
               state_dir,meta_path,log_path,rc_path,rc)
             VALUES('old-session','fixture','old-after-refusal','{}','2026-09-24T00:00:01Z',
               '/old','/old/meta','/old/log','/old/rc',0)",
            [],
        )
        .unwrap();
    assert_eq!(old_pending(), 1);
    assert!(!protocol::supported_entry_args(&[
        "--age319-unsupported".into()
    ]));
    let unsupported = Command::new(&runner)
        .arg("--age319-unsupported")
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap();
    assert!(!unsupported.status.success());
    assert!(String::from_utf8_lossy(&unsupported.stderr).contains("unsupported kernel CLI mode"));
    assert_eq!(
        fs::read_dir(broker_state.join("entries")).unwrap().count(),
        0
    );
    let out = temp.path().join("runner.out");
    let err = temp.path().join("runner.err");
    let args: &[&str] = if mode == "diagnostics" {
        &["diagnostics", "metrics", "--minutes", "1", "--json"]
    } else if mode == "join_only" {
        &["__age319-private-join-only-v1"]
    } else {
        &["--help"]
    };
    let mut entry = Command::new(&runner)
        .args(args)
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::from(File::create(&out).unwrap()))
        .stderr(Stdio::from(File::create(&err).unwrap()))
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    while !gate.join("ready").exists() && entry.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < until,
            "entry stalled: {} / {}",
            fs::read_to_string(&err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        gate.join("ready").exists(),
        "entry failed: {} / {}",
        fs::read_to_string(&err).unwrap(),
        fs::read_to_string(&broker_log).unwrap()
    );
    // The exact Runner is still blocked before exec, although the namespace
    // and durable one-use entry transition already exist.
    assert!(entry.try_wait().unwrap().is_none());
    assert_eq!(fs::metadata(&out).unwrap().len(), 0);
    let records: Vec<_> = fs::read_dir(broker_state.join("entries"))
        .unwrap()
        .collect();
    assert_eq!(records.len(), 1);
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(records[0].as_ref().unwrap().path()).unwrap()).unwrap();
    assert_eq!(record["domain_id"], domain);
    assert_eq!(record["join_consumed"], true);
    let root = record["root_id"].as_str().unwrap();
    let root_record: serde_json::Value =
        serde_json::from_slice(&fs::read(broker_state.join(format!("{root}.json"))).unwrap())
            .unwrap();
    let init_pid = root_record["init_host_pid"].as_i64().unwrap() as i32;
    let guardian_pid = record["guardian"]["host_pid"].as_i64().unwrap() as i32;
    let mailbox = MailboxDb::open_historical_read_only(&data.join("pid-identity.db")).unwrap();
    let owner = mailbox.completion_continuation_owner().unwrap().unwrap();
    assert_eq!(owner.guardian_identity.pid, i64::from(guardian_pid));
    assert_eq!(owner.domain_id, domain);
    assert_eq!(
        owner.supervisor_authority_id,
        record["supervisor_authority_id"].as_str().unwrap()
    );
    assert_eq!(
        mailbox
            .completion_owner_kernel_root_id(&owner.owner_generation)
            .unwrap()
            .as_deref(),
        Some(root)
    );
    assert_eq!(
        fs::read_link(format!("/proc/{guardian_pid}/ns/pid")).unwrap(),
        fs::read_link("/proc/self/ns/pid").unwrap()
    );
    let child_pid: i32 = fs::read_to_string(gate.join("ready"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(record["joined_child"]["host_pid"], child_pid);
    assert!(record["joined_child"]["starttime_ticks"].as_u64().is_some());
    assert_ne!(init_pid, child_pid);
    assert_eq!(
        fs::read_link(format!("/proc/{init_pid}/ns/pid")).unwrap(),
        fs::read_link(format!("/proc/{child_pid}/ns/pid")).unwrap()
    );
    assert_ne!(
        fs::read_link(format!("/proc/{init_pid}/ns/pid")).unwrap(),
        fs::read_link("/proc/self/ns/pid").unwrap()
    );
    if mode == "held_death" {
        // J has consumed its one use and fsynced the exact child, but the
        // broker still owns the pre-exec gate. Losing the broker must deliver
        // EOF, not an executable release or a second child.
        stop(&mut broker);
        eventually(|| entry.try_wait().unwrap().is_some());
        assert!(!entry.wait().unwrap().success());
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(records[0].as_ref().unwrap().path()).unwrap())
                .unwrap();
        assert_eq!(persisted["join_consumed"], true);
        assert_eq!(persisted["joined_child"]["host_pid"], child_pid);
        let restart_log = temp.path().join("held-restart.log");
        let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| {
            protocol::request_at(&socket, Operation::Classify).is_ok()
                || restarted.try_wait().unwrap().is_some()
        });
        assert!(
            restarted.try_wait().unwrap().is_none(),
            "held restart: {}",
            fs::read_to_string(&restart_log).unwrap()
        );
        assert!(
            protocol::request_at(&socket, Operation::ReserveEntry)
                .unwrap()
                .starts_with("error ")
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            1
        );
        stop(&mut restarted);
        unsafe {
            libc::kill(init_pid, libc::SIGKILL);
        }
        return;
    }
    fs::write(gate.join("release"), b"yes").unwrap();
    eventually(|| entry.try_wait().unwrap().is_some());
    let exit = entry.wait().unwrap();
    let entry_error = fs::read_to_string(&err).unwrap();
    assert!(
        exit.success(),
        "entry: {entry_error} broker: {}",
        fs::read_to_string(&broker_log).unwrap()
    );
    let output = fs::read_to_string(&out).unwrap();
    if mode == "diagnostics" {
        assert!(output.trim_start().starts_with('{'), "{output}");
    } else if mode == "join_only" {
        assert!(output.is_empty(), "{output}");
    } else {
        assert!(output.contains("Usage:"));
    }
    let sibling = Command::new(&runner)
        .arg("--help")
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap();
    assert!(!sibling.status.success());
    assert!(String::from_utf8_lossy(&sibling.stderr).contains("reservation refused"));
    // Possession of the root/owner IDs and writable acceptance-shaped files
    // does not let a sibling manufacture a positive guardian grant.
    let forged_state = temp.path().join("forged-work");
    fs::create_dir(&forged_state).unwrap();
    let forged_intent = forged_state.join("root-work-intent-v1.json");
    let forged_acceptance = forged_state.join("root-work-accepted-v1.json");
    fs::write(&forged_intent, b"{}").unwrap();
    fs::write(&forged_acceptance, b"{}").unwrap();
    let image = File::open(&runner).unwrap();
    let intent = File::open(&forged_intent).unwrap();
    let accepted = File::open(&forged_acceptance).unwrap();
    let state = File::open(&forged_state).unwrap();
    let cwd = File::open(".").unwrap();
    let false_grant = protocol::prepare_accepted_work_at(
        &socket,
        &AcceptedWorkSpec {
            root_id: root.into(),
            work_id: "forged-work".into(),
            request_sha256: "0".repeat(64),
            accepted_sha256: "0".repeat(64),
            owner_generation: owner.owner_generation.clone(),
        },
        [
            image.as_raw_fd(),
            intent.as_raw_fd(),
            cwd.as_raw_fd(),
            state.as_raw_fd(),
            accepted.as_raw_fd(),
        ],
    )
    .unwrap();
    assert!(false_grant.starts_with("error "), "{false_grant}");
    assert_eq!(
        fs::read_dir(broker_state.join("grants")).unwrap().count(),
        0
    );
    // The caller is a sibling executable, not the pinned entry. The root ID
    // and complete alleged binding do not turn it into launch authority.
    let cwd = File::open(".").unwrap();
    let (_receipt, completion) = UnixStream::pair().unwrap();
    let spec = JoinSpec {
        root_id: root.into(),
        domain_id: domain,
        supervisor_id: record["supervisor_authority_id"].as_str().unwrap().into(),
        guardian_pid: record["guardian"]["host_pid"].as_i64().unwrap() as i32,
        root_authority: "{}".into(),
        args: vec!["--help".into()],
        environment: vec![],
    };
    let replay = protocol::join_at(
        &socket,
        &spec,
        [0, 1, 2, cwd.as_raw_fd(), completion.as_raw_fd()],
    )
    .unwrap();
    assert!(replay.starts_with("error "), "{replay}");
    assert!(
        protocol::request_at(&socket, Operation::ReserveEntry)
            .unwrap()
            .starts_with("error ")
    );
    stop(&mut broker);
    // Restart reads the same root PID1 and spent join record. An uncertain
    // response cannot fork another original entry.
    assert!(Path::new(&format!("/proc/{init_pid}")).exists());
    let restart_log = temp.path().join("restart.log");
    let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&restart_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| {
        protocol::request_at(&socket, Operation::Classify).is_ok()
            || restarted.try_wait().unwrap().is_some()
    });
    assert!(
        restarted.try_wait().unwrap().is_none(),
        "restart: {}",
        fs::read_to_string(&restart_log).unwrap()
    );
    assert!(
        protocol::request_at(&socket, Operation::ReserveEntry)
            .unwrap()
            .starts_with("error ")
    );
    let (wrong_socket, _other_end) = UnixStream::pair().unwrap();
    let replayed = SourceSocketWitness {
        root_id: root.into(),
        domain_id: owner.domain_id.clone(),
        supervisor_id: owner.supervisor_authority_id.clone(),
        guardian: ProcessWitness {
            host_pid: guardian_pid,
            boot_id: owner.guardian_identity.boot_id.clone(),
            starttime_ticks: u64::try_from(owner.guardian_identity.starttime_ticks).unwrap(),
        },
        source: ProcessWitness {
            host_pid: child_pid,
            boot_id: record["joined_child"]["boot_id"].as_str().unwrap().into(),
            starttime_ticks: record["joined_child"]["starttime_ticks"].as_u64().unwrap(),
        },
        scope: SourceScope::Root,
    };
    // Restart cannot turn the dead joined child's old host identity into the
    // current caller, even while its root PID1 and grant record remain live.
    assert!(
        protocol::verify_source_socket_at(&socket, &replayed, wrong_socket.as_raw_fd()).is_err()
    );
    stop(&mut restarted);
    assert!(
        protocol::verify_source_socket_at(&socket, &replayed, wrong_socket.as_raw_fd()).is_err()
    );
    // Explicit fixture teardown does not claim a production drain or safe
    // registry retirement protocol.
    unsafe {
        libc::kill(init_pid, libc::SIGKILL);
    }
}

fn raw_fresh_id_request(socket: &Path, opcode: u8, id: uuid::Uuid) -> String {
    use std::io::{Read, Write};
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge).unwrap();
    let mut frame = vec![opcode];
    frame.extend_from_slice(&challenge);
    frame.extend_from_slice(id.as_bytes());
    stream.write_all(&frame).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

#[test]
fn original_runner_joins_once_behind_persistent_root_pid1() {
    if std::env::var_os("AGE319_PRIVATE_JOIN_INNER").is_some() {
        inner();
        return;
    }
    if std::env::var_os("OULIPOLY_AGE319_RUNNER_IMAGE").is_none() {
        return;
    }
    for mode in [
        "help",
        "diagnostics",
        "join_only",
        "held_death",
        "broker_state",
        "held_prepared",
        "held_guardian_death",
        "held_release",
        "held_release_broker_death",
        "held_release_guardian_death",
        "held_release_child_death",
        "held_release_lost_reply",
        "held_release_prepare_lost_reply",
        "held_release_driver_route",
        "held_release_exec_driver",
        "held_release_commit_fail",
        "held_release_child_predeath",
        "held_release_gate_fail",
        "native_cancel",
        "native_drain",
        "native_receipt_cancel",
        "native_receipt_drain",
        "native_pid1_loss",
        "native_receipt_pid1_loss",
        "normal_release",
        "normal_handoff",
        "normal_handoff_bash_child",
        "normal_handoff_fsync",
        "normal_handoff_effect_reply_loss",
        "normal_help",
        "normal_model_held",
        "normal_model_provider",
        "normal_model_provider_reply_loss",
        "normal_model_provider_q_reply_loss",
        "normal_model_provider_restart",
        "normal_model_provider_bad_config",
        "normal_model_provider_unsupported",
        "normal_model_provider_v3_closed",
        "normal_model_provider_v3_quota",
        "normal_model_provider_v3_quota_reply_loss",
        "normal_model_provider_v3_quota_post_k",
        "normal_model_provider_v3_quota_restart",
        "normal_model_provider_v3_quota_invalid",
        "normal_model_provider_v3_quota_full",
        "normal_model_provider_v3_quota_stale",
        "normal_model_provider_v3_quota_auth",
        "normal_model_provider_v3_quota_auth_reply_loss",
        "normal_model_provider_v3_quota_auth_post_k",
        "normal_model_provider_v3_quota_auth_restart",
        "normal_model_provider_v3_quota_auth_failed",
        "normal_model_provider_v3_quota_auth_shared",
        "normal_model_provider_v3_quota_route",
        "normal_model_provider_v3_quota_route_reply_loss",
        "normal_model_provider_v3_quota_route_invalid",
        "normal_model_provider_v3_quota_route_full",
        "normal_model_provider_v3_quota_route_stale",
        "normal_model_provider_v3_quota_route_auth_failed",
        "normal_model_provider_v3_quota_route_manual_healthy",
        "normal_model_provider_v3_quota_route_manual_invalid",
        "normal_model_provider_v3_quota_route_manual_full",
        "normal_model_provider_v3_quota_route_manual_failed",
        "normal_model_provider_v3_quota_route_manual_cross_model",
        "normal_model_provider_v3_quota_route_manual_refresh",
        "normal_model_provider_v3_quota_route_manual_reply_loss",
        "normal_model_provider_v3_quota_route_manual_route_reply_loss",
        "normal_model_provider_v3_quota_route_manual_pending",
        "normal_model_provider_v3_quota_route_manual_unknown",
        "normal_model_provider_v3_quota_route_manual_physical",
        "normal_model_provider_v3_quota_route_manual_physical_cross_model",
        "normal_model_provider_v3_quota_route_manual_physical_refresh",
        "normal_model_provider_v3_quota_route_manual_physical_reply_loss",
        "normal_model_provider_v3_quota_route_manual_physical_q_reply_loss",
        "normal_model_provider_v3_quota_route_manual_physical_restart",
        "normal_model_provider_v3_quota_route_manual_physical_manual_reply_loss",
        "normal_model_provider_v3_quota_route_manual_physical_route_reply_loss",
        "normal_model_provider_v3_quota_route_manual_physical_capacity",
        "normal_model_provider_v3_quota_route_manual_physical_account_quota",
        "normal_model_provider_v3_quota_route_manual_physical_bad_plan",
        "normal_model_provider_v3_quota_route_manual_physical_bad_actor",
        "normal_model_provider_v3_quota_route_manual_physical_source_changed",
        "normal_model_provider_v3_quota_route_physical",
        "normal_model_provider_v3_quota_route_physical_reply_loss",
        "normal_model_provider_v3_quota_route_physical_post_k",
        "normal_model_provider_v3_quota_route_physical_restart",
        "normal_model_provider_v3_quota_route_physical_q_reply_loss",
        "normal_model_provider_v3_quota_route_physical_nonzero",
        "normal_model_provider_v3_quota_route_physical_capacity",
        "normal_model_provider_v3_quota_route_physical_account_quota",
        "normal_model_provider_v3_quota_route_physical_bad_plan",
        "normal_model_provider_v3_quota_route_physical_bad_actor",
        "normal_model_provider_v3_quota_route_physical_source_changed",
        "normal_model_provider_v3_quota_route_physical_shared",
        "normal_model_provider_v3_quota_route_physical_shared_manual",
        "normal_model_provider_v3_quota_route_physical_shared_command_changed",
        "normal_model_provider_v3_quota_route_physical_shared_env_changed",
        "normal_model_provider_v3_quota_route_physical_shared_config_changed",
        "normal_model_provider_v3_quota_route_physical_shared_account_changed",
        "normal_model_provider_v3_quota_route_physical_shared_reply_loss",
        "normal_model_provider_v3_quota_route_physical_shared_restart",
        "normal_model_provider_v3_quota_route_physical_shared_pending",
        "normal_model_provider_quota",
        "normal_model_provider_auth",
        "normal_model_provider_auth_recovery",
        "normal_model_provider_auth_after_healthy",
        "normal_model_provider_auth_success",
        "normal_model_provider_auth_reply_loss",
        "normal_model_provider_auth_restart",
        "normal_model_provider_quota_available",
        "normal_model_provider_quota_reply_loss",
        "normal_model_provider_quota_restart",
        "normal_model_provider_no_pin",
        "normal_guardian_death",
        "normal_driver_death",
        "normal_broker_death",
        "normal_prepare_lost_reply",
        "normal_release_lost_reply",
        "normal_repair_lost_reply",
        "normal_source_grant_lost_reply",
        "normal_recipient",
        "normal_recipient_driver_post",
        "normal_recipient_broker_post",
        "normal_empty",
        "normal_guardian_post",
        "normal_driver_post",
        "normal_broker_post",
    ]
    .into_iter()
    .chain(
        std::iter::once("normal_model_provider_v3_quota_route_physical_shared_concurrent_red")
            .filter(|_| {
                std::env::var("AGE319_PRIVATE_JOIN_ONLY_MODE")
                    .ok()
                    .as_deref()
                    == Some("normal_model_provider_v3_quota_route_physical_shared_concurrent_red")
            }),
    ) {
        if std::env::var("AGE319_PRIVATE_JOIN_ONLY_MODE")
            .ok()
            .is_some_and(|only| only != mode)
        {
            continue;
        }
        let output = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "original_runner_joins_once_behind_persistent_root_pid1",
                "--nocapture",
            ])
            .env("AGE319_PRIVATE_JOIN_INNER", "1")
            .env("AGE319_PRIVATE_JOIN_MODE", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        eprintln!("private root mode passed: {mode}");
    }
}

#[test]
fn genuine_bash_source_v30_is_one_use_and_not_accepted_without_commit_custody() {
    if std::env::var_os("AGE319_PRIVATE_JOIN_INNER").is_some() {
        inner();
        return;
    }
    let Some(mode) = std::env::var("AGE319_PRIVATE_BASH_MODE").ok() else {
        return;
    };
    assert!(matches!(
        mode.as_str(),
        "normal_bash_source"
            | "normal_bash_source_lost_reply"
            | "normal_bash_source_nonzero"
            | "normal_bash_source_capture_io_failure"
    ));
    assert!(std::env::var_os("OULIPOLY_AGE319_RUNNER_IMAGE").is_some());
    assert!(std::env::var_os("AGE319_PRIVATE_BASH_REGISTRATION").is_some());
    assert!(std::env::var_os("AGE319_PRIVATE_BASH_STATE_DB").is_some());
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "genuine_bash_source_v30_is_one_use_and_not_accepted_without_commit_custody",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_JOIN_INNER", "1")
        .env("AGE319_PRIVATE_JOIN_MODE", &mode)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{mode}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
