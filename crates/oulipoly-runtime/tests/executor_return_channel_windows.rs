#![cfg(windows)]
//! Public standalone execution is consumption, not allocated transfer proof.
use base64::Engine;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::executor;
use std::collections::HashMap;

#[test]
fn windows_standalone_public_empty_artifact_invalid_and_cleanup_neighbors() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, dir.path());
    }
    let uuid = uuid::Uuid::new_v4();
    let parent = format!(r#"{{"source":"test","id":"{uuid}"}}"#);
    let reference = oulipoly_agent_messenger::ReturnedArtifactRef {
        version_id: format!("store://return/{uuid}/fixture/1"),
        name: "fixture".into(),
        store_address: oulipoly_agent_messenger::StoreAddress {
            workflow_run_id: format!("return:{uuid}"),
            artifact_name: "fixture".into(),
            version: 1,
        },
        sha256: "a".repeat(64),
        content_len: 3,
        format_hint: None,
        verdict_line: None,
        source: oulipoly_agent_messenger::ReturnedArtifactSource::InlineBytes,
        producer_invocation_uuid: uuid,
        returned_at: chrono::Utc::now(),
    };
    for (case, contents, blocked, valid) in [
        ("empty", String::new(), false, true),
        ("blank", " \r\n\t".into(), false, true),
        (
            "artifact",
            format!("{}\n", serde_json::to_string(&reference).unwrap()),
            false,
            true,
        ),
        ("invalid", "{broken\n".into(), false, false),
        ("cleanup", String::new(), true, false),
    ] {
        let observed = dir.path().join(format!("{case}-channel"));
        let script = format!(
            "$ErrorActionPreference='Stop'; $null=[Console]::In.ReadToEnd(); $p=$env:OULIPOLY_RETURN_CHANNEL; [IO.File]::WriteAllText('{}',$p); $bytes=[Text.UTF8Encoding]::new($false).GetBytes('{}'); $f=[IO.FileStream]::new($p,[IO.FileMode]::Append,[IO.FileAccess]::Write,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)); try {{ $f.Write($bytes,0,$bytes.Length) }} finally {{ $f.Dispose() }}; {}; [Console]::Out.Write('fixture-ok')",
            observed.display().to_string().replace('\'', "''"),
            contents.replace('\'', "''"),
            if blocked {
                "[IO.File]::WriteAllText((Join-Path (Split-Path $p) 'blocker'),'block')"
            } else {
                "$null=0"
            }
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(
            script
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let model = ModelConfig {
            name: "windows-channel-fixture".into(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig {
                name: "fixture".into(),
                command: "powershell.exe".into(),
                args: vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-EncodedCommand".into(),
                    encoded,
                ],
                environment: Default::default(),
                unset_environment: Default::default(),
                interactive_args: None,
                resume: None,
                session_capture: None,
                resume_acceptance: None,
                session_storage: None,
                system_prompt_override: None,
                tool_restrictions: None,
                invocation_mode: Default::default(),
            }],
            inputs: vec![],
            provider: None,
        };
        let result = executor::execute_with_inputs_and_env(
            &model,
            0,
            "fixture-prompt",
            None,
            &HashMap::new(),
            Some(&parent),
        );
        eprintln!("Windows standalone {case}: {result:?}");
        let path = std::path::PathBuf::from(std::fs::read_to_string(observed).unwrap());
        if valid {
            let result =
                result.expect("healthy standalone channel must not require transfer proof");
            assert_eq!(result.exit_code, 0);
            assert_eq!(result.stdout, b"fixture-ok");
            assert_eq!(
                result.returned_artifacts,
                if case == "artifact" {
                    vec![reference.clone()]
                } else {
                    vec![]
                }
            );
            assert!(!path.exists());
            assert!(!path.parent().unwrap().exists());
        } else {
            assert!(
                result
                    .unwrap_err()
                    .contains("return_channel_custody_uncertain")
            );
            assert!(
                path.parent().unwrap().exists(),
                "uncertain cleanup is retained"
            );
        }
    }
}

#[test]
fn windows_allocated_empty_channel_withholds_unsupported_unlink_proof() {
    // No process was started in this fixture: this receipt is a truthful
    // never-spawned input, not a fabricated non-Unix process-tree certificate.
    let attempt = uuid::Uuid::new_v4();
    let actor = oulipoly_provider::custody::ActorSettlementReceipt {
        attempt_id: attempt,
        operation: oulipoly_provider::custody::ProviderOperation::Launch,
        spawned: false,
        exact_process_identity: None,
        process_status: None,
        process_tree_terminated: false,
        leader_reaped: false,
        force_killed: false,
        host_cancellation_requested: false,
        operation_finished: true,
        uncertain: false,
    };
    let dir = tempfile::tempdir().unwrap();
    let channel = executor::ReturnChannel::for_attempt(
        dir.path(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        attempt,
        uuid::Uuid::new_v4(),
    )
    .unwrap();
    let settlement = channel.seal(&[actor], |_| {
        panic!("empty channel must not publish artifacts")
    });
    eprintln!("Windows allocated empty: {settlement:?}");
    assert!(!settlement.transferable());
    assert!(matches!(
        settlement,
        executor::ReturnChannelSettlement::CleanupFailed { .. }
    ));
}
