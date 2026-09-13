//! Pre-provider independent notification ownership. No workload argv crosses this
//! endpoint; original Bash supervisor/guardian/workload ancestry is unchanged.
//! Declared roles: orchestration, validator, accessor, parser, mapper.
#[cfg(target_os = "linux")]
mod custody;
#[cfg(target_os = "linux")]
mod driver;
#[cfg(target_os = "linux")]
mod linux;

pub(crate) const ENDPOINT_ENV: &str = "OULIPOLY_COMPLETION_ENDPOINT";

pub(crate) fn bootstrap(cli: &crate::usage::cli::Cli) -> Result<(), String> {
    use crate::usage::cli::Subcommands;
    let provider = !cli.usage
        && matches!(
            &cli.command,
            None | Some(Subcommands::Repl { .. }) | Some(Subcommands::Resume { .. })
        );
    if !provider {
        return Ok(());
    }
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
        Ok(owner.driver_identity != linux::identity(i64::from(std::process::id()))?)
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
