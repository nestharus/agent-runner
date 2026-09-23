//! Pre-provider root process-tree ownership. Completion-continuation operations
//! retain their existing State/mailbox authority; original-work-v1 requests use
//! a separate capability, acceptance, cancellation, and result protocol on the
//! same inherited endpoint and guardian.
//! Declared roles: orchestration, validator, accessor, parser, mapper.
#[cfg(target_os = "linux")]
mod custody;
#[cfg(target_os = "linux")]
mod driver;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod original_work;
#[cfg(target_os = "linux")]
mod root_supervisor;

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) const ENDPOINT_ENV: &str = "OULIPOLY_COMPLETION_ENDPOINT";
pub(crate) const ROOT_AUTHORITY_ENV: &str = "OULIPOLY_ROOT_AUTHORITY_V1";
pub(crate) const ORIGINAL_WORK_REQUIRED_ENV: &str = "OULIPOLY_ORIGINAL_WORK_REQUIRED_V1";
pub(crate) const EXPECTED_KERNEL_ROOT_ENV: &str = "OULIPOLY_KERNEL_EXPECTED_ROOT_V1";

#[cfg(target_os = "linux")]
pub(crate) struct PinnedGuardian {
    pub(crate) root_id: String,
    pub(crate) domain_id: String,
    pub(crate) supervisor_authority_id: String,
}

#[cfg(target_os = "linux")]
pub(crate) fn run_pinned_guardian(
    pin: &PinnedGuardian,
    announce: std::os::unix::net::UnixStream,
) -> Result<(), String> {
    linux::run_pinned_guardian(pin, announce)
}

#[cfg(target_os = "linux")]
pub(crate) fn verify_pinned_owner_ready(
    announce: &mut std::os::unix::net::UnixStream,
    pin: &PinnedGuardian,
    guardian_pid: i32,
) -> Result<(), String> {
    linux::verify_pinned_owner_ready(announce, pin, guardian_pid)
}

#[cfg(target_os = "linux")]
pub(crate) fn verify_kernel_owner_socket(
    root_id: &str,
    owner: &oulipoly_state::mailbox::CompletionDomainOwner,
    socket: &std::os::unix::net::UnixStream,
) -> Result<(), String> {
    linux::verify_kernel_owner_socket(root_id, owner, socket)
}

/// Preserve the State open source at entry; unrelated owner/path text is operational.
#[derive(Debug)]
pub(crate) enum BootstrapError {
    State(oulipoly_state::WritableOpenError),
    Operational(String),
}

impl BootstrapError {
    pub(crate) fn is_schema_refusal(&self) -> bool {
        matches!(self, Self::State(error) if error.is_schema_refusal())
    }
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::State(error) => std::fmt::Display::fmt(error, f),
            Self::Operational(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for BootstrapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::State(error) => Some(error),
            Self::Operational(_) => None,
        }
    }
}

impl From<String> for BootstrapError {
    fn from(message: String) -> Self {
        Self::Operational(message)
    }
}

impl From<&str> for BootstrapError {
    fn from(message: &str) -> Self {
        Self::Operational(message.to_owned())
    }
}

pub(crate) fn bootstrap(cli: &crate::usage::cli::Cli) -> Result<(), BootstrapError> {
    if !requires_service(cli) {
        return Ok(());
    }
    bootstrap_entry_service()
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

/// Acquire an independent service lease at a supported process-entry boundary.
/// Inspection and ACK callers must not bootstrap a recovery service.
pub fn bootstrap_service() -> Result<(), String> {
    bootstrap_entry_service().map_err(|error| error.to_string())
}

fn bootstrap_entry_service() -> Result<(), BootstrapError> {
    #[cfg(target_os = "linux")]
    {
        linux::bootstrap()?;
        // The initial provider is launched by this process, not by the
        // completion root worker. Establish lineage before dispatch can fork.
        custody::establish_entry_lineage().map_err(BootstrapError::Operational)
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
        if owner.driver_identity == linux::current_identity()? {
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
    match std::env::args().nth(1).as_deref() {
        Some(driver::DRIVER_ARG) => Some(driver::entry()),
        Some(root_supervisor::ROOT_WORKER_ARG) => Some(custody::root_worker_entry()),
        // The nested adopter/custodian executable path is retained only by
        // unit fixtures. Production launches are admitted by the root
        // supervisor and cannot select this former authority path.
        #[cfg(test)]
        Some(custody::CUSTODIAN_ARG | custody::ADOPTER_ARG) => Some(custody::entry()),
        _ => None,
    }
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
