#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, assert_no_terminal_marker_on_stdout, assert_signal_consumer_source_wired,
    assert_single_terminal_signal, main_rs_source, source_block_after, terminal_signal_lines,
};

#[test]
fn repl_non_clean_interactive_signal_emits_marker_and_finalizes_failed() {
    let fixture = Age153Fixture::new();
    fixture.write_model("age153-repl", &["claude-age153-repl"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age153-repl",
        "printf '%s\\n' 'interactive non-clean stderr' >&2\nexit 42",
    )]);

    let output = fixture.run_repl("age153-repl");

    assert_eq!(output.status.code(), Some(42), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_single_terminal_signal(&stderr, "NonzeroExit", false);
    assert_eq!(
        fixture.failed_invocation_count("claude-age153-repl", "exit_nonzero"),
        1
    );
}

#[test]
fn repl_clean_interactive_signal_finalizes_success_without_marker() {
    let fixture = Age153Fixture::new();
    fixture.write_model("age153-repl-clean", &["claude-age153-repl-clean"]);
    fixture.write_providers_with_bodies(&[(
        "claude-age153-repl-clean",
        "printf '%s\\n' 'interactive clean stdout'\nexit 0",
    )]);

    let output = fixture.run_repl("age153-repl-clean");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(terminal_signal_lines(&stderr).len(), 0, "{stderr}");
    assert_eq!(
        fixture.successful_invocation_count_without_terminal_reason("claude-age153-repl-clean"),
        1
    );
    assert_signal_consumer_source_wired(
        "fn run_repl(",
        &[
            "TerminalSignalDisposition::InteractiveClean",
            "TerminalSignalKind::CleanExit",
        ],
    );
}

#[test]
fn repl_terminal_signal_wiring_does_not_insert_bounded_silence_into_inherited_stdio_path() {
    let source = main_rs_source();
    let body = source_block_after(&source, "fn run_repl(");
    assert!(
        !body.contains("execute_with_bounded_silence")
            && !body.contains("BoundedSilenceConfig")
            && !body.contains("bounded_silence_config"),
        "run_repl inherited-stdio path must not gain bounded-silence supervision"
    );
    assert_signal_consumer_source_wired(
        "fn run_repl(",
        &["terminal_signal", "emit_terminal_signal_marker"],
    );
}
