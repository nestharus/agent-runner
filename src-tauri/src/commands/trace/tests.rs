//! ## Declared roles
//!
//! `validator`, `parser`, `formatter`

use crate::usage::cli::{Cli, Subcommands};
use clap::Parser;

const TRACE_UUID: &str = "11111111-1111-1111-1111-111111111111";

#[test]
fn trace_subcommand_parses_json_and_inline_transcript_flags() {
    let cli = Cli::try_parse_from([
        "oulipoly-agent-runner",
        "trace",
        TRACE_UUID,
        "--json",
        "--inline-transcript",
        "--max-depth",
        "10",
    ])
    .unwrap();

    match cli.command {
        Some(Subcommands::Trace {
            invocation_uuid,
            json,
            inline_transcript,
            transcript,
            max_depth,
        }) => {
            assert_eq!(invocation_uuid, TRACE_UUID);
            assert!(json);
            assert!(inline_transcript);
            assert!(!transcript);
            assert_eq!(max_depth, 10);
        }
        _ => panic!("expected trace subcommand"),
    }
}

#[test]
fn trace_subcommand_rejects_inline_transcript_without_json() {
    let err = match Cli::try_parse_from([
        "oulipoly-agent-runner",
        "trace",
        TRACE_UUID,
        "--inline-transcript",
    ]) {
        Ok(_) => panic!("expected clap to reject --inline-transcript without --json"),
        Err(err) => err,
    };

    let rendered = err.to_string();
    assert!(rendered.contains("--json"), "{rendered}");
}

#[test]
fn trace_subcommand_rejects_transcript_with_json() {
    // Per contract: `--transcript` is the human-mode footer; `--json`
    // surfaces transcripts via `--inline-transcript` instead. Clap
    // must reject the combination.
    let err = match Cli::try_parse_from([
        "oulipoly-agent-runner",
        "trace",
        "00000000-0000-0000-0000-000000000000",
        "--json",
        "--transcript",
    ]) {
        Ok(_) => panic!("expected clap to reject --json --transcript"),
        Err(e) => e,
    };
    let rendered = err.to_string();
    assert!(
        rendered.contains("--transcript") || rendered.contains("--json"),
        "{rendered}"
    );
}
