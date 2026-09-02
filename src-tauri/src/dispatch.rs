//! Top-level CLI routing and dispatch.
//!
//! ## Declared roles
//!
//! `orchestration`, `parser`, `validator`, `accessor`, `formatter`, `mapper`, `predicate`, `filter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/dispatch.rs::dispatch_to_runtime_services
//!     role: adapter
//!     Translates:
//!       - oulipoly_runtime::services ports and DTOs consumed by CLI dispatch
//!   - component: src-tauri/src/dispatch.rs::dispatch_to_state
//!     role: adapter
//!     Translates:
//!       - oulipoly_state DB, schema, quota, invocation row, session row, acceptance row
//!   - component: src-tauri/src/dispatch.rs::dispatch_to_runtime_modules
//!     role: adapter
//!     Translates:
//!       - oulipoly_runtime modules outside services consumed by CLI dispatch
//!   - component: src-tauri/src/dispatch.rs::dispatch_to_cli_application
//!     role: adapter
//!     Translates:
//!       - src-tauri CLI input resolution and resume-error formatting contract
//!       - src-tauri public usage command DTO contract
//!       - src-tauri commands, run, usage, and wiring composition contract
//! ```
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/dispatch.rs
//!     role: intrinsic-surface
//!     Domain: cli_lifecycle_orchestration
//!     Owns:
//!       - lifecycle loops
//!       - run_with_balancing lifecycle loop
//!       - run_resume lifecycle loop
//!       - run_repl lifecycle loop
//!       - top-level --resume dispatch
//!       - invocation finalization sequencing
//!       - terminal signal outcome sequencing
//!       - provider retry and migration sequencing
//!   - component: src-tauri/src/dispatch.rs::pre_invocation_failure_marker
//!     role: intrinsic-surface
//!     Domain: pre_invocation_failure_marker
//!     Owns:
//!       - emit_pre_invocation_failure
//!       - pre_invocation_failure_payload
//!       - pre_invocation_failure_message
//!       - emit_pool_exhausted_pre_invocation_failure
//!       - emit_provider_selection_pre_invocation_failure
//!       - emit_provider_resolution_pre_invocation_failure
//!       - OULIPOLY_FAILURE marker line
//!       - OULIPOLY_FAILURE payload field set
//!       - model_provider_names
//! ```

use oulipoly_runtime::{session_external_provider, session_replace};

use crate::cli::inputs::{
    TopLevelResumePromptSource, format_missing_prompt_error, resolve_top_level_resume_prompt_source,
};
use crate::resume_cli::format_resume_error;
use crate::usage::cli::{
    Cli, MailboxSubcommands, NotifySubcommands, SessionSubcommands, Subcommands,
};
use crate::{commands, run, usage, wiring};

mod clock;
mod formatter;
mod parent_invocation;
mod parser;
mod pre_invocation_failure;
mod predicate;
mod usage_context;

pub(crate) use clock::utc_now;
pub(crate) use formatter::format_timestamp_rfc3339;
pub(crate) use parent_invocation::resolve_parent_invocation_id;
pub(crate) use parser::provider_session_marker_uuid;
use parser::{resume_target_arg, resume_target_kind};
pub(crate) use pre_invocation_failure::emit_pre_invocation_failure;
pub(crate) use predicate::{
    diagnostics_model_configured, execution_succeeded, should_emit_resume_short_line,
};

pub(crate) fn run(cli: Cli) -> Result<i32, String> {
    // Keep read-only session inspection ahead of startup recovery and provider dispatch.
    if let Some(Subcommands::Session { command }) = &cli.command
        && let Some(result) = dispatch_inspection_only_session(command)
    {
        return result;
    }

    // Maintenance must not trigger wake recovery before it inspects or compacts storage.
    if let Some(Subcommands::Mailbox { command }) = &cli.command
        && matches!(
            command,
            MailboxSubcommands::CompactDelivered { .. } | MailboxSubcommands::PruneTerminal { .. }
        )
    {
        return dispatch_mailbox_subcommand(command.clone());
    }

    if let Err(err) = recover_pending_session_replaces() {
        return Ok(handle_pending_session_replace_error(&err));
    }

    // Config and database migration own their loading and error semantics. Do
    // not preflight the runtime provider registry before they can inspect or
    // repair malformed historical configuration.
    if let Some(command) = cli.command.clone()
        && let Some(result) = dispatch_registry_independent_subcommand(command)
    {
        return result;
    }
    let _startup_wake_reclaim_guard = if startup_wake_reclaim_sweep_enabled(&cli) {
        if provider_launch_schedules_startup_wake_reclaim(&cli) {
            crate::wake_coordinator::start_startup_wake_reclaim_sweep()
        } else {
            crate::wake_coordinator::run_startup_wake_reclaim_sweep();
            None
        }
    } else {
        None
    };

    if cli.new {
        return run_default_provider_repl(&cli);
    }

    let agent_runtime_services = match wiring::AgentRuntimeServices::cli_defaults() {
        Ok(services) => services,
        Err(error) => return handle_runtime_service_initialization_error(&cli, error),
    };
    let _session_turn_ingest_driver =
        session_turn_ingest_driver_enabled(&cli, &agent_runtime_services.provider_registry_handle)
            .then(|| {
                crate::session_turn_ingest_driver::start_session_turn_ingest_driver(
                    agent_runtime_services.provider_registry_handle.clone(),
                )
            })
            .flatten();

    if let Err(err) = recover_pending_provider_owned_session_replaces(&agent_runtime_services) {
        return Ok(handle_pending_session_replace_error(&err));
    }

    if cli.usage {
        return run_usage_command(&cli, &agent_runtime_services);
    }

    if let Some(command) = cli.command.clone() {
        return dispatch_subcommand(command, &agent_runtime_services);
    }

    if let Some(ref session_id) = cli.resume {
        return dispatch_top_level_resume(&cli, session_id, &agent_runtime_services);
    }

    if let Some(ref model_name) = cli.model {
        return crate::commands::direct_model::run_direct_model_cli(
            &cli,
            model_name,
            &agent_runtime_services,
        );
    }

    crate::commands::direct_model::run_agent_cli(&cli, &agent_runtime_services)
}

