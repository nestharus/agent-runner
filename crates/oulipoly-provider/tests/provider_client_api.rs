use oulipoly_provider::client::{
    ProviderClient, ProviderClientOptions, ProviderOutputLimits, ProviderTimeouts,
};
use oulipoly_provider::error::{
    HostErrorKind, ProviderCapabilityError, ProviderClientError, ProviderDiagnostics,
};
use oulipoly_provider::resolver::{
    ProviderArtifactRef, ProviderResolveOptions, ProviderResolver, ResolvedProviderCommand,
};
use oulipoly_provider::stream::{DecodedLaunchEvent, LaunchExit, LaunchJsonlReader};
use std::ffi::OsString;
use std::time::Duration;

#[test]
fn downstream_can_construct_client_resolver_options_and_name_errors() {
    let artifact = ProviderArtifactRef::Path {
        path: "/tmp/fake-provider".into(),
    };
    let resolver = ProviderResolver::new(ProviderResolveOptions::default());
    let client = ProviderClient::new(
        artifact,
        ProviderClientOptions {
            timeouts: ProviderTimeouts {
                default: Duration::from_secs(5),
                launch: Duration::from_secs(30),
                kill_after_grace: Duration::from_millis(100),
            },
            output_limits: ProviderOutputLimits {
                stdout_bytes: 1024 * 1024,
                stderr_bytes: 128 * 1024,
            },
            ..ProviderClientOptions::default()
        },
    );

    let error = ProviderClientError::host_transport(
        HostErrorKind::SpawnFailed,
        "describe",
        Some("request-example-001".to_owned()),
        ProviderDiagnostics::default(),
    );

    assert_eq!(resolver.options().path_entries().len(), 0);
    assert_eq!(client.options().output_limits.stderr_bytes, 128 * 1024);
    assert_eq!(error.transport_kind(), "spawn_failed");
}

#[test]
fn public_launch_stream_types_preserve_decoded_bytes_and_exit() {
    let event = DecodedLaunchEvent::Stdout {
        seq: 1,
        data: vec![0, 1, 255],
    };
    let reader = LaunchJsonlReader::new("request-example-001");

    assert_eq!(event.seq(), 1);
    assert_eq!(event.bytes(), Some(&[0, 1, 255][..]));
    assert_eq!(reader.request_id(), "request-example-001");
    let _exit_type: Option<LaunchExit> = None;
}

#[test]
fn resolved_command_public_shape_is_exact_artifact_plus_subcommand() {
    let resolved = ResolvedProviderCommand::new("/tmp/fake-provider");
    assert_eq!(
        resolved.argv_for_subcommand("describe"),
        vec![
            OsString::from("/tmp/fake-provider"),
            OsString::from("describe")
        ]
    );
}

#[test]
fn provider_capability_error_type_is_public_without_legacy_trait_imports() {
    let _type_name = std::any::type_name::<ProviderCapabilityError>();
}
