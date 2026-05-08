mod common;

use oulipoly_agent_messenger::append_return_channel;
use uuid::Uuid;

// proposal § Test-Intent Track row: channel JSONL append
// contract § Expected observable signals row: return writes JSONL receipt to channel
// named risk: Messenger Receive Transport HIGH - channel writes could omit line boundaries or reorder receipts
// selected level: library_integration
#[test]
fn append_return_channel_writes_one_json_line_per_receipt_in_order() {
    let db = common::TempDb::new();
    let channel = db.output_path("returns.jsonl");
    let invocation = Uuid::new_v4();
    let first = common::artifact_receipt(invocation, "first.md", 1);
    let second = common::artifact_receipt(invocation, "first.md", 2);

    append_return_channel(&channel, &first).expect("append first");
    append_return_channel(&channel, &second).expect("append second");

    let body = std::fs::read_to_string(&channel).expect("channel body");
    let lines: Vec<_> = body.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(lines[0])
            .expect("first JSON")
            .get("version_id")
            .and_then(serde_json::Value::as_str),
        Some(first.version_id.as_str())
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(lines[1])
            .expect("second JSON")
            .get("version_id")
            .and_then(serde_json::Value::as_str),
        Some(second.version_id.as_str())
    );
}

// proposal § Test-Intent Track row: channel Io error
// contract § Expected observable signals row: no partial channel write on failure
// named risk: Messenger Receive Transport HIGH - failed appends could leave partial malformed JSON consumed by the parent
// selected level: library_integration
#[test]
fn append_return_channel_io_error_writes_no_partial_line() {
    let db = common::TempDb::new();
    let channel = db.output_path("missing").join("returns.jsonl");
    let receipt = common::artifact_receipt(Uuid::new_v4(), "result.md", 1);

    let err = append_return_channel(&channel, &receipt).expect_err("append fails");

    assert!(matches!(
        err,
        oulipoly_agent_messenger::MessengerError::Io(_)
    ));
    assert!(
        !channel.exists(),
        "failed append must not create a partial channel file"
    );
}
