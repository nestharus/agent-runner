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
mod completion_owner;
mod diagnostics_payloads;
mod dispatch;
mod error_emit;
mod invocation;
mod json_error;
#[cfg(target_os = "linux")]
mod kernel_entry;
mod mailbox_delivery;
mod maintenance_worker;
mod migration_providers;
mod native_receipt;
#[allow(dead_code)]
#[path = "main/owned_turn_event_ingest.rs"]
mod owned_turn_event_ingest;
mod provider_artifact;
mod provider_proof;
mod quota_zero_turn;
mod redaction;
mod repl_cli;
mod resume_acceptance_adapter;
mod resume_cli;
mod run;
mod session_ingest_cli;
mod session_metadata_cli;
mod session_turn_ingest_driver;
mod spawn_cwd;
mod terminal_outcome_adapter;
mod usage;
mod wake_coordinator;
mod wiring;
mod zero_turn_orchestration;

use crate::usage::cli::Cli;

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    let _installed_writer_admission = match kernel_entry::verify_installed_entry_route() {
        Ok(admission) => admission,
        Err(error) => {
            eprintln!("OULIPOLY_KERNEL_ENTRY_GAP={error}");
            return ExitCode::FAILURE;
        }
    };
    ordinary_entrypoint(production_entrypoint)
}

fn ordinary_entrypoint(run: impl FnOnce() -> ExitCode) -> ExitCode {
    run_with_event_sink_shutdown(run, oulipoly_state::shutdown_process_event_sink)
}

fn production_entrypoint() -> ExitCode {
    #[cfg(target_os = "linux")]
    if let Some(result) = kernel_entry::child_entry() {
        return result;
    }
    #[cfg(target_os = "linux")]
    if let Some(result) = kernel_entry::host_entry() {
        return result;
    }
    if maintenance_worker::is_worker_invocation() {
        return match maintenance_worker::run_worker_invocation() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                let fallback = maintenance_worker::record_worker_failure_fallback(&error);
                eprintln!("OULIPOLY_MAINTENANCE_GAP=worker_failed:{error}");
                if let Err(fallback_error) = fallback {
                    eprintln!(
                        "OULIPOLY_MAINTENANCE_GAP=worker_failure_fallback_failed:{fallback_error}"
                    );
                }
                ExitCode::FAILURE
            }
        };
    }
    #[cfg(target_os = "linux")]
    if let Some(result) = completion_owner::custodian_entry() {
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    process_entrypoint()
}

fn run_with_event_sink_shutdown(
    run: impl FnOnce() -> ExitCode,
    shutdown: impl FnOnce() -> Result<(), String>,
) -> ExitCode {
    let exit = run();
    if let Err(error) = shutdown() {
        eprintln!("OULIPOLY_EVENT_STORE_GAP=graceful_shutdown_failed:{error}");
    }
    exit
}

