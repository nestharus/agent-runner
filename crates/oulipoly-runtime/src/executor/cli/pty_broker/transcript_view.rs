//! ## Declared roles
//!
//! `parser`, `mapper`, `formatter`, `accessor`, `filter`, `orchestration`
//!
//! Project located transcript JSONL lines into readable conversation lines for
//! the inspect pane.
//!
//! ## Transcript display input contract
//!
//! The inspect pane consumes a small runner-owned display subset from located
//! transcript JSONL: top-level `type` values `user` and `assistant`, optional
//! `message.content` as either a string or an array of blocks, and block `type`
//! values `text`, `tool_use`, and `tool_result` with optional `text`/`name`
//! fields. Unparseable, partial, unsupported, or uninteresting events are
//! ignored; the caller can fall back to raw tail lines when projection is empty.

use serde_json::Value;

const TOOL_RESULT_PLACEHOLDER: &str = "(tool result)";

/// Project the raw tail lines of a transcript into readable display lines.
pub(super) fn project_transcript_tail(raw_lines: &[String]) -> Vec<String> {
    raw_lines
        .iter()
        .flat_map(|line| project_transcript_line(line))
        .collect()
}

fn project_transcript_line(line: &str) -> Vec<String> {
    match parse_transcript_event(line) {
        Some(value) => project_event(&value),
        None => Vec::new(),
    }
}

fn parse_transcript_event(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line).ok()
}

fn project_event(value: &Value) -> Vec<String> {
    match event_speaker(value) {
        Some(speaker) => project_message(speaker, value),
        None => Vec::new(),
    }
}

fn event_speaker(value: &Value) -> Option<&'static str> {
    supported_event_type(value).map(event_speaker_label)
}

fn event_type(value: &Value) -> Option<&str> {
    value.get("type").and_then(Value::as_str)
}

fn supported_event_type(value: &Value) -> Option<&str> {
    event_type(value).filter(|event_type| event_type_has_speaker(event_type))
}

fn event_type_has_speaker(event_type: &str) -> bool {
    matches!(event_type, "user" | "assistant")
}

fn event_speaker_label(event_type: &str) -> &'static str {
    match event_type {
        "user" => "you",
        "assistant" => "agent",
        _ => unreachable!("supported event type"),
    }
}

fn project_message(speaker: &str, value: &Value) -> Vec<String> {
    match supported_message_content(value) {
        Some(content) => project_supported_message_content(speaker, content),
        None => Vec::new(),
    }
}

fn message_content(value: &Value) -> Option<&Value> {
    value
        .get("message")
        .and_then(|message| message.get("content"))
}

fn supported_message_content(value: &Value) -> Option<&Value> {
    message_content(value).filter(|content| message_content_supported(content))
}

fn message_content_supported(content: &Value) -> bool {
    matches!(content, Value::String(_) | Value::Array(_))
}

fn project_supported_message_content(speaker: &str, content: &Value) -> Vec<String> {
    match content {
        Value::String(text) => speaker_lines(speaker, text),
        Value::Array(blocks) => project_content_blocks(speaker, blocks),
        _ => unreachable!("supported message content"),
    }
}

fn project_content_blocks(speaker: &str, blocks: &[Value]) -> Vec<String> {
    blocks
        .iter()
        .flat_map(|block| project_content_block(speaker, block))
        .collect()
}

fn project_content_block(speaker: &str, block: &Value) -> Vec<String> {
    match supported_block_type(block) {
        Some(block_type) => project_supported_content_block(speaker, block_type, block),
        None => Vec::new(),
    }
}

fn block_type(block: &Value) -> Option<&str> {
    block.get("type").and_then(Value::as_str)
}

fn supported_block_type(block: &Value) -> Option<&str> {
    block_type(block).filter(|block_type| block_type_supported(block_type))
}

fn block_type_supported(block_type: &str) -> bool {
    matches!(block_type, "text" | "tool_use" | "tool_result")
}

fn project_supported_content_block(speaker: &str, block_type: &str, block: &Value) -> Vec<String> {
    match block_type {
        "text" => project_text_block(speaker, block),
        "tool_use" => speaker_lines(speaker, &tool_use_summary(block)),
        "tool_result" => speaker_lines(speaker, TOOL_RESULT_PLACEHOLDER),
        _ => unreachable!("supported content block type"),
    }
}

fn project_text_block(speaker: &str, block: &Value) -> Vec<String> {
    match block_text(block) {
        Some(text) => speaker_lines(speaker, text),
        None => Vec::new(),
    }
}

fn block_text(block: &Value) -> Option<&str> {
    block.get("text").and_then(Value::as_str)
}

fn tool_use_summary(block: &Value) -> String {
    format!("tool: {}", block_tool_name(block))
}

fn block_tool_name(block: &Value) -> &str {
    block.get("name").and_then(Value::as_str).unwrap_or("tool")
}

/// Render one message part as a speaker-tagged first line plus indented
/// continuation lines, skipping blank-only text.
fn speaker_lines(speaker: &str, text: &str) -> Vec<String> {
    let lines = non_blank_lines(text);
    let Some((first, rest)) = lines.split_first() else {
        return Vec::new();
    };
    format_speaker_lines(speaker, first, rest)
}

fn format_speaker_lines(speaker: &str, first: &str, rest: &[&str]) -> Vec<String> {
    let mut rendered = vec![format!("{speaker}: {first}")];
    rendered.extend(rest.iter().map(|line| format!("  {line}")));
    rendered
}

fn non_blank_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_assistant_text() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello world"}]}}"#;
        assert_eq!(
            project_transcript_line(line),
            vec!["agent: hello world".to_string()]
        );
    }

    #[test]
    fn projects_user_string_content() {
        let line = r#"{"type":"user","message":{"content":"do the thing"}}"#;
        assert_eq!(
            project_transcript_line(line),
            vec!["you: do the thing".to_string()]
        );
    }

    #[test]
    fn projects_tool_use_as_named_summary() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#;
        assert_eq!(
            project_transcript_line(line),
            vec!["agent: tool: Bash".to_string()]
        );
    }

    #[test]
    fn collapses_tool_result_blocks() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"huge output"}]}}"#;
        assert_eq!(
            project_transcript_line(line),
            vec!["you: (tool result)".to_string()]
        );
    }

    #[test]
    fn multiline_text_indents_continuation() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"line one\nline two"}]}}"#;
        assert_eq!(
            project_transcript_line(line),
            vec!["agent: line one".to_string(), "  line two".to_string()]
        );
    }

    #[test]
    fn skips_unparseable_and_summary_lines() {
        assert!(project_transcript_line("{partial json fragment").is_empty());
        assert!(project_transcript_line(r#"{"type":"summary","summary":"x"}"#).is_empty());
    }

    #[test]
    fn projects_tail_in_order_dropping_noise() {
        let raw = vec![
            "{bad".to_string(),
            r#"{"type":"user","message":{"content":"hi"}}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hey"}]}}"#
                .to_string(),
        ];
        assert_eq!(
            project_transcript_tail(&raw),
            vec!["you: hi".to_string(), "agent: hey".to_string()]
        );
    }
}
