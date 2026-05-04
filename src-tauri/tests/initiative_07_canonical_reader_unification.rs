#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06_import_replace::*;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// Risk: RC-2 — Claude array content containing non-text chunks may hash differently between export and import-replace.
/// Level: CLI integration.
/// Source: ~/projects/agent-runner/planning/trunk/research/07-canonical-reader-divergence-rca.md RC-2; ~/projects/agent-runner/planning/trunk/proposals/07-canonical-reader-unification.md §4.
/// Observable: export-derived preimage hash is accepted; receipt preimage_sha256 equals that oracle.
/// Residual: covers one representative tool_use chunk, not every Claude structured content kind.
#[test]
fn t_rc2_claude_content_array_with_tool_use_chunk_accepts_export_preimage() {
    let prepared = prepared_claude_fixture_with_transcript(&format!(
        "{}\n{}\n",
        claude_message_string_line(SESSION_A, "old-turn-1", "user", "old user", 0),
        claude_tool_use_content_line(SESSION_A, "old-turn-2", 1),
    ));
    let before_export = prepared.fixture.run_export(&prepared.session_id);
    assert_eq!(before_export.status.code(), Some(0), "{before_export:?}");
    let oracle = sha256sum_bytes(&before_export.stdout);
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "rc2",
    );

    let output = prepared.fixture.run_import_replace(
        &prepared.session_id,
        &input,
        &["--preimage-sha256", oracle.as_str()],
    );

    let receipt = assert_success(&output);
    assert_eq!(receipt["preimage_sha256"], oracle);
}

/// Risk: RC-4 — Claude compaction summaries may leave pre-summary turns in import-replace's preimage hash.
/// Level: CLI integration.
/// Source: ~/projects/agent-runner/planning/trunk/research/07-canonical-reader-divergence-rca.md RC-4; ~/projects/agent-runner/planning/trunk/proposals/07-canonical-reader-unification.md §4.
/// Observable: export-derived preimage hash is accepted; receipt preimage_sha256 equals that oracle.
/// Residual: covers one latest compaction boundary, not multiple summary markers.
#[test]
fn t_rc4_claude_compaction_summary_accepts_export_preimage() {
    let prepared = prepared_claude_fixture_with_transcript(&format!(
        "{}\n{}\n{}\n",
        claude_message_string_line(SESSION_A, "old-turn-1", "user", "pre-summary", 0),
        claude_compaction_summary_line(SESSION_A, "summary-turn", 1),
        claude_message_string_line(SESSION_A, "old-turn-2", "assistant", "post-summary", 2),
    ));
    let before_export = prepared.fixture.run_export(&prepared.session_id);
    assert_eq!(before_export.status.code(), Some(0), "{before_export:?}");
    let oracle = sha256sum_bytes(&before_export.stdout);
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "rc4",
    );

    let output = prepared.fixture.run_import_replace(
        &prepared.session_id,
        &input,
        &["--preimage-sha256", oracle.as_str()],
    );

    let receipt = assert_success(&output);
    assert_eq!(receipt["preimage_sha256"], oracle);
}

/// Risk: RC-5 — out-of-order Claude timestamps may be accepted by import-replace after export rejects them.
/// Level: CLI integration.
/// Source: ~/projects/agent-runner/planning/trunk/research/07-canonical-reader-divergence-rca.md RC-5; ~/projects/agent-runner/planning/trunk/proposals/07-canonical-reader-unification.md §4; TA-07-F03.
/// Observable: export and import-replace both exit non-zero; transcript bytes, session_turns, and pending journals are unchanged.
/// Residual: asserts one decreasing timestamp pair, not every timestamp normalization edge.
#[test]
fn t_rc5_claude_out_of_order_timestamps_reject_without_mutation() {
    let transcript = format!(
        "{}\n{}\n",
        claude_message_string_line(SESSION_A, "old-turn-1", "user", "first", 1),
        claude_message_string_line(SESSION_A, "old-turn-2", "assistant", "second", 0),
    );
    let prepared = prepared_claude_fixture_with_transcript(&transcript);
    let export = prepared.fixture.run_export(&prepared.session_id);
    assert_ne!(export.status.code(), Some(0), "{export:?}");
    let legacy_preimage = legacy_claude_preimage_hash(
        &prepared.provider_name,
        &prepared.jsonl_path,
        transcript.as_bytes(),
    );
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "rc5",
    );
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );

    let output = prepared.fixture.run_import_replace(
        &prepared.session_id,
        &input,
        &["--preimage-sha256", legacy_preimage.as_str()],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_mutation(&prepared, &before);
}

