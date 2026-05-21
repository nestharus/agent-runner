//! AGE-160 risk: PP-004 Tauri marker producer format.
//! Selected level: integration.
//! Source: the AGE-160 proposal § Test-intent track.

use oulipoly_state::CompositeInvocationId;
use uuid::Uuid;

const PREFIX: &str = "OULIPOLY_INVOCATION=";

fn marker(source: &str, id: Uuid) -> CompositeInvocationId {
    CompositeInvocationId {
        source: source.to_string(),
        id: id.to_string(),
    }
}

fn main_rs_source() -> &'static str {
    include_str!("../src/main.rs")
}

/// AGE-160 risk: PP-004 + A4 canonical producer-only marker grammar.
/// Selected level: integration.
/// Source: the AGE-160 proposal § Test-intent track; validates A4.
#[test]
fn age160_emits_declared_canonical_json_marker_on_stderr() {
    let id = Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap();
    let stderr_line = marker("codex2", id).stderr_line();

    assert_eq!(
        stderr_line,
        r#"OULIPOLY_INVOCATION={"source":"codex2","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#
    );
    let payload = stderr_line.strip_prefix(PREFIX).expect("marker prefix");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(payload).unwrap()["id"],
        id.to_string()
    );

    let source = main_rs_source();
    assert!(
        source.matches(".stderr_line()").count() >= 3,
        "run/repl/resume/balanced producer paths must keep emitting through CompositeInvocationId::stderr_line"
    );
}

/// AGE-160 risk: PP-004 parent-env raw JSON shape.
/// Selected level: integration.
/// Source: the AGE-160 proposal § Test-intent track.
#[test]
fn age160_parent_env_marker_format_matches_grammar() {
    let id = Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap();
    let parent_env = serde_json::to_string(&marker("codex2", id)).unwrap();

    assert!(!parent_env.starts_with(PREFIX));
    assert_eq!(
        parent_env,
        r#"{"source":"codex2","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#
    );

    let source = main_rs_source();
    assert!(
        source.contains("OULIPOLY_PARENT_INVOCATION"),
        "Tauri parent-env propagation must continue to own OULIPOLY_PARENT_INVOCATION"
    );
    assert!(
        source.matches("serde_json::to_string(&invocation)").count() >= 3,
        "run/repl/resume/balanced parent env propagation must use raw compact JSON without the stderr prefix"
    );
}
