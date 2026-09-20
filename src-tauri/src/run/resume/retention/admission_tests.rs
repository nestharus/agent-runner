//! Independent intent: root cycle8 admission decision. These call the actual
//! outer resume and explicit staged settlement CLI, not only a claim helper.
use super::*;
use crate::run::resume::{execution::PreparedHeadlessResumeExecution, orchestration};
use oulipoly_runtime::services::*;
use oulipoly_state::{InvocationMutationAuthority, InvocationStart, ProviderSessionBinding};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const OLD: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const NEW: &str = "730ba99f-8689-429e-8305-59d0b1e4f5a5";
const OTHER: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

struct Migration(AtomicUsize);
impl MigrationServicePort for Migration {
    fn migrate(
        &self,
        request: MigrationServiceRequest<'_>,
    ) -> Result<MigrationServiceOutput, ServiceError> {
        assert_eq!(request.manual_target, Some("target"));
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ServiceError::Dependency {
            message: "fixture external rotation callback reached; no provider launched".into(),
        })
    }
}
struct NoExecutor(AtomicUsize);
impl ExecutorServicePort for NoExecutor {
    fn execute(&self, _: ExecutorServiceRequest) -> Result<ExecutorServiceOutput, ServiceError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("ordinary admission must not execute provider")
    }
    fn execute_with_live_session_authority(
        &self,
        request: ExecutorServiceRequest,
        _: LiveSessionAuthorityTarget,
    ) -> Result<ExecutorServiceOutput, ServiceError> {
        self.execute(request)
    }
}

