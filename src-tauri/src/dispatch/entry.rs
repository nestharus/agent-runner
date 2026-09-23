//! Command-owned validation before independent owner acquisition.
//! Declared roles: orchestration, mapper

use crate::usage::cli::{Cli, NotifySubcommands, SessionSubcommands, Subcommands};

/// Reject invalid requests without opening storage or admitting a child.
/// Passing preflight is not authority: entry must authenticate/join its owner
/// before durable child validation, and execution retains its later fences.
pub(crate) fn preflight_entry(cli: &Cli) -> Result<Option<i32>, String> {
    if cli.usage {
        return Ok(None);
    }
    // Pinned helper commands must reject a mismatched transient work claim
    // before completion-owner bootstrap can establish a service or lineage.
    if std::env::var_os(crate::completion_owner::EXPECTED_KERNEL_ROOT_ENV).is_some()
        || std::env::var_os("AGENT_BASH_OWNER_WORK_ID_V1").is_some()
    {
        if let Some(Subcommands::Notify { command }) = &cli.command {
            let (handle, registration_file) = match command {
                NotifySubcommands::Register {
                    handle,
                    registration_file,
                    ..
                }
                | NotifySubcommands::Complete {
                    handle,
                    registration_file,
                    ..
                } => (Some(handle.as_str()), registration_file.as_deref()),
                NotifySubcommands::Activate { handle, .. } => (Some(handle.as_str()), None),
                NotifySubcommands::Registration {
                    registration_file, ..
                }
                | NotifySubcommands::CompletionState {
                    registration_file, ..
                } => (None, Some(registration_file.as_path())),
                _ => (None, None),
            };
            if let Some(path) = registration_file {
                let binding = crate::commands::notify_continuation::load_binding(path)?;
                let source = binding.registration()?;
                if handle.is_some_and(|handle| handle != source.handle) {
                    return Err("pinned owner work ID conflict".into());
                }
                crate::commands::notify::require_pinned_owner_work_id(&source.handle)?;
            } else if let Some(handle) = handle {
                crate::commands::notify::require_pinned_owner_work_id(handle)?;
            }
        }
    }
    match &cli.command {
        Some(Subcommands::Resume {
            session_id,
            chain_id,
            ..
        }) => {
            let target = super::resume_target_arg(session_id.as_deref(), chain_id.as_deref());
            if let Some(code) = crate::run::resume::reject_invalid_resume_input(target) {
                return Ok(Some(code));
            }
            Ok(reject_auto_wake_entry(target))
        }
        Some(Subcommands::Repl {
            resume: Some(target),
            ..
        }) => {
            crate::run::resume::validate_resume_input(target)?;
            Ok(reject_auto_wake_entry(target))
        }
        None if cli.resume.is_some() => {
            super::validate_top_level_resume_cli(cli)?;
            let target = cli.resume.as_deref().unwrap();
            crate::run::resume::validate_resume_input(target)?;
            Ok(reject_auto_wake_entry(target))
        }
        Some(Subcommands::Session { command }) => Ok(match command {
            SessionSubcommands::ImportReplace {
                session_id,
                preimage_sha256,
                ..
            } => crate::commands::session_import_replace::validate_import_replace_args(
                session_id,
                preimage_sha256.as_deref(),
            ),
            SessionSubcommands::PauseHandshake { session_id, ttl_ms } => {
                crate::commands::handshake::validate_pause_handshake_args(session_id, *ttl_ms)
                    .is_none()
                    .then_some(2)
            }
            SessionSubcommands::ResumeHandshake { session_id, .. } => {
                crate::commands::handshake::validate_resume_handshake_session_id(session_id)
            }
            _ => None,
        }),
        _ => Ok(None),
    }
}

fn reject_auto_wake_entry(target: &str) -> Option<i32> {
    crate::wake_coordinator::reject_auto_wake_entry(
        target,
        std::env::var_os(crate::completion_owner::ENDPOINT_ENV).is_some(),
    )
}

/// Durable validation of the same marked routes, only after owner bootstrap.
/// Headless execution retains its later identity/custody recheck. Replay there
/// is not a fresh busy/observation-stop decision. REPL has this entry check only;
/// it does not inherit headless's later recheck or no-rotation policy.
pub(crate) fn validate_owned_entry(cli: &Cli) -> Result<Option<i32>, String> {
    if cli.usage {
        return Ok(None);
    }
    let target = match &cli.command {
        Some(Subcommands::Resume {
            session_id,
            chain_id,
            ..
        }) => Some(super::resume_target_arg(
            session_id.as_deref(),
            chain_id.as_deref(),
        )),
        Some(Subcommands::Repl {
            resume: Some(target),
            ..
        }) => Some(target.as_str()),
        None => cli.resume.as_deref(),
        _ => None,
    };
    match target {
        Some(target) => crate::wake_coordinator::validate_auto_wake_child(target),
        None => Ok(None),
    }
}

/// Acquiring authority may encounter the same storage refusal as the handler.
/// Preserve the command renderer, classifying only actual State migration refusals.
pub(crate) fn entry_bootstrap_error(
    cli: &Cli,
    error: crate::completion_owner::BootstrapError,
) -> Result<i32, String> {
    if matches!(
        cli.command,
        Some(Subcommands::Session {
            command: SessionSubcommands::ImportReplace { .. }
        })
    ) {
        use oulipoly_runtime::session_replace::ReplaceError;
        let failure = if error.is_schema_refusal() {
            ReplaceError::SchemaIncompatible {
                reason: error.to_string(),
            }
        } else {
            ReplaceError::OperationalError {
                message: error.to_string(),
            }
        };
        return crate::commands::session_import_replace::render_import_replace_output(Err(failure));
    }
    Err(error.to_string())
}
