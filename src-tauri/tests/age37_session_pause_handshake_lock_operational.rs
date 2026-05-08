#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06_import_replace::{assert_json_error, prepared_claude_replace_fixture};
use std::fs;

/// Risk: AGE-37 lock presenter seams could change the current
/// LockError::Operational CLI mapping.
/// Level: CLI characterization.
/// Observable: pause-handshake maps an operational acquire failure to exit 1
/// with operational-error JSON on stderr, no stdout receipt, and no token
/// written.
#[test]
fn pause_handshake_lock_acquire_operational_exits_1_without_token_receipt() {
    let prepared = prepared_claude_replace_fixture();
    fs::create_dir_all(prepared.fixture.lock_path(&prepared.session_id)).unwrap();

    let output = prepared.fixture.run_pause_handshake(&prepared.session_id);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = assert_json_error(&output, "operational-error");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("lock metadata")),
        "{error}"
    );
    assert!(error["error"].get("token").is_none(), "{error}");
    assert!(prepared.fixture.lock_path(&prepared.session_id).is_dir());
}