fn dispatch_registry_independent_subcommand(command: Subcommands) -> Option<Result<i32, String>> {
    match command {
        Subcommands::MigrateDb => Some(commands::migrate::run_migrate_db()),
        Subcommands::Migrate { rebuild } => Some(commands::migrate::run_migrate(rebuild)),
        Subcommands::MigrateConfig { models_dir } => Some(
            commands::config_migration::run_migrate_config(models_dir.as_deref()),
        ),
        _ => None,
    }
}

fn handle_runtime_service_initialization_error(cli: &Cli, error: String) -> Result<i32, String> {
    if let Some(model_name) = cli.model.as_deref() {
        crate::commands::direct_model::validate_direct_model_cli_context(cli, model_name)?;
    }
    if matches!(
        cli.command,
        Some(Subcommands::Session {
            command: SessionSubcommands::ImportReplace { .. }
        })
    ) {
        return crate::commands::session_import_replace::render_import_replace_output(Err(
            session_replace::ReplaceError::OperationalError { message: error },
        ));
    }
    Err(error)
}

fn dispatch_inspection_only_session(command: &SessionSubcommands) -> Option<Result<i32, String>> {
    match command {
        SessionSubcommands::List { json } => {
            Some(crate::commands::session_list::run_session_list(*json))
        }
        SessionSubcommands::OfPid { pid, json } => {
            Some(crate::commands::pid_session::run_of_pid(*pid, *json))
        }
        SessionSubcommands::Alive { pid, json } => {
            Some(crate::commands::pid_session::run_alive(*pid, *json))
        }
        SessionSubcommands::Subtree {
            pid,
            json,
            max_depth,
        } => Some(crate::commands::pid_session::run_subtree(
            *pid, *json, *max_depth,
        )),
        _ => None,
    }
}

fn startup_wake_reclaim_sweep_enabled(cli: &Cli) -> bool {
    if cli.resume.is_some() {
        return false;
    }
    !matches!(
        &cli.command,
        Some(Subcommands::Notify { .. })
            | Some(Subcommands::Resume { .. })
            | Some(Subcommands::Repl {
                resume: Some(_),
                ..
            })
    )
}

fn provider_launch_schedules_startup_wake_reclaim(cli: &Cli) -> bool {
    if cli.usage {
        return false;
    }
    cli.command.is_none() || matches!(&cli.command, Some(Subcommands::Repl { resume: None, .. }))
}

fn session_turn_ingest_driver_enabled(
    cli: &Cli,
    registry: &oulipoly_runtime::provider_registry::ProviderRegistryHandle,
) -> bool {
    if cli.usage || registry.current().configured_artifact_keys().is_empty() {
        return false;
    }
    matches!(
        &cli.command,
        None | Some(Subcommands::Repl { .. })
            | Some(Subcommands::Resume { .. })
            | Some(Subcommands::Session {
                command: SessionSubcommands::Import { .. }
            })
    )
}

fn recover_pending_session_replaces() -> Result<(), session_replace::ReplaceError> {
    session_replace::recover_pending_replaces()
}

fn handle_pending_session_replace_error(err: &session_replace::ReplaceError) -> i32 {
    emit_pending_session_replace_error(err);
    pending_session_replace_exit_code(err)
}

