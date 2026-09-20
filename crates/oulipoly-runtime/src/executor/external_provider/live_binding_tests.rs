//! Synthetic State/sidecar tests of launch-owned live publication.
use super::*;

fn live_fixture(
    dir: &std::path::Path,
) -> (
    ExternalProviderDispatchContext,
    SpawnIdentityContext,
    RecordedLaunchGeneration,
    oulipoly_state::ProviderLaunchEndpoint,
) {
    let state = oulipoly_state::StateDb::open(&dir.join("state.db")).unwrap();
    let (spawn, generation) = crate::executor::cli::spawn_identity::live_binding_test_fixture(
        &dir.join("pid-identity.db"),
    );
    let row = state
        .start_invocation(&oulipoly_state::InvocationStart {
            invocation_uuid: spawn.invocation_uuid().into(),
            model_name: "test".into(),
            provider_name: "fixture".into(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    let provider = oulipoly_config::ProviderConfig::new("fixture", vec![]);
    let model = oulipoly_config::ModelConfig {
        name: "test".into(),
        prompt_mode: oulipoly_config::PromptMode::Arg,
        providers: vec![provider],
        inputs: vec![],
        provider: None,
    };
    let mut context = crate::executor::external_provider_context_from_request(
        crate::services::ExecutorServiceRequest::Facade {
            model,
            provider_index: 0,
            prompt: "test".into(),
            working_dir: None,
            models_dir: None,
            extra_inputs: Default::default(),
            parent_invocation_env: None,
        },
    )
    .unwrap();
    context.live_session_authority = Some(crate::services::LiveSessionAuthorityTarget {
        state_path: state.path().to_path_buf(),
        invocation_row_id: row,
        invocation_uuid: spawn.invocation_uuid().into(),
    });
    let recorded = recorded_launch_generation();
    remember_recorded_launch_generation(&recorded, Ok(generation)).unwrap();
    (
        context,
        spawn,
        recorded,
        oulipoly_state::ProviderLaunchEndpoint {
            endpoint_family: "fixture".into(),
            provider_instance_id: "fixture-instance".into(),
            settings_id: "fixture-settings".into(),
            endpoint_identity_sha256: "a".repeat(64),
        },
    )
}

fn live_marker(session: &str) -> DecodedLaunchEvent {
    DecodedLaunchEvent::Marker {
        seq: 1,
        name: PROVIDER_SESSION_MARKER.into(),
        value: serde_json::json!({"provider_session_id": session}),
    }
}

#[test]
fn ordinary_live_binding_commits_before_attachment_and_exact_retry() {
    let dir = tempfile::tempdir().unwrap();
    let (context, spawn, recorded, endpoint) = live_fixture(dir.path());
    let target = context.live_session_authority.clone().unwrap();
    let observer = external_launch_event_callback(
        ExecutionOutputSpool::new().unwrap(),
        Arc::new(Mutex::new(None)),
        Some(spawn),
        recorded,
        "fixture".into(),
        context,
        endpoint,
    );
    for _ in 0..2 {
        observer(&live_marker("session-a")).unwrap();
        let state = oulipoly_state::StateDb::open(&target.state_path).unwrap();
        assert_eq!(
            state
                .get_invocation_by_uuid(&target.invocation_uuid)
                .unwrap()
                .unwrap()
                .provider_session_id
                .as_deref(),
            Some("session-a")
        );
        assert_eq!(
            state
                .invocation_provider_session_authority(target.invocation_row_id)
                .unwrap()
                .unwrap()
                .settings_id,
            "fixture-settings"
        );
    }
    assert!(observer(&live_marker("conflicting-session")).is_err());
}

#[test]
fn ordinary_live_binding_rejects_wrong_stale_missing_and_failed_authority() {
    for case in [
        "wrong_account",
        "wrong_session",
        "wrong_actor",
        "stale_actor",
        "missing_target",
        "state_failure",
        "no_evidence",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (mut context, spawn, recorded, endpoint) = live_fixture(dir.path());
        let target = context.live_session_authority.clone().unwrap();
        let sql = rusqlite::Connection::open(&target.state_path).unwrap();
        match case {
            "wrong_session" => context.start_known_provider_session_id = Some("expected".into()),
            "wrong_actor" => {
                context
                    .live_session_authority
                    .as_mut()
                    .unwrap()
                    .invocation_uuid = "22222222-2222-4222-8222-222222222222".into()
            }
            "stale_actor" => {
                let state = oulipoly_state::StateDb::open(&target.state_path).unwrap();
                state
                    .finalize_invocation(
                        oulipoly_state::InvocationMutationAuthority::Standalone,
                        target.invocation_row_id,
                        true,
                        0,
                        None,
                        None,
                    )
                    .unwrap();
            }
            "missing_target" => context.live_session_authority = None,
            "state_failure" => {
                sql.execute_batch("CREATE TRIGGER fail_binding BEFORE UPDATE OF provider_session_id ON invocations BEGIN SELECT RAISE(ABORT, 'fixture persistence fault'); END;").unwrap();
            }
            _ => (),
        }
        let observer = external_launch_event_callback(
            ExecutionOutputSpool::new().unwrap(),
            Arc::new(Mutex::new(None)),
            Some(spawn),
            recorded,
            if case == "wrong_account" {
                "foreign"
            } else {
                "fixture"
            }
            .into(),
            context,
            endpoint,
        );
        let result = if case == "no_evidence" {
            observer(&DecodedLaunchEvent::Marker {
                seq: 1,
                name: "unrelated".into(),
                value: serde_json::Value::Null,
            })
        } else {
            observer(&live_marker("session-a"))
        };
        assert_eq!(result.is_ok(), case == "no_evidence", "{case}: {result:?}");
        let session: Option<String> = sql
            .query_row("SELECT provider_session_id FROM invocations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(session, None, "{case}");
        let sidecar = rusqlite::Connection::open(dir.path().join("pid-identity.db")).unwrap();
        let attached: Option<String> = sidecar
            .query_row("SELECT session_id FROM runtime_generation", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(attached, None, "{case} must not advertise attachment");
    }
}

#[test]
fn ordinary_live_binding_preserves_state_when_sidecar_fails_then_retries_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let (context, spawn, recorded, endpoint) = live_fixture(dir.path());
    let target = context.live_session_authority.clone().unwrap();
    let sidecar = rusqlite::Connection::open(dir.path().join("pid-identity.db")).unwrap();
    sidecar.execute_batch("CREATE TRIGGER fail_attach BEFORE UPDATE OF session_id ON runtime_generation BEGIN SELECT RAISE(ABORT, 'fixture attachment fault'); END;").unwrap();
    let failure = Arc::new(Mutex::new(None));
    let observer = external_launch_event_callback(
        ExecutionOutputSpool::new().unwrap(),
        failure.clone(),
        Some(spawn),
        recorded,
        "fixture".into(),
        context,
        endpoint,
    );
    assert!(observer(&live_marker("session-a")).is_err());
    assert!(failure.lock().unwrap().is_some());
    let state = oulipoly_state::StateDb::open(&target.state_path).unwrap();
    assert_eq!(
        state
            .get_invocation_by_uuid(&target.invocation_uuid)
            .unwrap()
            .unwrap()
            .provider_session_id
            .as_deref(),
        Some("session-a")
    );
    sidecar.execute_batch("DROP TRIGGER fail_attach").unwrap();
    observer(&live_marker("session-a")).unwrap();
    let attached: String = sidecar
        .query_row("SELECT session_id FROM runtime_generation", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(attached, "session-a");
    assert!(
        failure.lock().unwrap().is_some(),
        "prior failure evidence must survive retry"
    );
}

#[test]
fn ordinary_live_binding_create_vs_resume_metadata_survives_finalization() {
    use crate::services::ProviderSessionStartMode;
    for mode in [
        ProviderSessionStartMode::Create,
        ProviderSessionStartMode::Resume,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (mut context, spawn, recorded, endpoint) = live_fixture(dir.path());
        context.start_known_provider_session_id = Some("session-a".into());
        context.start_known_provider_session_mode = Some(mode);
        let target = context.live_session_authority.clone().unwrap();
        let observer = external_launch_event_callback(
            ExecutionOutputSpool::new().unwrap(),
            Arc::new(Mutex::new(None)),
            Some(spawn),
            recorded,
            "fixture".into(),
            context,
            endpoint.clone(),
        );
        observer(&live_marker("session-a")).unwrap();
        let expected_resume =
            matches!(mode, ProviderSessionStartMode::Resume).then_some("session-a");
        let state = oulipoly_state::StateDb::open(&target.state_path).unwrap();
        let read = || {
            state
                .get_invocation_by_uuid(&target.invocation_uuid)
                .unwrap()
                .unwrap()
        };
        assert_eq!(
            read().resume_input_id.as_deref(),
            expected_resume,
            "live {mode:?}"
        );
        // Repeat publication, then the normal post-execution authority commit.
        // None in later reconciliation must preserve legitimate Resume metadata,
        // not globally clear historical fields to hide incorrect Create writes.
        observer(&live_marker("session-a")).unwrap();
        crate::session_authority::commit_session_authority(
            crate::session_authority::SessionAuthorityCommitRequest {
                state: &state,
                invocation_row_id: target.invocation_row_id,
                invocation_uuid: &target.invocation_uuid,
                expectation: SessionAuthorityExpectation {
                    account_name: "fixture",
                    provider_session_id: Some("session-a"),
                },
                observation: Some(AuthoritativeSessionObservation {
                    account_name: "fixture",
                    provider_session_id: "session-a",
                }),
                capture_method: "external_provider_launch",
                provider_instance_id: &endpoint.provider_instance_id,
                settings_id: &endpoint.settings_id,
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
        state
            .finalize_invocation(
                oulipoly_state::InvocationMutationAuthority::Standalone,
                target.invocation_row_id,
                true,
                0,
                None,
                None,
            )
            .unwrap();
        drop(state);
        let reopened = oulipoly_state::StateDb::open(&target.state_path).unwrap();
        let row = reopened
            .get_invocation_by_uuid(&target.invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(
            row.resume_input_id.as_deref(),
            expected_resume,
            "durable {mode:?}"
        );
        assert_eq!(row.provider_session_id.as_deref(), Some("session-a"));
        assert!(
            reopened
                .invocation_provider_session_authority(target.invocation_row_id)
                .unwrap()
                .is_some()
        );
    }
}
