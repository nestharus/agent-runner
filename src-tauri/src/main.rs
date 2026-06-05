//! Oulipoly Plane agent runner entry point.
//!
//! ## Declared roles
//!
//! `orchestration`, `accessor`, `parser`, `predicate`, `mapper`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/main.rs::entrypoint_to_dispatch
//!     role: adapter
//!     Translates:
//!       - process argv and exit status into CLI dispatch invocation
//! ```

use clap::Parser;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

mod agent_resolution;
mod captured_child;
mod cli;
mod commands;
mod diagnostics_payloads;
mod dispatch;
mod error_emit;
mod invocation;
mod json_error;
mod mailbox_delivery;
mod migration_providers;
#[allow(dead_code)]
#[path = "main/owned_turn_event_ingest.rs"]
mod owned_turn_event_ingest;
mod quota_zero_turn;
mod redaction;
mod repl_cli;
mod resume_acceptance_adapter;
mod resume_cli;
mod run;
mod session_ingest_cli;
mod session_metadata_cli;
mod spawn_cwd;
mod terminal_outcome_adapter;
mod usage;
mod wake_coordinator;
mod wiring;
mod zero_turn_orchestration;

use crate::usage::cli::Cli;

fn main() -> ExitCode {
    process_entrypoint()
}

fn process_entrypoint() -> ExitCode {
    initialize_tracing();

    if should_run_gui() {
        return run_gui_entrypoint();
    }

    run_cli_entrypoint()
}

fn run_gui_entrypoint() -> ExitCode {
    agent_runner_lib::run_tauri();
    ExitCode::SUCCESS
}

fn run_cli_entrypoint() -> ExitCode {
    cli_exit_code(dispatch::run(parse_cli()))
}

fn initialize_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
}

fn should_run_gui() -> bool {
    arg_count(cli_args()) == 1
}

fn parse_cli() -> Cli {
    Cli::parse_from(crate::commands::resume_list::normalize_resume_list_args(
        cli_args(),
    ))
}

fn cli_args() -> std::env::Args {
    std::env::args()
}

fn arg_count(args: std::env::Args) -> usize {
    args.len()
}

fn cli_exit_code(result: Result<i32, String>) -> ExitCode {
    let exit = cli_exit(result);
    emit_cli_error_if_needed(&exit);
    cli_exit_to_code(&exit)
}

fn cli_exit_to_code(exit: &CliExit) -> ExitCode {
    match exit {
        CliExit::Success => ExitCode::SUCCESS,
        CliExit::Code(code) => ExitCode::from(*code as u8),
        CliExit::Error(_) => ExitCode::FAILURE,
    }
}

fn emit_cli_error_if_needed(exit: &CliExit) {
    if let CliExit::Error(error) = exit {
        emit_cli_error(error);
    }
}

enum CliExit {
    Success,
    Code(i32),
    Error(String),
}

fn cli_exit(result: Result<i32, String>) -> CliExit {
    match result {
        Ok(0) => CliExit::Success,
        Ok(code) => CliExit::Code(code),
        Err(error) => CliExit::Error(error),
    }
}

fn emit_cli_error(error: &str) {
    eprintln!("Error: {error}");
}