fn run_case(mode: &str) {
    let root = PathBuf::from(std::env::var_os("OULIPOLY_DATA_DIR").unwrap());
    let state = StateDb::open(&root.join("state.db")).unwrap();
    let mut mailbox = MailboxDb::open(&root.join("pid-identity.db")).unwrap();
    let uuid = uuid::Uuid::new_v4().to_string();
    let row = state
        .start_invocation(&InvocationStart {
            invocation_uuid: uuid.clone(),
            model_name: "fixture".into(),
            provider_name: "fixture".into(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .bind_invocation_provider_session_start(
            InvocationMutationAuthority::Standalone,
            row,
            &ProviderSessionBinding {
                provider_session_id: OLD.into(),
                capture_method: "fixture",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
    state
        .mint_imported_chain_if_absent("fixture", OLD, &chrono::Utc::now(), "fixture")
        .unwrap();
    let chain = state.chain_id_for_segment("fixture", OLD).unwrap().unwrap();
    let sql = rusqlite::Connection::open(root.join("pid-identity.db")).unwrap();
    // Positively dead fixture PID, not native drain and not missing claim.
    sql.execute("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,wake_invocation_uuid,wake_pid,reason,auto_wake_count) VALUES (?1,'original-claim','2000-01-01T00:00:00Z',?2,?3,'fixture',1)", rusqlite::params![OLD,uuid,i64::MAX]).unwrap();
    let bytes = b"original\0\xffcomplete\n";
    let spool = ExecutionOutputSpool::from_complete_bytes(bytes, b"retained stderr").unwrap();
    spool.persist_for_invocation(&state, row, &uuid).unwrap();
    let paths = state
        .invocation_output_artifact_paths(&uuid)
        .unwrap()
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for p in [&paths.stdout, &paths.stderr] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let summary = spool.summary().unwrap();
    let refs = ["selected-first", "selected-second"]
        .into_iter()
        .map(|name| oulipoly_agent_messenger::ReturnedArtifactRef {
            version_id: format!("store://return/{uuid}/{name}/1"),
            name: name.into(),
            store_address: oulipoly_agent_messenger::StoreAddress {
                workflow_run_id: format!("return:{uuid}"),
                artifact_name: name.into(),
                version: 1,
            },
            sha256: "a".repeat(64),
            content_len: 3,
            format_hint: None,
            verdict_line: None,
            source: oulipoly_agent_messenger::ReturnedArtifactSource::InlineBytes,
            producer_invocation_uuid: uuid.parse().unwrap(),
            returned_at: chrono::Utc::now(),
        })
        .collect::<Vec<_>>();
    let effects = CompletedTurnEffects {
        invocation_row_id: row,
        delivery_ids: vec![],
        session_id: OLD.into(),
        turn_generation_id: uuid.clone(),
        submitted_evidence: None,
        confirmed_evidence: None,
        observed_at: 0,
        returned_artifacts: refs.clone(),
        resume_acceptance_status: None,
        resume_acceptance_evidence: None,
        success: true,
        exit_code: 0,
        error_category: None,
        terminal_reason: Some("completed".into()),
    };
    // Genuine no-input completion: no ACK, prompt proof or native receipt fabricated.
    let context = Context {
        version: 1,
        provider_session: OLD.into(),
        mailbox_session: OLD.into(),
        chain: chain.clone(),
        seqs: vec![],
        nonce: None,
        prompt_attestation: None,
        observed_turn: None,
        original_wake_claim: Some("original-claim".into()),
        sidecar_generation: Some(mailbox.sidecar_generation().unwrap()),
        stdout: Body {
            path: paths.stdout.clone(),
            len: summary.stdout_bytes,
            sha256: summary.stdout_sha256,
        },
        stderr: Body {
            path: paths.stderr.clone(),
            len: summary.stderr_bytes,
            sha256: summary.stderr_sha256,
        },
        classification: serde_json::json!({"terminal_completion_confirmed":true}),
    };
    let payload = serde_json::to_value(&context).unwrap();
    let pending = !matches!(mode, "no-pending" | "repl-no-pending");
    if pending {
        state
            .retain_completed_turn_selection(InvocationMutationAuthority::Standalone, row, &refs)
            .unwrap();
        state
            .admit_completed_turn(InvocationMutationAuthority::Standalone, &effects, &payload)
            .unwrap();
    }
    if mode == "pending-next" {
        mailbox
            .enqueue_agent_bash_complete(&oulipoly_state::mailbox::AgentBashCompleteEnqueue {
                session_id: OLD,
                handle: "distinct-next-work",
                payload_json: r#"{"kind":"distinct-next-work"}"#,
                owner_invocation_uuid: Some("next-owner"),
                matched_os_pid: Some(1),
                matched_os_boot_id: Some("fixture-boot"),
                matched_os_pid_starttime_ticks: Some(1),
                matched_chain_index: Some(0),
                state_dir: "/private/next-state",
                meta_path: "/private/next-meta",
                log_path: "/private/next-log",
                rc_path: "/private/next-rc",
                rc: 0,
            })
            .unwrap();
    }
    let before = state.completed_turn(&uuid).unwrap();
    let session = match mode {
        "retargeted" | "duplicate-other" | "duplicate-original" => {
            state
                .rotate_chain_segment_transactionally(oulipoly_state::ChainSegmentRotationInput {
                    chain_id: &chain,
                    source_provider_name: "fixture",
                    source_session_id: OLD,
                    target_provider_name: if mode.starts_with("duplicate-") {
                        "target"
                    } else {
                        "fixture"
                    },
                    target_session_id: NEW,
                    changed_at: &chrono::Utc::now(),
                    reason: oulipoly_core::TransitionReason::Manual,
                })
                .unwrap();
            NEW
        }
        "repl-target-conflict" => {
            state
                .rotate_chain_segment_transactionally(oulipoly_state::ChainSegmentRotationInput {
                    chain_id: &chain,
                    source_provider_name: "fixture",
                    source_session_id: OLD,
                    target_provider_name: "target",
                    target_session_id: NEW,
                    changed_at: &chrono::Utc::now(),
                    reason: oulipoly_core::TransitionReason::Manual,
                })
                .unwrap();
            OLD
        }
        "unrelated" => OTHER,
        _ => OLD,
    };
    let selected_chain = if mode == "unrelated" {
        state
            .mint_imported_chain_if_absent("fixture", OTHER, &chrono::Utc::now(), "fixture")
            .unwrap();
        state
            .chain_id_for_segment("fixture", OTHER)
            .unwrap()
            .unwrap()
    } else if mode.starts_with("duplicate-") || mode == "repl-target-conflict" {
        state
            .mint_imported_chain_if_absent("distinct", session, &chrono::Utc::now(), "fixture")
            .unwrap();
        let other = state
            .chain_id_for_segment("distinct", session)
            .unwrap()
            .unwrap();
        assert_ne!(other, chain);
        if matches!(
            mode,
            "duplicate-other" | "duplicate-current-other" | "repl-target-conflict"
        ) {
            other
        } else {
            chain.clone()
        }
    } else {
        chain.clone()
    };
    let cfg = root.join("config");
    std::fs::create_dir_all(cfg.join("models")).unwrap();
    // All storage and configs belong to this env-cleared fixture. Commands are
    // launch tripwires, not installed providers. Named migration is compatible.
    let provider_tripwire = root.join("provider-tripwire");
    std::fs::write(
        &provider_tripwire,
        format!(
            "#!/bin/sh\nprintf launched > '{}'\nexit 91\n",
            root.join("provider-launched").display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&provider_tripwire, std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    let provider_text = ["fixture", "target", "distinct"].into_iter().map(|name| format!(
        "[{name}]\ncommand = '{}'\n[{name}.resume]\nkind = 'flag'\nflag = '--resume'\n[{name}.session_storage]\nkind = 'claude_code'\nprojects_dir = '{}'\n",
        provider_tripwire.display(),
        root.join(name).display()
    )).collect::<String>();
    std::fs::write(cfg.join("providers.toml"), &provider_text).unwrap();
    let providers = oulipoly_config::ProvidersConfig::load(&cfg.join("providers.toml")).unwrap();
    std::fs::write(cfg.join("models/fixture.toml"), "[[providers]]\nname = 'fixture'\n[[providers]]\nname = 'target'\n[[providers]]\nname = 'distinct'\n").unwrap();
    let models = oulipoly_config::load_models(&cfg.join("models"), Some(&providers)).unwrap();
    // Construct the service registry without endpoint discovery; actual resume
    // resolution stays production. Migration/execution below use explicit ports.
    std::fs::write(cfg.join("providers.toml"), "").unwrap();
    let mut services =
        crate::wiring::AgentRuntimeServices::production(crate::wiring::RuntimePaths {
            config_root: cfg.clone(),
            models_dir: cfg.join("models"),
            agents_dir: cfg.join("agents"),
            data_root: root.clone(),
            state_db_path: state.path().into(),
            lock_dir: root.join("locks"),
            working_dir: root.clone(),
        })
        .unwrap();
    let ResumeServiceOutput::ResumeResolved { resolved } = services
        .resume_service
        .resolve_resume(ResumeServiceRequest {
            state: &state,
            models: &models,
            providers_cfg: &providers,
            input: &selected_chain,
            model_override: None,
        })
        .unwrap()
    else {
        panic!("exact fixture chain must resolve")
    };
    assert_eq!(resolved.chain_id, selected_chain);
    assert_eq!(resolved.active_session_id, session);
    if mode.starts_with("duplicate-") || mode == "repl-target-conflict" {
        assert_eq!(
            resolved.active_provider,
            if matches!(
                mode,
                "duplicate-other" | "duplicate-current-other" | "repl-target-conflict"
            ) {
                "distinct"
            } else {
                "target"
            }
        );
    }
    let migration = Arc::new(Migration(AtomicUsize::new(0)));
    services.migration_service = migration.clone();
    let executor = Arc::new(NoExecutor(AtomicUsize::new(0)));
    services.executor_service = executor.clone();
    let mut prepared = PreparedHeadlessResumeExecution {
        _mailbox_finalization_guard: None,
        original_answer: None,
        submission_token: None,
        target_kind: oulipoly_state::InboxTargetKind::Session,
        target_id: session.into(),
        answer: None,
        mailbox_session_id: session.into(),
        mailbox_delivery_seqs: vec![],
        mailbox_delivery_nonce: None,
        mailbox_delivery_requires_turn_confirmation: false,
        env: crate::migration_providers::ResumeExecutionEnvironment {
            state,
            providers_cfg: providers,
            models,
            sessions_cfg: Default::default(),
            config_root: cfg.clone(),
            models_dir: cfg.join("models"),
        },
        resolved,
        effective_spawn_cwd: root.clone(),
        parent_invocation_id: None,
        max_attempts: 1,
        provider_prompt_accepted: false,
    };
    drop(sql);
    drop(mailbox);
    if mode.starts_with("repl-") {
        std::fs::write(cfg.join("providers.toml"), provider_text).unwrap();
        run_repl_case(
            mode,
            &mut services,
            &prepared.env.state,
            &cfg,
            &root,
            &selected_chain,
            &uuid,
        );
        assert_eq!(executor.0.load(Ordering::SeqCst), 0);
        return;
    }
    let error = orchestration::run_prepared_resume(
        &services,
        &mut prepared,
        None,
        Some("target"),
        &selected_chain,
        Some(&root),
    )
    .unwrap_err();
    let blocked = matches!(
        mode,
        "pending" | "pending-next" | "retargeted" | "duplicate-original"
    );
    let shared_claim_collision = mode == "duplicate-current-other";
    assert_eq!(
        migration.0.load(Ordering::SeqCst),
        usize::from(!blocked && !shared_claim_collision),
        "{mode}: {error}"
    );
    assert_eq!(executor.0.load(Ordering::SeqCst), 0);
    let mailbox = MailboxDb::open(&root.join("pid-identity.db")).unwrap();
    if blocked {
        assert!(error.contains("completed_turn_pending"), "{error}");
        // Also enter the public launch admission directly: its destructive
        // coordinate path must be fenced independently of outer preflight.
        let queue_error = crate::wake_coordinator::admit_resolved_session_launch(
            &uuid::Uuid::new_v4().to_string(),
            &prepared.resolved,
        )
        .err()
        .expect("pending original duty must refuse queue admission");
        assert!(
            queue_error.contains("completed_turn_pending"),
            "{queue_error}"
        );
        assert_eq!(
            mailbox
                .wake_session_reader()
                .wake_claim(OLD)
                .unwrap()
                .unwrap()
                .claim_token,
            "original-claim"
        );
        let record = prepared.env.state.completed_turn(&uuid).unwrap().unwrap();
        assert_eq!(record.effects, before.as_ref().unwrap().effects);
        assert_eq!(record.context, before.as_ref().unwrap().context);
        assert_eq!(std::fs::read(&paths.stdout).unwrap(), bytes);
        drop(mailbox);
        // Real explicit consumer, separate process, no production config/provider.
        let output = std::process::Command::new(
            std::env::var_os("AGE360_ADMISSION_BUILT_RUNNER")
                .expect("fixture parent builds the candidate"),
        )
        .args(["completed-turn", "--invocation", &uuid, "--settle"])
        .output()
        .unwrap();
        assert!(output.status.success(), "{output:?}");
        println!(
            "settlement CLI stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let replay =
            std::process::Command::new(std::env::var_os("AGE360_ADMISSION_BUILT_RUNNER").unwrap())
                .args([
                    "completed-turn",
                    "--invocation",
                    &uuid,
                    "--settle",
                    "--output",
                ])
                .output()
                .unwrap();
        assert!(replay.status.success(), "{replay:?}");
        assert_eq!(replay.stdout, bytes);
        println!(
            "exact settlement replay returned original complete binary body, {} bytes",
            replay.stdout.len()
        );

        let after = prepared.env.state.completed_turn(&uuid).unwrap().unwrap();
        assert!(after.committed);
        assert_eq!(after.effects, record.effects);
        assert_eq!(after.context, record.context);
        assert_eq!(
            prepared.env.state.list_returned_artifacts(row).unwrap(),
            refs
        );
        assert_eq!(std::fs::read(paths.stdout).unwrap(), bytes);
        assert_eq!(after.tails["idle"], "complete");
        assert!(
            !root.join("provider-launched").exists(),
            "settlement-only selector executed a provider"
        );
        if mode == "pending-next" {
            let mailbox = MailboxDb::open(&root.join("pid-identity.db")).unwrap();
            let pending = mailbox.list_pending(OLD).unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].handle, "distinct-next-work");
            assert_eq!(after.tails["wake"], "distinct_next_turn_pending");
            assert_eq!(after.tails["pending_count"], 1);
            assert_eq!(after.tails["wake_owner"], "root/operator");
            assert_eq!(
                after.tails["wake_action"],
                "separate authorized session advance; recovery never launches"
            );
            assert!(
                mailbox
                    .wake_session_reader()
                    .wake_claim(OLD)
                    .unwrap()
                    .is_none()
            );
            assert!(
                mailbox
                    .continuation_activation(OLD, "original-claim")
                    .unwrap()
                    .is_none()
            );
        }
        println!(
            "outer refusal preserved exact retained turn; staged settlement-only recovery completed: {mode}"
        );
    } else {
        if !shared_claim_collision {
            assert!(error.contains("resume migration failed"), "{error}");
        } else {
            assert!(error.contains("completed_turn_claim_pending"), "{error}");
        }
        if matches!(
            mode,
            "unrelated" | "duplicate-other" | "duplicate-current-other"
        ) {
            let exact = mailbox
                .wake_session_reader()
                .wake_claim(OLD)
                .unwrap()
                .expect("another resolved chain deleted the original retained claim");
            assert_eq!(exact.claim_token, "original-claim", "{mode}: {error}");
            assert_eq!(
                exact.wake_invocation_uuid.as_deref(),
                Some(uuid.as_str()),
                "{mode}: retained authority owner changed; {error}"
            );
        } else {
            assert!(
                mailbox
                    .wake_session_reader()
                    .wake_claim(OLD)
                    .unwrap()
                    .is_none()
            );
        }
        if mode == "duplicate-current-other" {
            let queue = rusqlite::Connection::open(root.join("pid-identity.db")).unwrap();
            let states = queue
                .prepare("SELECT state FROM session_admission_queue ORDER BY queue_sequence")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(
                states,
                vec!["cancelled"],
                "bounded refusal must release only this unmaterialized queue reservation"
            );
            println!(
                "actual shared-claim collision refused before migration and preserved exact original token/owner"
            );
        } else {
            println!(
                "ordinary admission reached selected migration callback without provider: {mode}"
            );
        }
    }
}

fn isolated_case(mode: &str, node: &str) {
    const MODE: &str = "AGE360_ADMISSION_CASE";
    if let Ok(mode) = std::env::var(MODE) {
        run_case(&mode);
        return;
    }
    {
        let root = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env(MODE, mode)
            .env("AGE360_ADMISSION_BUILT_RUNNER", candidate_runner());
        for (key, sub) in [
            ("HOME", "home"),
            ("CODEX_HOME", "codex"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_RUNTIME_DIR", "runtime"),
            ("OULIPOLY_DATA_DIR", "runner-data"),
            ("TMPDIR", "tmp"),
        ] {
            let path = root.path().join(sub);
            std::fs::create_dir_all(&path).unwrap();
            child.env(key, path);
        }
        let output = child
            .args(["--exact", node, "--nocapture"])
            .output()
            .unwrap();
        println!(
            "case={mode} stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{mode}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed; 0 failed")
        );
    }
}

#[test]
fn retained_turn_outer_manual_admission_and_recovery() {
    isolated_case(
        "pending",
        "run::resume::retention::admission_tests::retained_turn_outer_manual_admission_and_recovery",
    );
}
#[test]
fn retained_turn_recovery_with_nonzero_next_work_never_launches_provider_or_wake() {
    isolated_case(
        "pending-next",
        "run::resume::retention::admission_tests::retained_turn_recovery_with_nonzero_next_work_never_launches_provider_or_wake",
    );
}

#[test]
fn retained_turn_outer_retargeted_chain_preserves_original() {
    isolated_case(
        "retargeted",
        "run::resume::retention::admission_tests::retained_turn_outer_retargeted_chain_preserves_original",
    );
}
#[test]
fn retained_turn_outer_unrelated_session_keeps_normal_admission() {
    isolated_case(
        "unrelated",
        "run::resume::retention::admission_tests::retained_turn_outer_unrelated_session_keeps_normal_admission",
    );
}
#[test]
fn retained_turn_outer_no_pending_keeps_normal_admission() {
    isolated_case(
        "no-pending",
        "run::resume::retention::admission_tests::retained_turn_outer_no_pending_keeps_normal_admission",
    );
}

// Unit harnesses do not receive CARGO_BIN_EXE (integration tests do). Ask
// Cargo for this checkout's ordinary binary artifact once per harness, never
// select an installed executable or require an external fixture variable.
fn candidate_runner() -> &'static PathBuf {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let output = std::process::Command::new(env!("CARGO"))
            .args(["build", "--offline", "--locked", "--manifest-path"])
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .args(["--bin", "oulipoly-agent-runner", "--message-format=json"])
            .output()
            .expect("build the current checkout's runner fixture");
        assert!(
            output.status.success(),
            "candidate build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let executable = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter_map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).ok()?;
                (value["reason"] == "compiler-artifact"
                    && value["target"]["name"] == "oulipoly-agent-runner"
                    && value["profile"]["test"] == false)
                    .then(|| value["executable"].as_str().map(PathBuf::from))
                    .flatten()
            })
            .next_back()
            .expect("Cargo must report the candidate binary");
        assert!(executable.is_file());
        println!(
            "candidate runner selected from Cargo artifact: {}",
            executable.display()
        );
        executable
    })
}

#[test]
fn retained_turn_distinct_provider_same_native_session_proceeds() {
    isolated_case(
        "duplicate-other",
        "run::resume::retention::admission_tests::retained_turn_distinct_provider_same_native_session_proceeds",
    );
}
#[test]
fn retained_turn_original_chain_same_native_session_refuses() {
    isolated_case(
        "duplicate-original",
        "run::resume::retention::admission_tests::retained_turn_original_chain_same_native_session_refuses",
    );
}
#[test]
fn retained_turn_interactive_refuses_before_named_migration() {
    isolated_case(
        "repl-pending",
        "run::resume::retention::admission_tests::retained_turn_interactive_refuses_before_named_migration",
    );
}
#[test]
fn retained_turn_interactive_no_pending_named_migration_control() {
    isolated_case(
        "repl-no-pending",
        "run::resume::retention::admission_tests::retained_turn_interactive_no_pending_named_migration_control",
    );
}

#[test]
fn retained_turn_interactive_refuses_conflicting_materialized_target_before_effects() {
    isolated_case(
        "repl-target-conflict",
        "run::resume::retention::admission_tests::retained_turn_interactive_refuses_conflicting_materialized_target_before_effects",
    );
}

struct NamedBuiltInMigration {
    calls: AtomicUsize,
    expected_target: &'static str,
    expect_materialized: bool,
}
impl MigrationServicePort for NamedBuiltInMigration {
    fn migrate(
        &self,
        request: MigrationServiceRequest<'_>,
    ) -> Result<MigrationServiceOutput, ServiceError> {
        assert_eq!(request.manual_target, Some(self.expected_target));
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = ProductionMigrationService::default().migrate(request);
        if !self.expect_materialized {
            let error = result.expect_err("conflicting named target must be refused");
            assert!(
                error.to_string().contains("completed_turn_target_pending"),
                "{error}"
            );
            return Err(error);
        }
        let result = result?;
        assert!(
            matches!(result, MigrationServiceOutput::Migrated { .. }),
            "{result:?}"
        );
        // End the no-pending control after actual transcript/segment effects,
        // before invocation/provider execution. This is not a fake migration.
        Err(ServiceError::Dependency {
            message: "fixture named migration materialized; stop before provider".into(),
        })
    }
}

fn run_repl_case(
    mode: &str,
    services: &mut crate::wiring::AgentRuntimeServices,
    state: &StateDb,
    cfg: &Path,
    root: &Path,
    chain: &str,
    uuid: &str,
) {
    let target_conflict = mode == "repl-target-conflict";
    let source_provider = if target_conflict {
        "distinct"
    } else {
        "fixture"
    };
    let target_provider = if target_conflict { "fixture" } else { "target" };
    let project: String = root
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let source = root
        .join(source_provider)
        .join(&project)
        .join(format!("{OLD}.jsonl"));
    let target = root
        .join(target_provider)
        .join(&project)
        .join(format!("{OLD}.jsonl"));
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    let transcript = format!(
        "{{\"uuid\":\"turn-1\",\"sessionId\":\"{OLD}\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"type\":\"assistant\"}}"
    );
    std::fs::write(&source, &transcript).unwrap();
    let original_target = target_conflict.then(|| b"retained-target-preimage\n".to_vec());
    if let Some(bytes) = &original_target {
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, bytes).unwrap();
    } else {
        assert!(!target.exists());
    }
    let migration = Arc::new(NamedBuiltInMigration {
        calls: AtomicUsize::new(0),
        expected_target: target_provider,
        expect_materialized: !target_conflict,
    });
    services.migration_service = migration.clone();
    let sql = rusqlite::Connection::open(state.path()).unwrap();
    let segments = |conn: &rusqlite::Connection| {
        conn.prepare("SELECT provider_name,session_id,ended_at FROM session_chain_segments WHERE chain_id=?1 ORDER BY id")
            .unwrap().query_map([chain], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,Option<String>>(2)?)))
            .unwrap().collect::<Result<Vec<_>, _>>().unwrap()
    };
    let before = segments(&sql);
    let result = crate::run::run_repl(
        services,
        Some("fixture"),
        Some(chain),
        Some(target_provider),
        Some(root),
        Some(&cfg.join("models")),
    );
    let pending = mode == "repl-pending";
    assert_eq!(
        migration.calls.load(Ordering::SeqCst),
        usize::from(!pending),
        "{result:?}"
    );
    assert_eq!(std::fs::read_to_string(&source).unwrap(), transcript);
    if pending || target_conflict {
        if target_conflict {
            assert_eq!(result.unwrap(), 1);
        } else {
            let error = result.unwrap_err();
            assert!(error.contains("completed_turn_pending"), "{error}");
        }
        if let Some(bytes) = original_target {
            assert_eq!(
                std::fs::read(&target).unwrap(),
                bytes,
                "conflicting historical target changed before refusal"
            );
        } else {
            assert!(
                !target.exists(),
                "no destination transcript may be published before refusal"
            );
        }
        assert_eq!(
            segments(&sql),
            before,
            "no source closure/target segment before refusal"
        );
        assert!(!state.completed_turn(uuid).unwrap().unwrap().committed);
        let claim = MailboxDb::open(&root.join("pid-identity.db"))
            .unwrap()
            .wake_session_reader()
            .wake_claim(OLD)
            .unwrap()
            .unwrap();
        assert_eq!(claim.claim_token, "original-claim");
        assert_eq!(claim.wake_invocation_uuid.as_deref(), Some(uuid));
    } else {
        assert_eq!(result.unwrap(), 1);
        assert_eq!(std::fs::read_to_string(target).unwrap(), transcript);
        let after = segments(&sql);
        assert_eq!(after.len(), before.len() + 1);
        assert!(after[0].2.is_some());
        assert_eq!(after.last().unwrap(), &("target".into(), OLD.into(), None));
    }
    println!(
        "actual interactive resolver/named migration: {mode}; source preserved, destination and segment effects checked"
    );
}

#[test]
fn retained_turn_distinct_chain_cannot_release_original_same_session_claim() {
    isolated_case(
        "duplicate-current-other",
        "run::resume::retention::admission_tests::retained_turn_distinct_chain_cannot_release_original_same_session_claim",
    );
}
