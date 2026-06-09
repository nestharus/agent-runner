//! ## Declared roles
//!
//! `parser`, `mapper`, `formatter`, `accessor`, `filter`, `orchestration`
//!
//! Project raw transcript JSONL lines into readable conversation lines for the
//! inspect pane. Each input line is one transcript event; unparseable or
//! uninteresting events are dropped. The byte-bounded tail can begin with a
//! partial JSON fragment, which simply fails to parse and is skipped.

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
    match event_type(value)? {
        "user" => Some("you"),
        "assistant" => Some("agent"),
        _ => None,
    }
}

fn event_type(value: &Value) -> Option<&str> {
    value.get("type").and_then(Value::as_str)
}

fn project_message(speaker: &str, value: &Value) -> Vec<String> {
    match message_content(value) {
        Some(Value::String(text)) => speaker_lines(speaker, text),
        Some(Value::Array(blocks)) => project_content_blocks(speaker, blocks),
        _ => Vec::new(),
    }
}

fn message_content(value: &Value) -> Option<&Value> {
    value
        .get("message")
        .and_then(|message| message.get("content"))
}

fn project_content_blocks(speaker: &str, blocks: &[Value]) -> Vec<String> {
    blocks
        .iter()
        .flat_map(|block| project_content_block(speaker, block))
        .collect()
}

fn project_content_block(speaker: &str, block: &Value) -> Vec<String> {
    match block_type(block) {
        Some("text") => block_text(block)
            .map(|text| speaker_lines(speaker, text))
            .unwrap_or_default(),
        Some("tool_use") => speaker_lines(speaker, &tool_use_summary(block)),
        Some("tool_result") => speaker_lines(speaker, TOOL_RESULT_PLACEHOLDER),
        _ => Vec::new(),
    }
}

fn block_type(block: &Value) -> Option<&str> {
    block.get("type").and_then(Value::as_str)
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