/// Risk: RC-6 — Codex transcripts missing session_meta may be accepted by import-replace after export rejects them.
/// Level: CLI integration.
/// Source: ~/projects/agent-runner/planning/trunk/research/07-canonical-reader-divergence-rca.md RC-6; ~/projects/agent-runner/planning/trunk/proposals/07-canonical-reader-unification.md §4; TA-07-F03.
/// Observable: export and import-replace both exit non-zero; transcript bytes, session_turns, and pending journals are unchanged.
/// Residual: covers absent session_meta, not mismatched session_meta id.
#[test]
fn t_rc6_codex_without_session_meta_rejects_without_mutation() {
    let transcript = format!(
        "{}\n{}\n",
        codex_response_item_line("old-turn-1", "user", "old codex user", 0),
        codex_response_item_line("old-turn-2", "assistant", "old codex assistant", 1),
    );
    let prepared = prepared_codex_fixture_with_transcript(&transcript);
    let export = prepared.fixture.run_export(&prepared.session_id);
    assert_ne!(export.status.code(), Some(0), "{export:?}");
    let legacy_preimage = legacy_codex_preimage_hash(
        &prepared.provider_name,
        &prepared.jsonl_path,
        transcript.as_bytes(),
    );
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "rc6",
    );
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );

    let output = prepared.fixture.run_import_replace(
        &prepared.session_id,
        &input,
        &["--preimage-sha256", legacy_preimage.as_str()],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_mutation(&prepared, &before);
}

fn prepared_claude_fixture_with_transcript(body: &str) -> PreparedReplace {
    let fixture = ImportReplaceFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let workspace_root = fixture.root().join("workspace");
    let jsonl_path = fixture.stage_claude_jsonl(&projects_dir, &workspace_root, SESSION_A, body);
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(
        CLAUDE_PROVIDER,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
    );
    fixture.write_sessions_with_locator_path(CLAUDE_PROVIDER, &jsonl_path);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    fixture.seed_turns_with_metadata(CLAUDE_PROVIDER, SESSION_A, &jsonl_path);
    PreparedReplace {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        jsonl_path,
    }
}

fn prepared_codex_fixture_with_transcript(body: &str) -> PreparedReplace {
    let fixture = ImportReplaceFixture::new();
    let sessions_dir = fixture.root().join("codex-sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let jsonl_path = fixture.stage_jsonl("rollout-2026-04-17.jsonl", body);
    fixture.write_model(MODEL, &[CODEX_PROVIDER]);
    fixture.write_provider(
        CODEX_PROVIDER,
        StorageKind::Codex {
            sessions_dir: &sessions_dir,
        },
    );
    fixture.write_sessions_with_locator_path(CODEX_PROVIDER, &jsonl_path);
    fixture.seed_active_chain(
        CHAIN_A,
        CODEX_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    fixture.seed_turns_with_metadata(CODEX_PROVIDER, SESSION_A, &jsonl_path);
    PreparedReplace {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CODEX_PROVIDER.to_string(),
        jsonl_path,
    }
}

fn assert_no_mutation(prepared: &PreparedReplace, before: &MutationSnapshot) {
    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_eq!(after.transcript_bytes, before.transcript_bytes);
    assert_eq!(after.turn_rows, before.turn_rows);
    assert!(
        !prepared
            .fixture
            .pending_journal_path(&prepared.session_id)
            .exists(),
        "{:?}",
        prepared.fixture.pending_journal_path(&prepared.session_id)
    );
    assert_eq!(after.journal_files, before.journal_files);
}

fn claude_message_string_line(
    session_id: &str,
    turn_id: &str,
    role: &str,
    message: &str,
    offset: i64,
) -> String {
    json!({
        "sessionId": session_id,
        "type": role,
        "uuid": turn_id,
        "timestamp": format!("2026-04-17T08:00:{offset:02}Z"),
        "message": message,
    })
    .to_string()
}

fn claude_tool_use_content_line(session_id: &str, turn_id: &str, offset: i64) -> String {
    json!({
        "sessionId": session_id,
        "type": "assistant",
        "uuid": turn_id,
        "timestamp": format!("2026-04-17T08:00:{offset:02}Z"),
        "message": {
            "role": "assistant",
            "content": [
                {"type": "text", "text": "old assistant"},
                {"type": "tool_use", "id": "toolu_01", "name": "Read", "input": {"file_path": "Cargo.toml"}}
            ]
        }
    })
    .to_string()
}

fn claude_compaction_summary_line(session_id: &str, turn_id: &str, offset: i64) -> String {
    json!({
        "sessionId": session_id,
        "type": "summary",
        "uuid": turn_id,
        "timestamp": format!("2026-04-17T08:00:{offset:02}Z"),
        "isCompactSummary": true,
        "message": {"role": "assistant", "content": "compact summary"},
    })
    .to_string()
}

fn codex_response_item_line(turn_id: &str, role: &str, text: &str, offset: i64) -> String {
    let content_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    json!({
        "type": "response_item",
        "timestamp": format!("2026-04-17T08:00:{offset:02}Z"),
        "payload": {
            "type": "message",
            "id": turn_id,
            "role": role,
            "content": [{"type": content_type, "text": text}],
        }
    })
    .to_string()
}

fn legacy_claude_preimage_hash(provider_name: &str, jsonl_path: &Path, bytes: &[u8]) -> String {
    let mut records = Vec::new();
    for line in jsonl_data_lines(bytes) {
        let value: Value = serde_json::from_slice(line.text).unwrap();
        let role = value
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/message/role").and_then(Value::as_str))
            .unwrap_or("assistant");
        let (content, unsupported_content) = legacy_claude_content(&value);
        records.push(LegacyCanonicalRecord {
            session_id: value["sessionId"].as_str().unwrap().to_string(),
            provider_name: provider_name.to_string(),
            turn_id: value["uuid"].as_str().unwrap().to_string(),
            role: role.to_string(),
            timestamp: value["timestamp"].as_str().unwrap().to_string(),
            content,
            source: legacy_source("claude_code", jsonl_path, &line),
            unsupported_record: !matches!(role, "user" | "assistant") || unsupported_content,
        });
    }
    legacy_records_hash(&records)
}

fn legacy_codex_preimage_hash(provider_name: &str, jsonl_path: &Path, bytes: &[u8]) -> String {
    let mut session_id = String::new();
    let mut records = Vec::new();
    for line in jsonl_data_lines(bytes) {
        let value: Value = serde_json::from_slice(line.text).unwrap();
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            session_id = value
                .pointer("/payload/id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            continue;
        }
        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let payload = value.get("payload").unwrap();
        if payload.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let role = payload
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("assistant");
        let turn_id = value
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| payload.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| format!("codex-line-{}", line.number));
        let (content, unsupported_content) = legacy_text_items(payload.get("content"));
        records.push(LegacyCanonicalRecord {
            session_id: session_id.clone(),
            provider_name: provider_name.to_string(),
            turn_id,
            role: role.to_string(),
            timestamp: value["timestamp"].as_str().unwrap().to_string(),
            content,
            source: legacy_source("codex_session", jsonl_path, &line),
            unsupported_record: !matches!(role, "user" | "assistant") || unsupported_content,
        });
    }
    legacy_records_hash(&records)
}

fn legacy_claude_content(value: &Value) -> (Vec<LegacyContentChunk>, bool) {
    let Some(message) = value.get("message") else {
        return (Vec::new(), false);
    };
    if let Some(text) = message.as_str() {
        return (
            vec![LegacyContentChunk::Text {
                text: text.to_string(),
            }],
            false,
        );
    }
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        return (
            vec![LegacyContentChunk::Text {
                text: text.to_string(),
            }],
            false,
        );
    }
    legacy_text_items(message.get("content"))
}

