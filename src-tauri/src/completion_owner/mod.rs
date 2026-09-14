//! Pre-provider independent notification ownership. No workload argv crosses this
//! endpoint; original Bash supervisor/guardian/workload ancestry is unchanged.
//! Declared roles: orchestration, validator, accessor, parser, mapper.
#[cfg(target_os = "linux")]
mod custody;
#[cfg(target_os = "linux")]
mod driver;
#[cfg(target_os = "linux")]
mod linux;

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) const ENDPOINT_ENV: &str = "OULIPOLY_COMPLETION_ENDPOINT";

pub(crate) fn bootstrap(cli: &crate::usage::cli::Cli) -> Result<(), String> {
    if !requires_service(cli) {
        return Ok(());
    }
    bootstrap_service()
}

// Process-entry boundary, before runtime threads or recovery. Database opens,
// readback, ACK and maintenance do not acquire a service lease.
fn requires_service(cli: &crate::usage::cli::Cli) -> bool {
    use crate::usage::cli::{MailboxSubcommands, NotifySubcommands, Subcommands};
    !cli.usage
        && (matches!(
            &cli.command,
            None | Some(Subcommands::Repl { .. })
                | Some(Subcommands::Resume { .. })
                | Some(Subcommands::Notify {
                    command: NotifySubcommands::Register { .. }
                        | NotifySubcommands::Listen { .. }
                        | NotifySubcommands::Activate { .. }
                        | NotifySubcommands::Complete { .. }
                })
                | Some(Subcommands::Mailbox {
                    command: MailboxSubcommands::Resume { .. }
                })
        ) || startup_wake_reclaim_sweep_enabled(cli))
}

pub(crate) fn startup_wake_reclaim_sweep_enabled(cli: &crate::usage::cli::Cli) -> bool {
    use crate::usage::cli::{SessionSubcommands, Subcommands};
    if cli.usage || cli.resume.is_some() {
        return false;
    }
    // Inspection/ACK and explicit storage maintenance must not become recovery
    // service entrypoints. Import and session mutations retain startup recovery.
    matches!(
        &cli.command,
        None | Some(Subcommands::Repl { resume: None, .. })
            | Some(Subcommands::Session {
                command: SessionSubcommands::Import { .. }
                    | SessionSubcommands::ImportReplace { .. }
                    | SessionSubcommands::PauseHandshake { .. }
                    | SessionSubcommands::ResumeHandshake { .. }
            })
    )
}

pub(crate) fn bootstrap_service() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::bootstrap()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

pub(crate) fn require_owner(
    domain_id: &str,
) -> Result<oulipoly_state::mailbox::CompletionDomainOwner, String> {
    #[cfg(target_os = "linux")]
    {
        linux::require_owner(domain_id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = domain_id;
        Err("completion-continuation-v2 requires supported Linux independent entry".into())
    }
}

#[cfg(target_os = "linux")]
pub fn spawn_activation<F>(
    path: &std::path::Path,
    attempt: &oulipoly_state::mailbox::ContinuationAttempt,
    command: F,
) -> Result<i64, String>
where
    F: FnOnce() -> Result<std::process::Command, String>,
{
    custody::spawn_activation(path, attempt, command)
}

pub fn defer_wake_to_owner() -> Result<bool, String> {
    let mailbox = oulipoly_state::mailbox::MailboxDb::open_default()?;
    if mailbox.completion_continuation_domain()?.is_none() {
        return Ok(false);
    }
    let owner = mailbox
        .completion_continuation_owner()?
        .ok_or("native completion owner unavailable")?;
    #[cfg(target_os = "linux")]
    {
        if owner.driver_identity == linux::identity(i64::from(std::process::id()))? {
            return Ok(false);
        }
        // A stored owner row is discovery, not proof of a usable successor.
        // Never elect here: this can run in runtime threads and managed children.
        require_owner(&owner.domain_id)?;
        Ok(true)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = owner;
        Ok(true)
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn custodian_entry() -> Option<Result<(), String>> {
    matches!(
        std::env::args().nth(1).as_deref(),
        Some(custody::CUSTODIAN_ARG | custody::ADOPTER_ARG)
    )
    .then(custody::entry)
}

#[cfg(test)]
mod entry_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn service_entry_tracks_wake_producers_not_database_access() {
        for args in [
            vec!["runner", "-m", "fixture", "prompt"],
            vec!["runner", "resume", "--session-id", "fixture"],
            vec!["runner", "mailbox", "resume", "--session-id", "fixture"],
            vec![
                "runner",
                "notify",
                "agent-bash-activate",
                "--handle",
                "fixture",
            ],
            vec![
                "runner",
                "notify",
                "agent-bash-register",
                "--handle",
                "fixture",
                "--delivery-mode",
                "async",
                "--state-dir",
                "/fixture",
                "--meta",
                "/fixture/meta",
                "--log",
                "/fixture/log",
                "--rc",
                "/fixture/rc",
            ],
            vec![
                "runner",
                "notify",
                "agent-bash-complete",
                "--handle",
                "fixture",
                "--caller-ppid",
                "1",
                "--state-dir",
                "/fixture",
                "--meta",
                "/fixture/meta",
                "--log",
                "/fixture/log",
                "--rc",
                "/fixture/rc",
            ],
            vec!["runner", "session", "import"],
        ] {
            assert!(
                requires_service(&crate::usage::cli::Cli::try_parse_from(&args).unwrap()),
                "{args:?}"
            );
        }
        for args in [
            vec!["runner", "--usage"],
            vec!["runner", "mailbox", "list", "--session-id", "fixture"],
            vec![
                "runner",
                "mailbox",
                "ack",
                "--session-id",
                "fixture",
                "--from-seq",
                "1",
                "--to-seq",
                "1",
            ],
            vec!["runner", "mailbox", "pause", "--session-id", "fixture"],
            vec!["runner", "notify", "agent-bash-capability"],
            vec![
                "runner",
                "notify",
                "agent-bash-registration",
                "--registration-file",
                "/fixture",
            ],
            vec![
                "runner",
                "notify",
                "agent-bash-completion-state",
                "--registration-file",
                "/fixture",
            ],
            vec!["runner", "session", "schema-probe"],
            vec!["runner", "session", "list"],
            vec!["runner", "trace", "fixture"],
            vec!["runner", "migrate-db"],
        ] {
            let cli = crate::usage::cli::Cli::try_parse_from(&args).unwrap();
            assert!(!requires_service(&cli), "{args:?}");
            assert!(!startup_wake_reclaim_sweep_enabled(&cli), "{args:?}");
        }
    }
}
