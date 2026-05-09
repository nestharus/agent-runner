#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06_import_replace::{assert_json_error, prepared_claude_replace_fixture};

/// Risk: AGE-37 cut-over could move `--from-file` reading behind a new adapter
/// and accidentally change the current invalid-input mapping or mutate state.
/// Level: CLI characterization.
/// Observable: missing `--from-file` input exits 15, emits
/// invalid-input-transcript JSON on stderr, writes no stdout, and leaves the
/// transcript, DB rows, and replace journal untouched.
#[test]
fn import_replace_missing_from_file_exits_15_without_mutation() {
    let prepared = prepared_claude_replace_fixture();
    let missing_path = prepared.fixture.root().join("missing-canonical.jsonl");
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );

    let output = prepared.fixture.run_import_replace_from_file(
        &prepared.session_id,
        &missing_path,
        &[
            "--preimage-sha256",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ],
    );

    assert_eq!(output.status.code(), Some(15), "{output:?}");
    let error = assert_json_error(&output, "invalid-input-transcript");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("failed to read input file:")),
        "{error}"
    );
    assert_eq!(error["error"]["line"], serde_json::Value::Null, "{error}");

    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_eq!(after, before);
}