fn recover_pending_provider_owned_session_replaces(
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<(), session_replace::ReplaceError> {
    session_external_provider::recover_pending_provider_owned_replaces(
        agent_runtime_services.provider_registry_handle.clone(),
    )
}

fn emit_pending_session_replace_error(err: &session_replace::ReplaceError) {
    formatter::emit_stderr_line(&err.to_json().to_string());
}

fn pending_session_replace_exit_code(err: &session_replace::ReplaceError) -> i32 {
    err.exit_code()
}

fn run_default_provider_repl(cli: &Cli) -> Result<i32, String> {
    crate::wake_coordinator::start_wake_reclaim_maintenance_driver();
    run_default_provider_repl_for_project(cli.project.clone())
}

fn run_default_provider_repl_for_project(
    project: Option<std::path::PathBuf>,
) -> Result<i32, String> {
    let services = oulipoly_runtime::repl_default_provider::RuntimeServices::production(project)?;
    oulipoly_runtime::repl_default_provider::run_repl_with_default_provider(
        services,
        |invocation| crate::wake_coordinator::admit_session_launch(&invocation.id, None),
    )
}

fn run_usage_command(
    cli: &Cli,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    let context = usage_context::load_usage_context(cli)?;
    let mut stdout = std::io::stdout().lock();
    usage::dispatch::run_usage(
        agent_runtime_services,
        &context.providers_cfg,
        &context.models,
        &mut stdout,
    )
}

fn dispatch_subcommand(
    command: Subcommands,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    match command {
        Subcommands::Trace {
            invocation_uuid,
            json,
            inline_transcript,
            transcript,
            max_depth,
        } => dispatch_trace_subcommand(
            &invocation_uuid,
            json,
            inline_transcript,
            transcript,
            max_depth,
            agent_runtime_services,
        ),
        Subcommands::Repl {
            model,
            resume,
            rotate_provider,
            project,
            models_dir,
        } => {
            if model.is_none() && resume.is_none() && rotate_provider.is_none() {
                run_default_provider_repl_for_project(project)
            } else {
                run::run_repl(
                    agent_runtime_services,
                    model.as_deref(),
                    resume.as_deref(),
                    rotate_provider.as_deref(),
                    project.as_deref(),
                    models_dir.as_deref(),
                )
            }
        }
        Subcommands::Resume {
            model,
            session_id,
            chain_id,
            rotate_provider,
            prompt,
            file,
            submission_token,
            project,
            models_dir,
        } => dispatch_resume_subcommand(
            ResumeDispatchArgs {
                model: model.as_deref(),
                session_id: session_id.as_deref(),
                chain_id: chain_id.as_deref(),
                rotate_provider: rotate_provider.as_deref(),
                prompt: prompt.as_deref(),
                file: file.as_deref(),
                submission_token: submission_token.as_deref(),
                project: project.as_deref(),
                models_dir: models_dir.as_deref(),
            },
            agent_runtime_services,
        ),
        Subcommands::Session { command } => {
            dispatch_session_subcommand(command, agent_runtime_services)
        }
        Subcommands::Notify { command } => dispatch_notify_subcommand(command),
        Subcommands::Mailbox { command } => dispatch_mailbox_subcommand(command),
        Subcommands::ResumeList { uuid } => crate::commands::resume_list::run_resume_list(&uuid),
        Subcommands::MigrateDb => commands::migrate::run_migrate_db(),
        Subcommands::MigrateSessionOwnership {
            dry_run,
            apply,
            rollback,
            corrective,
            scratch_dir,
            backup_dir,
            confirm_mutate_live_db,
            skip_provider_proof,
            confirm_skip_provider_proof,
            state_db,
            models_dir,
        } => commands::migrate::run_migrate_session_ownership(
            commands::migrate::MigrateSessionOwnershipArgs {
                dry_run,
                apply,
                rollback,
                corrective,
                scratch_dir: scratch_dir.as_deref(),
                backup_dir: backup_dir.as_deref(),
                confirm_mutate_live_db,
                skip_provider_proof,
                confirm_skip_provider_proof,
                state_db: state_db.as_deref(),
                models_dir: models_dir.as_deref(),
            },
        ),
        Subcommands::Migrate { rebuild } => commands::migrate::run_migrate(rebuild),
        Subcommands::MigrateConfig { models_dir } => {
            commands::config_migration::run_migrate_config(models_dir.as_deref())
        }
    }
}

fn dispatch_notify_subcommand(command: NotifySubcommands) -> Result<i32, String> {
    match command {
        NotifySubcommands::Register {
            handle,
            delivery_mode,
            state_dir,
            meta,
            log,
            rc,
            repair_admitted,
            json,
        } => crate::commands::notify::run_agent_bash_register(
            crate::commands::notify::AgentBashRegisterArgs {
                handle: &handle,
                delivery_mode: &delivery_mode,
                state_dir: &state_dir,
                meta: &meta,
                log: &log,
                rc: &rc,
                repair_admitted,
                json,
            },
        ),
        NotifySubcommands::Activate { handle, json } => {
            crate::commands::notify::run_agent_bash_activate(
                crate::commands::notify::AgentBashActivateArgs {
                    handle: &handle,
                    json,
                },
            )
        }
        NotifySubcommands::Complete {
            caller_ppid,
            handle,
            state_dir,
            meta,
            log,
            rc,
            consumed,
            json,
        } => crate::commands::notify::run_agent_bash_complete(
            crate::commands::notify::AgentBashCompleteArgs {
                caller_ppid,
                handle: &handle,
                state_dir: &state_dir,
                meta: &meta,
                log: &log,
                rc: &rc,
                consumed,
                json,
            },
        ),
    }
}

fn dispatch_mailbox_subcommand(command: MailboxSubcommands) -> Result<i32, String> {
    match command {
        MailboxSubcommands::List {
            session_id,
            all,
            json,
        } => crate::commands::mailbox::run_list(&session_id, all, json),
        MailboxSubcommands::Search {
            session_id,
            query,
            all,
            limit,
            json,
        } => crate::commands::mailbox::run_search(&session_id, &query, all, limit, json),
        MailboxSubcommands::Show {
            session_id,
            seq,
            handle,
            include_artifacts,
            max_bytes,
            json,
        } => crate::commands::mailbox::run_show(
            &session_id,
            seq,
            handle.as_deref(),
            include_artifacts,
            max_bytes,
            json,
        ),
        MailboxSubcommands::Status { session_id, json } => {
            crate::commands::mailbox::run_status(&session_id, json)
        }
        MailboxSubcommands::Pause { session_id, json } => {
            crate::commands::mailbox::run_pause(&session_id, true, json)
        }
        MailboxSubcommands::Resume { session_id, json } => {
            crate::commands::mailbox::run_pause(&session_id, false, json)
        }
        MailboxSubcommands::Ack {
            session_id,
            from_seq,
            to_seq,
            delivered_by,
            json,
        } => crate::commands::mailbox::run_ack(&session_id, from_seq, to_seq, &delivered_by, json),
        MailboxSubcommands::CompactDelivered { limit, apply, json } => {
            crate::commands::mailbox::run_compact_delivered(limit, apply, json)
        }
        MailboxSubcommands::PruneTerminal {
            limit,
            apply,
            vacuum,
            json,
        } => crate::commands::mailbox::run_prune_terminal(limit, apply, vacuum, json),
    }
}

fn dispatch_trace_subcommand(
    invocation_uuid: &str,
    json: bool,
    inline_transcript: bool,
    transcript: bool,
    max_depth: usize,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    commands::trace::run_trace_command(
        commands::trace::trace_options(max_depth, json, inline_transcript, transcript),
        invocation_uuid,
        agent_runtime_services,
    )
}

struct ResumeDispatchArgs<'a> {
    model: Option<&'a str>,
    session_id: Option<&'a str>,
    chain_id: Option<&'a str>,
    rotate_provider: Option<&'a str>,
    prompt: Option<&'a str>,
    file: Option<&'a std::path::Path>,
    submission_token: Option<&'a str>,
    project: Option<&'a std::path::Path>,
    models_dir: Option<&'a std::path::Path>,
}

