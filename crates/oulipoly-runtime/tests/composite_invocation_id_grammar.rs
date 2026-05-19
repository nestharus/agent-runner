//! AGE-160 risk: PP-004 marker grammar consumed by runtime parser paths.
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

/// AGE-160 risk: PP-004 declared canonical JSON marker parser.
/// Selected level: integration.
/// Source: the AGE-160 proposal § Test-intent track.
#[test]
fn age160_parses_declared_canonical_json_marker() {
    let id = Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap();
    let stderr_line = marker("codex2", id).stderr_line();
    let payload = stderr_line.strip_prefix(PREFIX).expect("marker prefix");

    let parsed = CompositeInvocationId::parse_env_value(payload).unwrap();

    assert_eq!(parsed.source, "codex2");
    assert_eq!(parsed.id.to_string(), id.to_string());
}

/// AGE-160 risk: PP-004 + A4 legacy compatibility grammar.
/// Selected level: integration.
/// Source: the AGE-160 proposal § Test-intent track; validates A4.
#[test]
fn age160_legacy_shell_mangled_path_formally_accepts_declared_compatibility_payload() {
    let id = Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap();
    let payload = format!("{{source:'codex2',id:'{id}',ignored:'legacy'}}");

    let parsed = CompositeInvocationId::parse_env_value(&payload).unwrap();

    assert_eq!(parsed.source, "codex2");
    assert_eq!(parsed.id.to_string(), id.to_string());
}

/// AGE-160 risk: PP-004 parent-env raw JSON shape.
/// Selected level: integration.
/// Source: the AGE-160 proposal § Test-intent track.
#[test]
fn age160_parent_env_marker_payload_matches_declared_grammar_without_prefix() {
    let id = Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap();
    let parent_env = serde_json::to_string(&marker("codex2", id)).unwrap();

    assert!(!parent_env.starts_with(PREFIX));
    assert_eq!(
        CompositeInvocationId::parse_env_value(&parent_env)
            .unwrap()
            .id
            .to_string(),
        id.to_string()
    );
    assert!(
        CompositeInvocationId::parse_env_value(&format!("{PREFIX}{parent_env}")).is_err(),
        "parent-env parser must not accept a stderr marker line with the prefix included"
    );
}