fn legacy_text_items(value: Option<&Value>) -> (Vec<LegacyContentChunk>, bool) {
    let Some(items) = value.and_then(Value::as_array) else {
        return (Vec::new(), false);
    };
    let mut unsupported = false;
    let content = items
        .iter()
        .filter_map(|item| {
            item.get("text")
                .and_then(Value::as_str)
                .map(|text| LegacyContentChunk::Text {
                    text: text.to_string(),
                })
                .or_else(|| {
                    unsupported = true;
                    None
                })
        })
        .collect();
    (content, unsupported)
}

fn legacy_records_hash(records: &[LegacyCanonicalRecord]) -> String {
    let mut out = Vec::new();
    for record in records {
        let value = serde_json::to_value(record).unwrap();
        serde_json::to_writer(&mut out, &value).unwrap();
        out.push(b'\n');
    }
    sha256sum_bytes(&out)
}

fn legacy_source(storage_type: &str, jsonl_path: &Path, line: &JsonlDataLine<'_>) -> Value {
    json!({
        "storage_type": storage_type,
        "jsonl_path": jsonl_path,
        "line": line.number,
        "byte_start": line.byte_start,
        "byte_end": line.byte_end,
        "sha256": sha256sum_bytes(line.text),
    })
}

fn jsonl_data_lines(bytes: &[u8]) -> Vec<JsonlDataLine<'_>> {
    let mut lines = Vec::new();
    let mut offset = 0_usize;
    for (idx, segment) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let raw = segment.strip_suffix(b"\n").unwrap_or(segment);
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        let byte_start = offset;
        let byte_end = byte_start + line.len();
        offset += segment.len();
        if !line.iter().all(|byte| byte.is_ascii_whitespace()) {
            lines.push(JsonlDataLine {
                number: idx as u64 + 1,
                text: line,
                byte_start: byte_start as u64,
                byte_end: byte_end as u64,
            });
        }
    }
    lines
}

fn sha256sum_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Serialize)]
struct LegacyCanonicalRecord {
    session_id: String,
    provider_name: String,
    turn_id: String,
    role: String,
    timestamp: String,
    content: Vec<LegacyContentChunk>,
    source: Value,
    unsupported_record: bool,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LegacyContentChunk {
    Text { text: String },
}

struct JsonlDataLine<'a> {
    number: u64,
    text: &'a [u8],
    byte_start: u64,
    byte_end: u64,
}
