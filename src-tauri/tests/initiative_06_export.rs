#![cfg(unix)]

mod fixtures;

use agent_runner_session::read_canonical_transcript;
use fixtures::initiative_06_export::*;
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};

/// Risk: T1: resolver pass-through; Claude Code session emits canonical JSONL with all 8 fields per line.
/// Level: particular-integration.
/// Source: contract section 8 row T1; contract sections 1, 3, and 4.
/// Observable: exit 0; stdout is compact JSONL; each record has the required canonical field set and Claude source metadata.
/// Residual: does not exhaust every Claude native content variant.
#[test]
fn export_claude_session_emits_canonical_jsonl_records() {
    let prepared = cli_claude_export_fixture();

    let output = prepared
        .fixture
        .run_export(&prepared.session_id, &["--format", "canonical-jsonl"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(stdout.ends_with('\n'), "{stdout:?}");
    assert_eq!(stdout.lines().count(), 2, "{stdout:?}");
    assert!(!stdout.contains("\n  "), "{stdout:?}");

    let records = parse_stdout_jsonl(&output);
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert_required_canonical_shape(record);
        assert_eq!(record["session_id"], prepared.session_id);
        assert_eq!(record["provider_name"], prepared.provider_name);
        assert_eq!(record["source"]["storage_type"], "claude_code");
        assert_eq!(
            record["source"]["jsonl_path"],
            prepared.jsonl_path.to_string_lossy().as_ref()
        );
        assert_eq!(record["unsupported_record"], false);
        assert!(!record["content"].as_array().unwrap().is_empty());
    }
    assert_eq!(records[0]["turn_id"], "claude-turn-1");
    assert_eq!(records[0]["role"], "user");
    assert_eq!(records[1]["turn_id"], "claude-turn-2");
    assert_eq!(records[1]["role"], "assistant");
}

/// Risk: T2: Codex session emits canonical JSONL with the same shape as Claude output.
/// Level: particular-integration.
/// Source: contract section 8 row T2; contract sections 3 and 6.
/// Observable: exit 0; stdout records parse as canonical JSONL with codex_session source metadata.
/// Residual: Codex compaction is out of v1 scope and is covered as a residual in the proposal.
#[test]
fn export_codex_session_emits_canonical_jsonl_records() {
    let prepared = cli_codex_export_fixture();

    let output = prepared.fixture.run_export(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let records = parse_stdout_jsonl(&output);
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert_required_canonical_shape(record);
        assert_eq!(record["session_id"], prepared.session_id);
        assert_eq!(record["provider_name"], prepared.provider_name);
        assert_eq!(record["source"]["storage_type"], "codex_session");
        assert_eq!(record["unsupported_record"], false);
    }
    assert_eq!(records[0]["role"], "user");
    assert_eq!(records[1]["role"], "assistant");
}

/// Risk: T3: source preimage fields drift from the exact provider JSONL bytes.
/// Level: component.
/// Source: contract section 8 row T3; contract section 3 source metadata.
/// Observable: direct canonical-reader output has correct physical line number, byte offsets, and SHA-256 for LF, CRLF, and final unterminated lines.
/// Residual: non-UTF-8 paths are covered by resolver storage rejection, not by this parser component test.
#[test]
fn canonical_reader_source_metadata_matches_jsonl_preimage() {
    let fixture = component_source_preimage_fixture();

    let records = read_canonical_transcript(&fixture.metadata).unwrap();

    assert_eq!(records.len(), 3);
    let hashes = hardcoded_source_hashes();
    let expected = source_spans_by_turn_id(&fixture.jsonl_path);
    for record in &records {
        let turn_id = record.turn_id.as_str();
        let (line, byte_start, byte_end) = expected[turn_id];
        assert_eq!(record.source.line, line);
        assert_eq!(record.source.byte_start, byte_start);
        assert_eq!(record.source.byte_end, byte_end);
        assert_eq!(record.source.sha256, hashes[turn_id]);
        assert_eq!(record.source.jsonl_path, fixture.jsonl_path);
        assert_eq!(record.source.storage_type, "claude_code");
    }
}

fn source_spans_by_turn_id(path: &Path) -> BTreeMap<String, (u64, u64, u64)> {
    let bytes = fs::read(path).unwrap();
    let mut spans = BTreeMap::new();
    let mut line = 1_u64;
    let mut byte_start = 0_usize;

    while byte_start < bytes.len() {
        let newline = bytes[byte_start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|relative| byte_start + relative);
        let raw_end = newline.unwrap_or(bytes.len());
        let byte_end = if raw_end > byte_start && bytes[raw_end - 1] == b'\r' {
            raw_end - 1
        } else {
            raw_end
        };
        let line_bytes = &bytes[byte_start..byte_end];

        if line_bytes.iter().any(|byte| !byte.is_ascii_whitespace()) {
            let value: Value = serde_json::from_slice(line_bytes).unwrap();
            let turn_id = value["uuid"].as_str().unwrap().to_string();
            spans.insert(turn_id, (line, byte_start as u64, byte_end as u64));
        }

        match newline {
            Some(index) => {
                byte_start = index + 1;
                line += 1;
            }
            None => break,
        }
    }

    spans
}

/// Risk: T4: canonical transcript order silently changes from provider JSONL order.
/// Level: component.
/// Source: contract section 8 row T4; proposal section 4 D5.
/// Observable: direct canonical-reader output preserves source-file order for a multi-record fixture.
/// Residual: timestamp regression failure is a parser validation detail not separately fuzzed here.
#[test]
fn canonical_reader_preserves_provider_jsonl_order() {
    let fixture = component_ordering_fixture();

    let records = read_canonical_transcript(&fixture.metadata).unwrap();

    let turn_ids = records
        .iter()
        .map(|record| record.turn_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(turn_ids, vec!["claude-turn-1", "claude-turn-2"]);
}

/// Risk: T5: unsupported provider-native transcript records are dropped or fail the whole export instead of being represented as safe placeholders.
/// Level: component.
/// Source: contract section 8 row T5; contract section 3 unsupported_record rule.
/// Observable: direct canonical-reader output includes an unsupported placeholder with empty content and source metadata.
/// Residual: unsafe unsupported records still correctly exit 15 at CLI level only when the parser classifies them unsafe.
#[test]
fn canonical_reader_emits_unsupported_record_placeholders() {
    let fixture = component_unsupported_record_fixture();

    let records = read_canonical_transcript(&fixture.metadata).unwrap();

    assert_eq!(records.len(), 2);
    let unsupported = records
        .iter()
        .find(|record| record.turn_id == "claude-system-1")
        .expect("unsupported system record should be present");
    assert!(unsupported.unsupported_record);
    assert!(unsupported.content.is_empty());
    assert_eq!(unsupported.source.line, 2);
}

/// Risk: T6: malformed provider transcript writes partial stdout before failing.
/// Level: particular-integration.
/// Source: contract section 8 row T6; contract sections 3 and 5.
/// Observable: garbled mid-stream JSONL exits 15, stdout is empty, stderr is one JSON error line.
/// Residual: operating-system read failures are mapped as operational errors by a separate path.
#[test]
fn export_malformed_transcript_exits_15_without_partial_stdout() {
    let prepared = cli_malformed_transcript_fixture();

    let output = prepared.fixture.run_export(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(15), "{output:?}");
    let json = assert_json_error(&output, "malformed-provider-transcript");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("transcript"),
        "{json}"
    );
}

/// Risk: T7: export mutates state, config, cursors, quota rows, or provider transcripts while reading.
/// Level: particular-integration.
/// Source: contract section 8 row T7; contract section 7 side-effect contract.
/// Observable: row counts, transcript bytes and mtime, and config bytes are unchanged after a successful CLI export.
/// Residual: the contract permits the transcript locator to create STATE_DIR, so locator state-directory existence is not asserted.
#[test]
fn export_does_not_mutate_state_rows_transcript_or_config() {
    let prepared = cli_read_only_fixture();
    let before = prepared
        .fixture
        .snapshot_read_only_state(&prepared.jsonl_path);

    let output = prepared.fixture.run_export(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let after = prepared
        .fixture
        .snapshot_read_only_state(&prepared.jsonl_path);
    assert_eq!(after, before);
}

/// Risk: T8: compaction-aware emission includes stale pre-compaction records or omits the boundary summary.
/// Level: component.
/// Source: contract section 8 row T8; proposal section 4 D4.
/// Observable: direct canonical-reader output starts at the latest compact-summary boundary and excludes earlier records.
/// Residual: Codex has no v1 compaction marker and is intentionally outside this component fixture.
#[test]
fn canonical_reader_emits_live_transcript_from_latest_compaction_boundary() {
    let fixture = component_compaction_fixture();

    let records = read_canonical_transcript(&fixture.metadata).unwrap();

    let turn_ids = records
        .iter()
        .map(|record| record.turn_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(turn_ids, vec!["compact-summary", "post-compact"]);
    assert!(!turn_ids.contains(&"claude-turn-1"));
}

/// Risk: T9: resolver missing-session errors are remapped to a generic operational failure.
/// Level: particular-integration.
/// Source: contract section 8 row T9; contract sections 4 and 5.
/// Observable: unknown well-formed UUID exits 10 with stderr code session-not-found and empty stdout.
/// Residual: invalid UUID exit 2 is part of the CLI parse path, not this resolver mapping group.
#[test]
fn export_unknown_well_formed_uuid_exits_session_not_found() {
    let fixture = cli_missing_uuid_fixture();

    let output = fixture.run_export("99999999-9999-4999-8999-999999999999", &[]);

    assert_eq!(output.status.code(), Some(10), "{output:?}");
    assert_json_error(&output, "session-not-found");
}

/// Risk: T9: resolver ambiguous-session errors are remapped to a generic operational failure.
/// Level: particular-integration.
/// Source: contract section 8 row T9; contract sections 4 and 5.
/// Observable: two recent owner chains for the same session exit 11 with stderr code ambiguous-session and empty stdout.
/// Residual: ambiguity preview payload shape is owned by locate, not reasserted by export.
#[test]
fn export_ambiguous_session_exits_ambiguous_session() {
    let prepared = cli_ambiguous_session_fixture();

    let output = prepared.fixture.run_export(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(11), "{output:?}");
    assert_json_error(&output, "ambiguous-session");
}

/// Risk: T9: resolver unsupported-storage errors are remapped to a generic operational failure.
/// Level: particular-integration.
/// Source: contract section 8 row T9; contract sections 4 and 5.
/// Observable: located provider with storage_type other exits 12 with stderr code unsupported-storage and empty stdout.
/// Residual: individual locator failure flavors are covered by locate tests and are not duplicated here.
#[test]
fn export_unsupported_storage_exits_unsupported_storage() {
    let prepared = cli_unsupported_storage_fixture();

    let output = prepared.fixture.run_export(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(12), "{output:?}");
    assert_json_error(&output, "unsupported-storage");
}
