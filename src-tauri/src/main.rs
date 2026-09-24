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
    if let Err(error) = kernel_entry::verify_installed_entry_route() {
        eprintln!("OULIPOLY_KERNEL_ENTRY_GAP={error}");
        return ExitCode::FAILURE;
    }
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
    #[cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
    if let Some(result) = private_installed_probe() {
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

#[cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
fn private_installed_probe() -> Option<ExitCode> {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    static WINCH: AtomicBool = AtomicBool::new(false);
    static INT: AtomicBool = AtomicBool::new(false);
    extern "C" fn winch(_: libc::c_int) {
        WINCH.store(true, Ordering::Relaxed);
    }
    extern "C" fn interrupt(_: libc::c_int) {
        INT.store(true, Ordering::Relaxed);
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().and_then(|arg| arg.to_str()) != Some("__age319-private-installed-probe-v1") {
        return None;
    }
    if (unsafe { libc::geteuid() }) != 0
        || !std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
    {
        return Some(ExitCode::FAILURE);
    }
    match args.get(1).and_then(|arg| arg.to_str()) {
        Some("tty") if args.len() == 2 => {
            unsafe {
                libc::signal(libc::SIGWINCH, winch as libc::sighandler_t);
                libc::signal(libc::SIGINT, interrupt as libc::sighandler_t);
            }
            let cwd = std::env::current_dir().unwrap();
            let mut size: libc::winsize = unsafe { std::mem::zeroed() };
            let tty = unsafe { libc::isatty(0) } == 1;
            let ctty =
                unsafe { libc::open(c"/dev/tty".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
            if ctty >= 0 {
                unsafe { libc::close(ctty) };
            }
            unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut size) };
            println!(
                "PRIVATE_TTY_READY cwd={} tty={tty} ctty={} rows={} cols={} display={}",
                cwd.display(),
                ctty >= 0,
                size.ws_row,
                size.ws_col,
                std::env::var("DISPLAY").unwrap_or_default()
            );
            std::io::stdout().flush().ok();
            let mut input = Vec::new();
            let mut byte = [0u8; 1];
            while input.len() < 128 {
                let read = unsafe { libc::read(0, byte.as_mut_ptr().cast(), 1) };
                if read == 1 {
                    if byte[0] == b'\n' {
                        break;
                    }
                    input.push(byte[0]);
                } else if read < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                    continue;
                } else {
                    break;
                }
            }
            println!(
                "PRIVATE_TTY_RESULT input={} winch={} int={}",
                String::from_utf8_lossy(&input),
                WINCH.load(Ordering::Relaxed),
                INT.load(Ordering::Relaxed)
            );
            Some(ExitCode::SUCCESS)
        }
        Some("ambient") if args.len() == 3 => {
            let marker = std::path::PathBuf::from(&args[2]);
            let child = unsafe { libc::fork() };
            if child < 0 {
                return Some(ExitCode::FAILURE);
            }
            if child == 0 {
                unsafe {
                    libc::setsid();
                    libc::clearenv();
                }
                let observed_pid = std::fs::read_to_string("/proc/self/status")
                    .ok()
                    .and_then(|status| {
                        status
                            .lines()
                            .find(|line| line.starts_with("NSpid:"))
                            .and_then(|line| line.split_whitespace().nth(1))
                            .and_then(|field| field.parse::<i32>().ok())
                    })
                    .unwrap_or(-1);
                let _ = std::fs::write(
                    &marker,
                    format!("ambient-descendant-alive host_pid={observed_pid}\n"),
                );
                for fd in 0..1024 {
                    unsafe { libc::close(fd) };
                }
                loop {
                    unsafe { libc::pause() };
                }
            }
            println!("PRIVATE_AMBIENT_PARENT_EXIT child={child}");
            Some(ExitCode::SUCCESS)
        }
        Some("setuid") if args.len() == 2 => {
            use std::os::fd::AsRawFd;
            let image = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
            let child = unsafe { libc::fork() };
            if child < 0 {
                return Some(ExitCode::FAILURE);
            }
            if child == 0 {
                let argv = [
                    c"oulipoly-agent-runner".as_ptr(),
                    c"__age319-private-installed-probe-v1".as_ptr(),
                    c"setuid-child".as_ptr(),
                    std::ptr::null(),
                ];
                let envp = [std::ptr::null::<libc::c_char>()];
                unsafe {
                    if libc::setresgid(1, 1, 1) != 0 || libc::setresuid(1, 1, 1) != 0 {
                        libc::_exit(71);
                    }
                    libc::syscall(
                        libc::SYS_execveat,
                        image.as_raw_fd(),
                        c"".as_ptr(),
                        argv.as_ptr(),
                        envp.as_ptr(),
                        libc::AT_EMPTY_PATH,
                    );
                    libc::_exit(72);
                }
            }
            let mut status = 0;
            if unsafe { libc::waitpid(child, &mut status, 0) } != child
                || !libc::WIFEXITED(status)
                || libc::WEXITSTATUS(status) != 0
            {
                eprintln!("PRIVATE_SETUID_CHILD_FAILED status={status}");
                return Some(ExitCode::FAILURE);
            }
            Some(ExitCode::SUCCESS)
        }
        Some("setuid-child") if args.len() == 2 => {
            let ruid = unsafe { libc::getuid() };
            let euid = unsafe { libc::geteuid() };
            let nnp = unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) };
            println!("PRIVATE_SETUID_CHILD ruid={ruid} euid={euid} nnp={nnp}");
            Some(if ruid == 1 && euid == 0 && nnp == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Some("sleep") if args.len() == 2 => {
            println!("PRIVATE_SLEEP_READY");
            std::io::stdout().flush().ok();
            loop {
                unsafe { libc::pause() };
            }
        }
        _ => Some(ExitCode::FAILURE),
    }
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
