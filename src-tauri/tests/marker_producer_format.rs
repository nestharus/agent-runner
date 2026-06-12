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
    concat!(
        include_str!("../src/main.rs"),
        "\n",
        include_str!("../src/dispatch.rs"),
        "\n",
        include_str!("../src/run/mod.rs"),
        "\n",
        include_str!("../src/run/balancing/mod.rs"),
        "\n",
        include_str!("../src/run/balancing/orchestration.rs"),
        "\n",
        include_str!("../src/run/balancing/accessor.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/completed.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/disposition.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/quota.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/shared.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/attempt/spawn.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/config.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/invocation.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/quota.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/routing.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/context/session.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/executor_request.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/failure.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/finalizer_request.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/session_ingest.rs"),
        "\n",
        include_str!("../src/run/balancing/mapper/terminal.rs"),
        "\n",
        include_str!("../src/run/balancing/parser.rs"),
        "\n",
        include_str!("../src/run/balancing/disposition.rs"),
        "\n",
        include_str!("../src/run/balancing/filter.rs"),
        "\n",
        include_str!("../src/run/balancing/finalization.rs"),
        "\n",
        include_str!("../src/run/balancing/formatter.rs"),
        "\n",
        include_str!("../src/run/balancing/diagnostics.rs"),
        "\n",
        include_str!("../src/run/balancing/predicate.rs"),
        "\n",
        include_str!("../src/run/balancing/state_update.rs"),
        "\n",
        include_str!("../src/run/balancing/validator.rs"),
        "\n",
        include_str!("../src/run/repl/mod.rs"),
        "\n",
        include_str!("../src/run/repl/orchestration.rs"),
        "\n",
        include_str!("../src/run/repl/disposition.rs"),
        "\n",
        include_str!("../src/run/repl/finalization.rs"),
        "\n",
        include_str!("../src/run/repl/formatter.rs"),
        "\n",
        include_str!("../src/run/repl/mapper.rs"),
        "\n",
        include_str!("../src/run/repl/validator.rs"),
        "\n",
        include_str!("../src/run/resume/mod.rs"),
        "\n",
        include_str!("../src/run/resume/orchestration.rs"),
        "\n",
        include_str!("../src/run/resume/disposition.rs"),
        "\n",
        include_str!("../src/run/resume/finalization.rs"),
        "\n",
        include_str!("../src/run/resume/formatter.rs"),
        "\n",
        include_str!("../src/run/resume/mapper.rs"),
    )
}

/// AGE-160 risk: PP-004 + A4 canonical producer-only marker grammar.
/// Selected level: integration.
/// Source: the AGE-160 proposal § Test-intent track; validates A4.
#[test]
fn age160_emits_declared_canonical_json_marker_on_stderr() {
    let id = marker_id();
    let stderr_line = marker_stderr_line(id);
    let payload_id = marker_payload_id(&stderr_line);
    let expected_id = marker_id_string(id);

    assert_canonical_stderr_line(&stderr_line, &payload_id, &expected_id);
    assert_marker_producer_source_usage();
}

fn marker_id() -> Uuid {
    Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap()
}

fn marker_stderr_line(id: Uuid) -> String {
    let source = marker_source();
    marker(&source, id).stderr_line()
}

fn marker_source() -> String {
    ["cod", "ex2"].concat()
}

fn marker_id_string(id: Uuid) -> String {
    id.to_string()
}

fn marker_payload_id(stderr_line: &str) -> String {
    serde_json::from_str::<serde_json::Value>(marker_payload(stderr_line)).unwrap()["id"]
        .as_str()
        .expect("marker id string")
        .to_string()
}

fn marker_payload(stderr_line: &str) -> &str {
    stderr_line.strip_prefix(PREFIX).expect("marker prefix")
}

fn assert_canonical_stderr_line(stderr_line: &str, payload_id: &str, expected_id: &str) {
    assert_eq!(
        stderr_line,
        r#"OULIPOLY_INVOCATION={"source":"codex2","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#
    );
    assert_eq!(payload_id, expected_id);
}

fn assert_marker_producer_source_usage() {
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
    let parent_env = parent_env_marker(marker_id());

    assert_parent_env_marker(&parent_env);
    assert_parent_env_source_usage();
}

fn parent_env_marker(id: Uuid) -> String {
    let source = marker_source();
    serde_json::to_string(&marker(&source, id)).unwrap()
}

fn assert_parent_env_marker(parent_env: &str) {
    assert!(!parent_env.starts_with(PREFIX));
    assert_eq!(
        parent_env,
        r#"{"source":"codex2","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#
    );
}

fn assert_parent_env_source_usage() {
    let source = main_rs_source();
    assert!(
        source.contains("OULIPOLY_PARENT_INVOCATION"),
        "Tauri parent-env propagation must continue to own OULIPOLY_PARENT_INVOCATION"
    );
    assert!(
        source.matches("serde_json::to_string(invocation)").count()
            + source.matches("serde_json::to_string(&invocation)").count()
            >= 3,
        "run/repl/resume/balanced parent env propagation must use raw compact JSON without the stderr prefix"
    );
}