fn dispatch_resume_subcommand(
    args: ResumeDispatchArgs<'_>,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    run::run_resume(
        agent_runtime_services,
        args.model,
        resume_target_arg(args.session_id, args.chain_id),
        resume_target_kind(args.session_id, args.chain_id),
        args.rotate_provider,
        args.prompt,
        args.file,
        args.submission_token,
        args.project,
        args.models_dir,
    )
}

fn dispatch_session_subcommand(
    command: SessionSubcommands,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    match command {
        SessionSubcommands::List { json } => crate::commands::session_list::run_session_list(json),
        SessionSubcommands::Import {
            provider,
            limit,
            since_unix_ms,
            backfill_turns,
            json,
        } => crate::commands::session_import::run_session_import(
            crate::commands::session_import::SessionImportCliArgs {
                provider: provider.as_deref(),
                limit,
                since_unix_ms,
                backfill_turns,
                json,
            },
            agent_runtime_services,
        ),
        SessionSubcommands::Locate { session_id, json } => {
            dispatch_session_locate_subcommand(&session_id, json)
        }
        SessionSubcommands::SchemaProbe => {
            crate::commands::schema_probe::run_session_schema_probe()
        }
        SessionSubcommands::Export { session_id, format } => {
            dispatch_session_export_subcommand(&session_id, &format, agent_runtime_services)
        }
        SessionSubcommands::PauseHandshake { session_id, ttl_ms } => {
            crate::commands::handshake::run_pause_handshake(
                &session_id,
                ttl_ms,
                agent_runtime_services,
            )
        }
        SessionSubcommands::OfPid { pid, json } => {
            crate::commands::pid_session::run_of_pid(pid, json)
        }
        SessionSubcommands::Alive { pid, json } => {
            crate::commands::pid_session::run_alive(pid, json)
        }
        SessionSubcommands::Subtree {
            pid,
            json,
            max_depth,
        } => crate::commands::pid_session::run_subtree(pid, json, max_depth),
        SessionSubcommands::ResumeHandshake { session_id, token } => {
            crate::commands::handshake::run_resume_handshake(
                &session_id,
                &token,
                agent_runtime_services,
            )
        }
        SessionSubcommands::ImportReplace {
            session_id,
            from_file,
            preimage_sha256,
        } => crate::commands::session_import_replace::run_session_import_replace(
            &session_id,
            from_file.as_deref(),
            preimage_sha256.as_deref(),
            agent_runtime_services,
        ),
    }
}

fn dispatch_session_locate_subcommand(session_id: &str, json: bool) -> Result<i32, String> {
    commands::session_locate_export::run_session_locate(session_id, json)
}

fn dispatch_session_export_subcommand(
    session_id: &str,
    format: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    commands::session_locate_export::run_session_export(session_id, format, agent_runtime_services)
}

fn dispatch_top_level_resume(
    cli: &Cli,
    session_id: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    validate_top_level_resume_cli(cli)?;
    if let Some(request_path) = cli.fresh_continuation_request.as_deref() {
        let prompt_source = resolve_top_level_resume_prompt_source(cli)?;
        let prompt = continuation_command_prompt(&prompt_source)?;
        return run::continuation_command::run(
            agent_runtime_services,
            cli.model.as_deref(),
            session_id,
            oulipoly_state::InboxTargetKind::Session,
            prompt,
            cli.file.as_deref(),
            cli.submission_token.as_deref(),
            cli.project.as_deref(),
            cli.models_dir.as_deref(),
            request_path,
        );
    }
    match resolve_top_level_resume_prompt_source(cli)? {
        TopLevelResumePromptSource::Headless {
            positional_or_stdin_prompt,
        } => dispatch_headless_top_level_resume(
            cli,
            session_id,
            positional_or_stdin_prompt.as_deref(),
            agent_runtime_services,
        ),
        TopLevelResumePromptSource::Interactive => {
            dispatch_interactive_top_level_resume(cli, session_id, agent_runtime_services)
        }
    }
}

