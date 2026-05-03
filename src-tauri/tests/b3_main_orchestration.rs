#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06::{CLAUDE_PROVIDER, component_claude_success_fixture};

/// Risk: T13 - CLI session locate can keep one-off concrete state/config
/// loading while the library services use the new B1/B2/B3 bundle.
/// Level: end-to-end smoke.
/// Source: proposal §8 T13; B3 contract §3 `main.rs` CLI orchestration.
/// Observable: the supported CLI locate path still resolves an existing
/// fixture session and emits the same provider/session payload.
#[test]
fn cli_session_locate_path_still_resolves_existing_session() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);

    let output = prepared.fixture.run_locate(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["session_id"], prepared.session_id);
    assert_eq!(json["provider_name"], prepared.provider_name);
    assert_eq!(json["chain_id"], prepared.chain_id);
}
