//! ## Declared roles
//!
//! `validator`, `parser`, `mapper`

use clap::Parser;

use crate::usage::cli::{Cli, Subcommands};

// risk: CLI surface; level: unit; source: proposal §11.1 CLI surface / A5, A8.
#[test]
fn resume_list_user_syntax_rewrites_to_hidden_subcommand() {
    let argv = super::normalize_resume_list_args([
        "oulipoly-agent-runner",
        "resume",
        "--list",
        "5169694d-de0f-40d1-890c-6e28e55bab27",
    ]);

    let cli = Cli::try_parse_from(argv).unwrap();

    match cli.command {
        Some(Subcommands::ResumeList { uuid }) => {
            assert_eq!(uuid, "5169694d-de0f-40d1-890c-6e28e55bab27");
        }
        other => panic!("expected hidden resume-list variant, got {other:?}"),
    }
}

// risk: CLI surface; level: unit; source: proposal §11.1 CLI surface / A5, A8.
#[test]
fn resume_list_line_includes_required_chain_fields() {
    let ts = chrono::DateTime::parse_from_rfc3339("2026-04-17T08:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let preview = oulipoly_state::ChainPreview {
        chain_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        last_used_at: ts,
        active_provider: "claude".to_string(),
        active_session_id: "dd116a3c-6819-42b1-b3d2-f512331eb5ec".to_string(),
        turn_count: 42,
        recent_turns: vec![oulipoly_state::TurnPreview {
            role: "assistant".to_string(),
            timestamp: ts,
            snippet: None,
        }],
    };

    let line = super::format_resume_list_line(&preview);

    assert!(line.contains("chain_id=5169694d-de0f-40d1-890c-6e28e55bab27"));
    assert!(line.contains("last_used_at=2026-04-17T08:00:00+00:00"));
    assert!(line.contains("active_provider=claude"));
    assert!(line.contains("active_session_id=dd116a3c-6819-42b1-b3d2-f512331eb5ec"));
    assert!(line.contains("turn_count=42"));
    assert!(line.contains("recent_turns_count=1"));
}