fn continuation_command_prompt(
    prompt_source: &TopLevelResumePromptSource,
) -> Result<Option<&str>, String> {
    match prompt_source {
        TopLevelResumePromptSource::Headless {
            positional_or_stdin_prompt,
        } => Ok(positional_or_stdin_prompt.as_deref()),
        TopLevelResumePromptSource::Interactive => Err(format_missing_prompt_error()),
    }
}

fn dispatch_headless_top_level_resume(
    cli: &Cli,
    session_id: &str,
    positional_or_stdin_prompt: Option<&str>,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    run::run_resume(
        agent_runtime_services,
        cli.model.as_deref(),
        session_id,
        oulipoly_state::InboxTargetKind::Session,
        cli.rotate_provider.as_deref(),
        positional_or_stdin_prompt,
        cli.file.as_deref(),
        cli.submission_token.as_deref(),
        cli.project.as_deref(),
        cli.models_dir.as_deref(),
    )
}

fn dispatch_interactive_top_level_resume(
    cli: &Cli,
    session_id: &str,
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> Result<i32, String> {
    run::run_repl(
        agent_runtime_services,
        cli.model.as_deref(),
        Some(session_id),
        cli.rotate_provider.as_deref(),
        cli.project.as_deref(),
        cli.models_dir.as_deref(),
    )
}

fn validate_top_level_resume_cli(cli: &Cli) -> Result<(), String> {
    if cli.agent_file.is_some() {
        Err(formatter::format_resume_agent_file_incompatible_error())
    } else {
        Ok(())
    }
}

pub(crate) fn render_resume_error(error: oulipoly_state::ResumeError) {
    formatter::emit_stderr_line(&format_resume_error(error));
}

pub(crate) fn resume_session_mismatch_category() -> String {
    oulipoly_runtime::diagnostics::ErrorCategory::ResumeSessionMismatch
        .as_str()
        .to_string()
}

pub(crate) fn diagnose_execution_error(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    result: &oulipoly_runtime::executor::ExecutionResult,
    models: &std::collections::HashMap<String, oulipoly_config::ModelConfig>,
    working_dir: Option<&std::path::Path>,
) -> Option<String> {
    let input = crate::redaction::diagnostic_input(&result.stderr, &result.stdout);
    crate::commands::diagnostics::run_diagnostics(
        agent_runtime_services,
        &input,
        result.exit_code,
        models,
        working_dir,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use oulipoly_state::{CompositeInvocationId, InvocationStart, StateDb};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};
    use uuid::Uuid;

    const REPL_MODEL: &str = "fixture-model";

    trait ResumeSessionField {
        fn as_optional_str(&self) -> Option<&str>;
    }

    impl ResumeSessionField for String {
        fn as_optional_str(&self) -> Option<&str> {
            Some(self.as_str())
        }
    }

    impl ResumeSessionField for Option<String> {
        fn as_optional_str(&self) -> Option<&str> {
            self.as_deref()
        }
    }

    fn resume_session_field_as_deref(field: &impl ResumeSessionField) -> Option<&str> {
        field.as_optional_str()
    }

    fn parse_resume_subcommand<const N: usize>(argv: [&str; N]) -> Subcommands {
        Cli::try_parse_from(argv)
            .unwrap()
            .command
            .expect("resume argv should produce a subcommand")
    }

    #[test]
    fn agent_bash_registration_skips_startup_wake_reclaim_sweep() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "notify",
            "agent-bash-register",
            "--handle",
            "ab_test",
            "--delivery-mode",
            "sync",
            "--state-dir",
            "/tmp/ab_test",
            "--meta",
            "/tmp/ab_test/meta.json",
            "--log",
            "/tmp/ab_test/log",
            "--rc",
            "/tmp/ab_test/rc",
        ])
        .unwrap();

        assert!(!startup_wake_reclaim_sweep_enabled(&cli));
    }

    #[test]
    fn provider_launch_schedules_startup_recovery_but_mailbox_inspection_does_not() {
        let launch = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "-m",
            "fixture-model",
            "fixture prompt",
        ])
        .unwrap();
        let mailbox = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "mailbox",
            "list",
            "--session-id",
            "fixture-session",
        ])
        .unwrap();

        assert!(provider_launch_schedules_startup_wake_reclaim(&launch));
        assert!(!provider_launch_schedules_startup_wake_reclaim(&mailbox));
    }

    fn assert_resume_debug_contains_option_field(
        command: Subcommands,
        field_name: &str,
        expected: &str,
    ) {
        assert_resume_subcommand(&command);

        let rendered = render_subcommand_debug(&command);
        let expected_fragment = expected_option_field_fragment(field_name, expected);
        assert!(
            rendered.contains(&expected_fragment),
            "expected `{expected_fragment}` in parsed Resume variant: {rendered}"
        );
    }

    fn assert_resume_subcommand(command: &Subcommands) {
        match command {
            Subcommands::Resume { .. } => {}
            _ => panic!("expected resume subcommand"),
        }
    }

    fn render_subcommand_debug(command: &Subcommands) -> String {
        format!("{command:?}")
    }

    fn expected_option_field_fragment(field_name: &str, expected: &str) -> String {
        format!("{field_name}: Some(\"{expected}\")")
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_parent_invocation_env(value: Option<&str>, test: impl FnOnce()) {
        let _guard = env_lock().lock().unwrap();
        let previous = read_parent_invocation_env();
        set_parent_invocation_env(value);

        let result = catch_unwind(AssertUnwindSafe(test));

        restore_parent_invocation_env(previous);

        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    fn read_parent_invocation_env() -> Option<std::ffi::OsString> {
        std::env::var_os("OULIPOLY_PARENT_INVOCATION")
    }

    fn set_parent_invocation_env(value: Option<&str>) {
        match value {
            Some(value) => set_parent_invocation_env_value(value),
            None => remove_parent_invocation_env(),
        }
    }

    fn restore_parent_invocation_env(value: Option<std::ffi::OsString>) {
        match value {
            Some(value) => set_parent_invocation_env_os_value(value),
            None => remove_parent_invocation_env(),
        }
    }

    fn set_parent_invocation_env_value(value: &str) {
        unsafe {
            std::env::set_var("OULIPOLY_PARENT_INVOCATION", value);
        }
    }

    fn set_parent_invocation_env_os_value(value: std::ffi::OsString) {
        unsafe {
            std::env::set_var("OULIPOLY_PARENT_INVOCATION", value);
        }
    }

    fn remove_parent_invocation_env() {
        unsafe {
            std::env::remove_var("OULIPOLY_PARENT_INVOCATION");
        }
    }

    fn test_db() -> StateDb {
        open_test_db(Path::new(":memory:"))
    }

    fn open_test_db(path: &Path) -> StateDb {
        StateDb::open(path).unwrap()
    }

    #[test]
    fn no_subcommand_still_parses_existing_model_flow() {
        // `agent` is the first positional in `Cli`, so a single bare arg
        // (`ping`) lands there — `prompt_args` only captures any *additional*
        // trailing args. The existing `collect_positional_prompt(_, true)`
        // path joins agent + prompt_args, so the runtime prompt still
        // becomes "ping". Exercised end-to-end by the integration test
        // `default_cli_flow_still_runs_without_subcommand`.
        let cli =
            Cli::try_parse_from(["oulipoly-agent-runner", "--model", "fixture", "ping"]).unwrap();

        assert!(cli.command.is_none());
        assert_eq!(cli.model.as_deref(), Some("fixture"));
        assert_eq!(cli.agent.as_deref(), Some("ping"));
        assert!(cli.prompt_args.is_empty());

        // Two trailing args: the first goes to `agent`, the rest into
        // `prompt_args`. This is the existing PR-A behavior preserved
        // under subcommand dispatch.
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--model",
            "fixture",
            "ping",
            "pong",
        ])
        .unwrap();
        assert_eq!(cli.agent.as_deref(), Some("ping"));
        assert_eq!(cli.prompt_args, vec!["pong"]);
    }

    #[test]
    fn parser_accepts_provider_pin_for_fresh_model_run() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--model",
            "fixture",
            "--pin-provider",
            "provider2",
            "ping",
        ])
        .unwrap();

        assert_eq!(cli.pin_provider.as_deref(), Some("provider2"));
    }

    #[test]
    fn parser_rejects_provider_pin_for_resume() {
        let err = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "--pin-provider",
            "provider2",
        ])
        .unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parser_accepts_long_new() {
        let cli = Cli::try_parse_from(["oulipoly-agent-runner", "--new"]).unwrap();

        assert!(cli.new);
        assert!(cli.command.is_none());
        assert!(cli.agent.is_none());
        assert!(cli.prompt_args.is_empty());
        assert!(cli.model.is_none());
        assert!(cli.resume.is_none());
    }

    #[test]
    fn parser_accepts_short_n() {
        let cli = Cli::try_parse_from(["oulipoly-agent-runner", "-n"]).unwrap();

        assert!(cli.new);
        assert!(cli.command.is_none());
        assert!(cli.agent.is_none());
        assert!(cli.prompt_args.is_empty());
        assert!(cli.model.is_none());
        assert!(cli.resume.is_none());
    }

    #[test]
    fn parser_rejects_resume_then_new() {
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "--new",
        ]) {
            Ok(_) => panic!("--resume and --new should conflict"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
        let rendered = err.to_string();
        assert!(rendered.contains("--resume"), "{rendered}");
        assert!(rendered.contains("--new"), "{rendered}");
    }

    #[test]
    fn parser_rejects_new_then_resume() {
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--new",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
        ]) {
            Ok(_) => panic!("--new and --resume should conflict"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
        let rendered = err.to_string();
        assert!(rendered.contains("--new"), "{rendered}");
        assert!(rendered.contains("--resume"), "{rendered}");
    }

    #[test]
    fn repl_subcommand_parses_required_model_without_optional_paths() {
        let cli = Cli::try_parse_from(["oulipoly-agent-runner", "repl", REPL_MODEL]).unwrap();

        match cli.command {
            Some(Subcommands::Repl {
                model,
                resume,
                rotate_provider: _,
                project,
                models_dir,
            }) => {
                assert_eq!(model.as_deref(), Some(REPL_MODEL));
                assert_eq!(resume, None);
                assert_eq!(project, None);
                assert_eq!(models_dir, None);
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_parses_project_path() {
        let cli = Cli::try_parse_from(["oulipoly-agent-runner", "repl", REPL_MODEL, "-p", "/tmp"])
            .unwrap();

        match cli.command {
            Some(Subcommands::Repl {
                resume, project, ..
            }) => {
                assert_eq!(resume, None);
                assert_eq!(project, Some(PathBuf::from("/tmp")));
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_parses_models_dir_override() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "repl",
            "--models-dir",
            "/tmp/models",
            REPL_MODEL,
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Repl {
                resume, models_dir, ..
            }) => {
                assert_eq!(resume, None);
                assert_eq!(models_dir, Some(PathBuf::from("/tmp/models")));
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_parses_resume_after_model() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "repl",
            REPL_MODEL,
            "--resume",
            session_id,
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Repl { model, resume, .. }) => {
                assert_eq!(model.as_deref(), Some(REPL_MODEL));
                assert_eq!(resume.as_deref(), Some(session_id));
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn resume_subcommand_accepts_positional_chain_id() {
        let chain_id = "7ec82d7d-5f83-4be7-8868-b1ce3c9c3123";
        let command = parse_resume_subcommand(["oulipoly-agent-runner", "resume", chain_id]);

        // Keep this layout-tolerant: Phase 6c may use flat fields or a flattened target Args struct.
        assert_resume_debug_contains_option_field(command, "chain_id", chain_id);
    }

    #[test]
    fn resume_subcommand_accepts_session_id_flag() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let command = parse_resume_subcommand([
            "oulipoly-agent-runner",
            "resume",
            "--session-id",
            session_id,
        ]);

        assert_resume_debug_contains_option_field(command, "session_id", session_id);
    }

    #[test]
    fn resume_forms_preserve_caller_submission_token() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let token = "caller-stable-token";
        let top_level = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--resume",
            session_id,
            "--submission-token",
            token,
            "payload",
        ])
        .unwrap();
        assert_eq!(top_level.submission_token.as_deref(), Some(token));

        let nested = parse_resume_subcommand([
            "oulipoly-agent-runner",
            "resume",
            session_id,
            "--submission-token",
            token,
            "--prompt",
            "payload",
        ]);
        assert_resume_debug_contains_option_field(nested, "submission_token", token);
    }

    #[test]
    fn resume_subcommand_rejects_both_positional_and_session_id() {
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "resume",
            "7ec82d7d-5f83-4be7-8868-b1ce3c9c3123",
            "--session-id",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
        ]) {
            Ok(_) => panic!("expected clap to reject both resume target forms"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn resume_subcommand_rejects_no_target() {
        let err = match Cli::try_parse_from(["oulipoly-agent-runner", "resume"]) {
            Ok(_) => panic!("expected clap to reject resume without a target"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn resume_subcommand_dispatch_routes_positional_to_run_resume_arg() {
        let chain_id = "7ec82d7d-5f83-4be7-8868-b1ce3c9c3123";
        let command = parse_resume_subcommand(["oulipoly-agent-runner", "resume", chain_id]);

        // Parser-level proxy for dispatch: the field selected by the Resume arm must hold
        // the positional target unchanged so Phase 6c can pass it directly to run_resume.
        assert_resume_debug_contains_option_field(command, "chain_id", chain_id);
    }

    #[test]
    fn resume_subcommand_parses_answer_file_and_project() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "resume",
            "-m",
            REPL_MODEL,
            "--session-id",
            session_id,
            "-f",
            "/tmp/answer.md",
            "-p",
            "/tmp/project",
            "--models-dir",
            "/tmp/models",
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Resume {
                model,
                session_id: parsed_session,
                rotate_provider: _,
                prompt,
                file,
                project,
                models_dir,
                ..
            }) => {
                assert_eq!(model.as_deref(), Some(REPL_MODEL));
                assert_eq!(
                    resume_session_field_as_deref(&parsed_session),
                    Some(session_id)
                );
                assert_eq!(prompt, None);
                assert_eq!(file, Some(PathBuf::from("/tmp/answer.md")));
                assert_eq!(project, Some(PathBuf::from("/tmp/project")));
                assert_eq!(models_dir, Some(PathBuf::from("/tmp/models")));
            }
            _ => panic!("expected resume subcommand"),
        }
    }

    #[test]
    fn resume_subcommand_parses_inline_prompt() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "resume",
            "-m",
            REPL_MODEL,
            "--session-id",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "--prompt",
            "answer text",
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Resume { prompt, file, .. }) => {
                assert_eq!(prompt.as_deref(), Some("answer text"));
                assert_eq!(file, None);
            }
            _ => panic!("expected resume subcommand"),
        }
    }

    #[test]
    fn resume_subcommand_rejects_prompt_and_file_together() {
        let err = match Cli::try_parse_from([
            "oulipoly-agent-runner",
            "resume",
            "-m",
            REPL_MODEL,
            "--session-id",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "--prompt",
            "answer text",
            "-f",
            "/tmp/answer.md",
        ]) {
            Ok(_) => panic!("expected clap to reject --prompt with --file"),
            Err(err) => err,
        };

        let rendered = err.to_string();
        assert!(
            rendered.contains("--prompt") || rendered.contains("--file"),
            "{rendered}"
        );
    }

    #[test]
    fn repl_subcommand_parses_resume_before_model() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "repl",
            "--resume",
            session_id,
            REPL_MODEL,
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Repl { model, resume, .. }) => {
                assert_eq!(model.as_deref(), Some(REPL_MODEL));
                assert_eq!(resume.as_deref(), Some(session_id));
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_allows_missing_model_with_resume() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "repl",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
        ])
        .unwrap();

        match cli.command {
            Some(Subcommands::Repl { model, resume, .. }) => {
                assert_eq!(model, None);
                assert_eq!(
                    resume.as_deref(),
                    Some("5169694d-de0f-40d1-890c-6e28e55bab27")
                );
            }
            _ => panic!("expected repl subcommand"),
        }
    }

    #[test]
    fn repl_subcommand_requires_resume_value() {
        let err =
            match Cli::try_parse_from(["oulipoly-agent-runner", "repl", REPL_MODEL, "--resume"]) {
                Ok(_) => panic!("expected clap to reject --resume without a value"),
                Err(err) => err,
            };

        let rendered = err.to_string();
        assert!(rendered.contains("--resume"), "{rendered}");
    }

    #[test]
    fn repl_subcommand_rejects_extra_positional_arguments() {
        let err = match Cli::try_parse_from(["oulipoly-agent-runner", "repl", REPL_MODEL, "extra"])
        {
            Ok(_) => panic!("expected clap to reject extra repl positional arguments"),
            Err(err) => err,
        };

        let rendered = err.to_string();
        assert!(rendered.contains("extra"), "{rendered}");
    }

    #[test]
    fn resolve_parent_invocation_id_returns_none_when_env_is_unset() {
        let db = test_db();

        with_parent_invocation_env(None, || {
            assert_eq!(resolve_parent_invocation_id(&db), None);
        });
    }

    #[test]
    fn resolve_parent_invocation_id_returns_existing_parent_rowid() {
        let db = test_db();
        let parent = CompositeInvocationId {
            source: "fixture-provider".to_string(),
            id: Uuid::new_v4().to_string(),
        };
        let row_id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: parent.id.clone(),
                model_name: "fixture-model".to_string(),
                provider_name: parent.source.clone(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let parent_env = serde_json::to_string(&parent).unwrap();

        with_parent_invocation_env(Some(&parent_env), || {
            assert_eq!(resolve_parent_invocation_id(&db), Some(row_id));
        });
    }

    #[test]
    fn resolve_parent_invocation_id_uses_same_db_uuid_despite_source_name_drift() {
        let db = test_db();
        let parent = CompositeInvocationId {
            source: "renamed-provider".to_string(),
            id: Uuid::new_v4().to_string(),
        };
        let row_id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: parent.id.clone(),
                model_name: "fixture-model".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        db.finalize_invocation(row_id, false, 1, None, Some("exit_nonzero"))
            .unwrap();
        let parent_env = serde_json::to_string(&parent).unwrap();

        with_parent_invocation_env(Some(&parent_env), || {
            assert_eq!(resolve_parent_invocation_id(&db), Some(row_id));
        });
    }

    #[test]
    fn resolve_parent_invocation_id_returns_none_for_malformed_json() {
        let db = test_db();
        with_parent_invocation_env(Some("not-json"), || {
            assert_eq!(resolve_parent_invocation_id(&db), None);
        });
    }

    #[test]
    fn resolve_parent_invocation_id_returns_none_for_unknown_uuid() {
        let db = test_db();
        let raw = r#"{"source":"fixture-provider","id":"00000000-0000-0000-0000-000000000000"}"#;
        with_parent_invocation_env(Some(raw), || {
            assert_eq!(resolve_parent_invocation_id(&db), None);
        });
    }

    #[test]
    fn resolve_parent_invocation_id_returns_none_for_invalid_uuid_format() {
        let db = test_db();
        let raw = r#"{"source":"fixture-provider","id":"not-a-uuid"}"#;
        with_parent_invocation_env(Some(raw), || {
            assert_eq!(resolve_parent_invocation_id(&db), None);
        });
    }

    #[test]
    fn resume_short_line_helper_emits_for_non_tty_stderr() {
        assert!(should_emit_resume_short_line(false));
    }

    #[test]
    fn resume_short_line_helper_also_emits_for_tty_stderr() {
        // The short [resume] -> <provider> line is unconditional per
        // proposal §5: V10 (observable selection) wins over V15
        // (caller-controlled surface) for this single line. The test
        // pins the "always-on" guarantee against future drift.
        assert!(should_emit_resume_short_line(true));
    }

    // risk: CLI surface; level: unit; source: proposal §11.1 CLI surface / A8.
    #[test]
    fn top_level_resume_parse_allows_missing_model_and_rotate_provider_flag() {
        let cli = Cli::try_parse_from([
            "oulipoly-agent-runner",
            "--resume",
            "5169694d-de0f-40d1-890c-6e28e55bab27",
            "--rotate-provider",
            "provider2",
            "continue",
        ])
        .unwrap();

        assert!(cli.command.is_none());
        assert_eq!(cli.model, None);
        assert_eq!(
            cli.resume.as_deref(),
            Some("5169694d-de0f-40d1-890c-6e28e55bab27")
        );
        assert_eq!(cli.rotate_provider.as_deref(), Some("provider2"));
    }

    #[test]
    fn fresh_continuation_rejects_interactive_prompt_source() {
        let error = continuation_command_prompt(&TopLevelResumePromptSource::Interactive)
            .expect_err("fresh continuation should require input");

        assert_eq!(
            error,
            "No prompt provided. Pass as argument, --file, or pipe to stdin."
        );
    }

    #[test]
    fn fresh_continuation_preserves_file_only_prompt_source() {
        let prompt_source = TopLevelResumePromptSource::Headless {
            positional_or_stdin_prompt: None,
        };

        assert_eq!(continuation_command_prompt(&prompt_source).unwrap(), None);
    }
}
