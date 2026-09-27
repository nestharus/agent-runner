//! Private native K fixture: run the real provider client inside the held work.
//! The enclosing probe requires the existing private user namespace. This is
//! executable custody evidence, never proof of host-root sudo credentials.

const PRIVATE_CUSTODY_QUIESCENCE_POLL: std::time::Duration = std::time::Duration::from_millis(10);

use oulipoly_core::launch_custody::{LaunchCustody, LaunchScope};
use oulipoly_provider::client::{ProviderClient, ProviderClientOptions};
use oulipoly_provider::custody::AttemptActorCustody;
use oulipoly_provider::resolver::ProviderArtifactRef;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

pub(crate) fn run(marker: &Path) -> Result<(), String> {
    let script = marker.with_extension("provider.py");
    std::fs::write(&script, include_str!("private_provider_probe.py"))
        .map_err(|error| error.to_string())?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let custody = Arc::new(
        LaunchCustody::start(marker.with_extension("provider-proof"))
            .map_err(|error| error.to_string())?,
    );
    let scope = LaunchScope::enter(Some(custody.clone()));
    let actor = AttemptActorCustody::original_tree(uuid::Uuid::new_v4());
    let client = ProviderClient::new(
        ProviderArtifactRef::Path { path: script },
        ProviderClientOptions::default().with_attempt_custody(Some(actor.clone())),
    );
    client
        .invoke_json(
            "describe",
            serde_json::json!({
                "contract": "oulipoly.provider/v1",
                "request_id": "private-unrestricted",
                "provider_instance_id": "private-unrestricted",
                "host": {"app":"fixture","app_version":"0","platform":"linux",
                    "working_directory":".","config_root":".","data_root":".","env":{}},
                "params": {}
            }),
            vec![(
                "PRIVATE_PROVIDER_MARKER".to_owned(),
                marker.as_os_str().to_owned(),
            )],
        )
        .map_err(|error| error.to_string())?;
    if !actor
        .receipts()
        .iter()
        .all(|receipt| receipt.effect_incapable())
    {
        return Err("provider returned without original-tree settlement".into());
    }
    drop(scope);
    custody.seal();
    while !custody.quiescent() {
        std::thread::sleep(PRIVATE_CUSTODY_QUIESCENCE_POLL);
    }
    Ok(())
}