fn process_entrypoint() -> ExitCode {
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new(native_receipt::helper::ARG))
    {
        let target = match std::env::args()
            .nth(3)
            .map(|value| serde_json::from_str(&value))
            .transpose()
        {
            Ok(target) => target,
            Err(error) => {
                eprintln!("invalid receipt target: {error}");
                return ExitCode::FAILURE;
            }
        };
        return match native_receipt::helper::entry_target(
            std::env::args_os().nth(2).as_deref() == Some(std::ffi::OsStr::new("once")),
            target,
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    if wake_coordinator::is_wake_reclaim_handoff_invocation() {
        return cli_exit_to_code(&cli_exit(
            wake_coordinator::run_wake_reclaim_handoff_invocation().map(|()| 0),
        ));
    }

    if should_run_gui() {
        return run_gui_entrypoint();
    }

    run_cli_entrypoint()
}

fn run_gui_entrypoint() -> ExitCode {
    schedule_entrypoint_opportunity(
        None,
        maintenance_worker::schedule_daily_opportunity_fail_open,
    );
    initialize_tracing();
    agent_runner_lib::run_tauri();
    ExitCode::SUCCESS
}

fn run_cli_entrypoint() -> ExitCode {
    let cli = parse_cli();
    schedule_entrypoint_opportunity(
        Some(&cli),
        maintenance_worker::schedule_daily_opportunity_fail_open,
    );
    let result = dispatch::run_offline_entry(&cli)
        .and_then(|offline_exit| {
            if let Some(code) = offline_exit {
                return Ok(Some(code));
            }
            dispatch::preflight_entry(&cli)
        })
        .and_then(|early_exit| {
            if let Some(code) = early_exit {
                return Ok(code);
            }
            if let Err(error) = completion_owner::bootstrap(&cli) {
                return dispatch::entry_bootstrap_error(&cli, error);
            }
            if let Some(code) = dispatch::validate_owned_entry(&cli)? {
                return Ok(code);
            }
            initialize_tracing();
            dispatch::run(cli)
        });
    let exit = cli_exit(result);
    emit_cli_error_if_needed(&exit);
    cli_exit_to_code(&exit)
}

/// This is the common production placement boundary: opportunity admission is
/// attempted after argv parsing but before offline dispatch, owner bootstrap,
/// runtime construction, or the GUI event loop.
fn schedule_entrypoint_opportunity(
    cli: Option<&Cli>,
    schedule: impl FnOnce(maintenance_worker::ScheduleBasis),
) {
    match cli {
        None => schedule(maintenance_worker::ScheduleBasis::GuiStartup),
        Some(cli) if maintenance_worker::cli_requests_opportunity(cli) => {
            schedule(maintenance_worker::ScheduleBasis::ProviderStartup);
        }
        Some(_) => {}
    }
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

#[cfg(test)]
mod maintenance_entrypoint_tests {
    use super::*;

    #[test]
    fn common_entrypoint_placement_fixture_schedules_gui_and_provider_before_bootstrap_only() {
        let provider = Cli::try_parse_from(["runner", "--model", "test"]).unwrap();
        let diagnostics = Cli::try_parse_from([
            "runner",
            "diagnostics",
            "maintenance",
            "--kind",
            "event_discovery",
            "--partition",
            "event-store-v1",
        ])
        .unwrap();
        let maintenance = Cli::try_parse_from([
            "runner",
            "maintenance",
            "status",
            "--kind",
            "event_discovery",
            "--partition",
            "event-store-v1",
        ])
        .unwrap();

        #[derive(Debug, PartialEq, Eq)]
        enum Step {
            Scheduled(maintenance_worker::ScheduleBasis),
            Bootstrap,
        }
        let mut observed = Vec::new();
        schedule_entrypoint_opportunity(None, |basis| observed.push(Step::Scheduled(basis)));
        schedule_entrypoint_opportunity(Some(&provider), |basis| {
            observed.push(Step::Scheduled(basis))
        });
        schedule_entrypoint_opportunity(Some(&diagnostics), |basis| {
            observed.push(Step::Scheduled(basis))
        });
        schedule_entrypoint_opportunity(Some(&maintenance), |basis| {
            observed.push(Step::Scheduled(basis))
        });
        observed.push(Step::Bootstrap);

        assert_eq!(
            observed,
            [
                Step::Scheduled(maintenance_worker::ScheduleBasis::GuiStartup),
                Step::Scheduled(maintenance_worker::ScheduleBasis::ProviderStartup),
                Step::Bootstrap,
            ]
        );
    }

    #[test]
    fn ordinary_entrypoint_runs_event_sink_shutdown_after_work_and_preserves_exit() {
        use std::cell::RefCell;

        let observed = RefCell::new(Vec::new());
        let exit = run_with_event_sink_shutdown(
            || {
                observed.borrow_mut().push("run");
                ExitCode::FAILURE
            },
            || {
                observed.borrow_mut().push("shutdown");
                Ok(())
            },
        );

        assert_eq!(observed.into_inner(), ["run", "shutdown"]);
        assert_eq!(exit, ExitCode::FAILURE);
    }

    #[test]
    fn ordinary_entrypoint_closes_installed_global_sink_exactly_once() {
        const CHILD_ROOT: &str = "OULIPOLY_AGE374_ENTRYPOINT_FIXTURE";
        const TEST_NAME: &str = "maintenance_entrypoint_tests::ordinary_entrypoint_closes_installed_global_sink_exactly_once";

        if let Some(data_root) = std::env::var_os(CHILD_ROOT) {
            use oulipoly_state::event_store::{
                GenerationState, WriterLayout, parse_id_hex, read_generation_metadata,
                read_prepared_manifest,
            };
            use rusqlite::{Connection, OpenFlags};

            let data_root = std::path::PathBuf::from(data_root);
            let exit = ordinary_entrypoint(|| {
                let _ = oulipoly_state::diagnostic_recorder::process_recorder();
                ExitCode::FAILURE
            });
            assert_eq!(exit, ExitCode::FAILURE);

            let event_root = data_root.join("diagnostics/event-store-v1");
            let writer_dirs: Vec<_> = std::fs::read_dir(event_root.join("writers"))
                .unwrap()
                .map(|entry| entry.unwrap())
                .collect();
            assert_eq!(writer_dirs.len(), 1);
            let writer_bytes = parse_id_hex(writer_dirs[0].file_name().to_str().unwrap()).unwrap();
            let layout = WriterLayout::open_existing(&event_root, writer_bytes).unwrap();
            let selected = layout.read_head(|_, _| Ok(())).unwrap().unwrap();
            let selected_manifest =
                read_prepared_manifest(&layout.generation_dir(selected.record.generation_id))
                    .unwrap();
            let predecessor = selected_manifest
                .predecessor_generation_id
                .expect("ordinary entrypoint shutdown must rotate the installed exact writer");
            let predecessor_connection = Connection::open_with_flags(
                layout.generation_db(predecessor),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .unwrap();
            let predecessor_metadata = read_generation_metadata(&predecessor_connection).unwrap();
            assert_eq!(predecessor_metadata.state, GenerationState::Closed);
            assert_eq!(
                predecessor_metadata.successor_generation_id,
                Some(selected.record.generation_id)
            );

            let closed_head = selected.record;
            oulipoly_state::shutdown_process_event_sink().unwrap();
            assert_eq!(
                layout.read_head(|_, _| Ok(())).unwrap().unwrap().record,
                closed_head
            );
            assert_eq!(
                std::fs::read_dir(layout.generations_dir()).unwrap().count(),
                2
            );
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ROOT, root.path())
            .env("OULIPOLY_DATA_DIR", root.path())
            .env("OULIPOLY_CONFIG_HOME", root.path().join("config"))
            .status()
            .unwrap();
        assert!(
            status.success(),
            "entrypoint fixture child failed: {status}"
        );
    }
}
