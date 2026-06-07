//! ## Declared roles
//!
//! Roles: mapper, formatter, predicate, orchestration, accessor, validator.
//!
//! - mapper: [`classify_terminal_reason`] maps a child [`ExitStatus`] onto
//!   the stable reason vocabulary (`None | "exit_nonzero" | "signal:NAME"
//!   | "unknown_exit"`).
//! - mapper: [`exit_code_from_status`] is the canonical executor-boundary
//!   `ExitStatus` → `i32` conversion.
//! - mapper: [`terminal_status_from_exit_status`],
//!   [`terminal_reason_from_signal`], [`synthetic_exit_code`].
//! - mapper: [`recognize_terminal_signal`] composes evidence and dispatches
//!   to the bounded provider recognizer.
//! - formatter: [`signal_name`].
//! - predicate: [`should_forward_interactive_sigterm`].
//! - orchestration: [`InteractiveSignalGuard::install`] and `Drop` manage
//!   signal-handler lifecycle and the forwarding thread.
//! - accessor: [`child_signal_pid`].
//! - validator: embedded exit-code contract test
//!   `exit_code_from_status_uses_unified_child_process_contract`.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs
//!     role: adapter
//!     Translates:
//!       - std-process-exit-status-contract
//!       - unix-signal-name-contract
//!       - signal-hook-forwarding-contract
//!       - executor-terminal-signal-dto-contract
//!       - terminal-signal-recognizer-contract
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs
//!     role: intrinsic-surface
//!     Domain: runtime terminal-signal vocabulary + reason mapping
//!     Owns:
//!       - full TerminalSignalKind vocabulary
//!       - terminal status and synthetic exit-code mapping
//!       - built-in terminal evidence construction
//!       - terminal reason canonicalization hook
//!       - provider-error terminal-reason evidence preservation for Unknown signals
//! ```

#[cfg(unix)]
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
#[cfg(unix)]
use signal_hook::iterator::{Handle as SignalsHandle, Signals};
use std::process::{Child, ExitStatus};
#[cfg(unix)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::SystemTime;

use super::provider_identity::ProviderRecognizer;
use crate::executor::terminal_signal::{
    TerminalSignal, TerminalSignalEvidence, TerminalSignalKind, TerminalStatusEvidence,
};

const PROVIDER_ERROR_EVIDENCE_PREFIX: &str = "provider error: ";

pub fn classify_terminal_reason(status: &std::process::ExitStatus) -> Option<String> {
    if let Some(code) = status.code() {
        return (code != 0).then(|| "exit_nonzero".to_string());
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            return Some(format!("signal:{}", signal_name(signal)));
        }
    }

    Some("unknown_exit".to_string())
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGHUP => "SIGHUP".to_string(),
        libc::SIGINT => "SIGINT".to_string(),
        libc::SIGQUIT => "SIGQUIT".to_string(),
        libc::SIGILL => "SIGILL".to_string(),
        libc::SIGTRAP => "SIGTRAP".to_string(),
        libc::SIGABRT => "SIGABRT".to_string(),
        libc::SIGBUS => "SIGBUS".to_string(),
        libc::SIGFPE => "SIGFPE".to_string(),
        libc::SIGKILL => "SIGKILL".to_string(),
        libc::SIGUSR1 => "SIGUSR1".to_string(),
        libc::SIGSEGV => "SIGSEGV".to_string(),
        libc::SIGUSR2 => "SIGUSR2".to_string(),
        libc::SIGPIPE => "SIGPIPE".to_string(),
        libc::SIGALRM => "SIGALRM".to_string(),
        libc::SIGTERM => "SIGTERM".to_string(),
        libc::SIGCHLD => "SIGCHLD".to_string(),
        libc::SIGCONT => "SIGCONT".to_string(),
        libc::SIGSTOP => "SIGSTOP".to_string(),
        libc::SIGTSTP => "SIGTSTP".to_string(),
        libc::SIGTTIN => "SIGTTIN".to_string(),
        libc::SIGTTOU => "SIGTTOU".to_string(),
        _ => signal.to_string(),
    }
}

#[cfg(not(unix))]
fn signal_name(signal: i32) -> String {
    signal.to_string()
}

/// `exit_code_from_status` is the canonical executor-boundary conversion from
/// a child `ExitStatus` to the runtime's `i32` exit-code representation:
///
/// - `status.code()` is preserved when `Some(n)` (covers normal exits including
///   non-zero).
/// - On Unix, when `status.code()` is `None` and `ExitStatusExt::signal()`
///   returns `sig`, the helper returns `128 + sig` (POSIX shell convention;
///   e.g., SIGTERM -> 143).
/// - When neither a normal code nor a signal is available, the helper returns
///   `-1` (preserved fallback).
pub(super) fn exit_code_from_status(status: &ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(code) = status.code() {
            return code;
        }
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    status.code().unwrap_or(-1)
}

pub(super) fn terminal_status_from_exit_status(status: &ExitStatus) -> TerminalStatusEvidence {
    if let Some(code) = status.code() {
        return TerminalStatusEvidence::Exited { code };
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            return TerminalStatusEvidence::SignalTerminated { signal };
        }
    }

    TerminalStatusEvidence::Unknown
}

pub(super) fn terminal_reason_from_signal(
    signal: &TerminalSignal,
    status: Option<&ExitStatus>,
) -> Option<String> {
    let status = status.map(terminal_status_from_exit_status);
    terminal_reason_from_signal_status(signal, status.as_ref())
}

pub(crate) fn terminal_reason_from_signal_status(
    signal: &TerminalSignal,
    status: Option<&TerminalStatusEvidence>,
) -> Option<String> {
    match signal.kind {
        TerminalSignalKind::ProlongedSilence
        | TerminalSignalKind::QuotaExhaustedInband
        | TerminalSignalKind::MaybeQuotaExhausted
        | TerminalSignalKind::RateLimited
        | TerminalSignalKind::SpawnError => {
            crate::diagnostics::canonical_terminal_reason_for_kind(signal.kind).map(str::to_string)
        }
        TerminalSignalKind::Unknown => unknown_terminal_reason(signal),
        TerminalSignalKind::CleanExit
        | TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit => status.and_then(terminal_status_reason),
    }
}

fn terminal_status_reason(status: &TerminalStatusEvidence) -> Option<String> {
    match status {
        TerminalStatusEvidence::Exited { code } => (*code != 0).then(|| "exit_nonzero".to_string()),
        TerminalStatusEvidence::SignalTerminated { signal } => {
            Some(format!("signal:{}", signal_name(*signal)))
        }
        TerminalStatusEvidence::Unknown => Some("unknown_exit".to_string()),
        TerminalStatusEvidence::SpawnError { .. }
        | TerminalStatusEvidence::ProlongedSilence { .. } => None,
    }
}

fn unknown_terminal_reason(signal: &TerminalSignal) -> Option<String> {
    let evidence = signal.evidence.trim();
    if evidence.starts_with(PROVIDER_ERROR_EVIDENCE_PREFIX) {
        return Some(signal.evidence.clone());
    }
    crate::diagnostics::canonical_terminal_reason_for_kind(signal.kind).map(str::to_string)
}

pub(super) fn synthetic_exit_code(signal: &TerminalSignal) -> i32 {
    match signal.kind {
        TerminalSignalKind::CleanExit => 0,
        TerminalSignalKind::ProlongedSilence
        | TerminalSignalKind::QuotaExhaustedInband
        | TerminalSignalKind::MaybeQuotaExhausted
        | TerminalSignalKind::RateLimited
        | TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit
        | TerminalSignalKind::SpawnError
        | TerminalSignalKind::Unknown => -1,
    }
}

pub(crate) fn terminal_exit_code_from_signal(signal: &TerminalSignal, real_exit_code: i32) -> i32 {
    let synthetic = synthetic_exit_code(signal);
    if real_exit_code == 0 && synthetic != 0 {
        return synthetic;
    }
    real_exit_code
}

pub(super) fn recognize_terminal_signal(
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
    terminal_status: TerminalStatusEvidence,
) -> TerminalSignal {
    let evidence = terminal_signal_evidence(provider_name, stdout, stderr, terminal_status);
    recognizer.recognize(&evidence)
}

fn terminal_signal_evidence<'a>(
    provider_name: &'a str,
    stdout: &'a [u8],
    stderr: &'a [u8],
    terminal_status: TerminalStatusEvidence,
) -> TerminalSignalEvidence<'a> {
    TerminalSignalEvidence {
        provider_name,
        stdout,
        stderr,
        terminal_status,
        observed_at: SystemTime::now(),
    }
}

#[cfg(test)]
pub(super) fn terminal_signal_for_spawn_error(provider_name: &str, reason: &str) -> TerminalSignal {
    recognize_terminal_signal(
        provider_name,
        ProviderRecognizer::OpenAiCompat,
        &[],
        &[],
        TerminalStatusEvidence::SpawnError {
            reason: reason.to_string(),
        },
    )
}

#[cfg(unix)]
pub(super) struct InteractiveSignalGuard {
    handle: SignalsHandle,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl InteractiveSignalGuard {
    pub(super) fn install(child: &mut Child) -> Result<Self, String> {
        Self::install_for_target(SignalForwardTarget::ChildPid(child_signal_pid(child)))
    }

    pub(super) fn install_process_group(child_pid: u32) -> Result<Self, String> {
        Self::install_for_target(SignalForwardTarget::ProcessGroup(child_pid as i32))
    }

    fn install_for_target(target: SignalForwardTarget) -> Result<Self, String> {
        let signals = install_interactive_signals()?;
        let handle = signals.handle();
        let thread = spawn_interactive_signal_thread(signals, target);

        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}

#[cfg(unix)]
fn install_interactive_signals() -> Result<Signals, String> {
    Signals::new([SIGINT, SIGTERM, SIGHUP]).map_err(signal_install_error)
}

#[cfg(unix)]
fn signal_install_error(err: std::io::Error) -> String {
    format!("Failed to install signal handlers: {err}")
}

#[cfg(unix)]
fn child_signal_pid(child: &Child) -> i32 {
    child.id() as i32
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum SignalForwardTarget {
    ChildPid(i32),
    ProcessGroup(i32),
}

#[cfg(unix)]
fn spawn_interactive_signal_thread(
    mut signals: Signals,
    target: SignalForwardTarget,
) -> thread::JoinHandle<()> {
    let forwarded_sigterm = Arc::new(AtomicBool::new(false));
    let thread_forwarded_sigterm = Arc::clone(&forwarded_sigterm);

    thread::spawn(move || {
        forward_interactive_signals(&mut signals, target, &thread_forwarded_sigterm);
    })
}

#[cfg(unix)]
fn forward_interactive_signals(
    signals: &mut Signals,
    target: SignalForwardTarget,
    forwarded_sigterm: &AtomicBool,
) {
    for signal in signals.forever() {
        if should_forward_interactive_signal(signal, target, forwarded_sigterm) {
            send_signal(target, signal);
        }
    }
}

#[cfg(unix)]
fn should_forward_interactive_signal(
    signal: i32,
    target: SignalForwardTarget,
    forwarded_sigterm: &AtomicBool,
) -> bool {
    match target {
        SignalForwardTarget::ChildPid(_) => {
            should_forward_interactive_sigterm(signal, forwarded_sigterm)
        }
        SignalForwardTarget::ProcessGroup(_) => {
            (signal == SIGINT || signal == SIGHUP)
                || should_forward_interactive_sigterm(signal, forwarded_sigterm)
        }
    }
}

#[cfg(unix)]
fn should_forward_interactive_sigterm(signal: i32, forwarded_sigterm: &AtomicBool) -> bool {
    signal == SIGTERM && !forwarded_sigterm.swap(true, Ordering::SeqCst)
}

#[cfg(unix)]
impl Drop for InteractiveSignalGuard {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(unix)]
fn send_signal(target: SignalForwardTarget, signal: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    let pid = match target {
        SignalForwardTarget::ChildPid(pid) => pid,
        SignalForwardTarget::ProcessGroup(pid) => -pid,
    };
    let _ = unsafe { kill(pid, signal) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn exit_code_from_status_uses_unified_child_process_contract() {
        use std::os::unix::process::ExitStatusExt;

        let success = std::process::ExitStatus::from_raw(0);
        let nonzero = std::process::ExitStatus::from_raw(7 << 8);
        let sigterm = std::process::ExitStatus::from_raw(15);
        let sigint = std::process::ExitStatus::from_raw(2);
        let unknown = std::process::ExitStatus::from_raw(0x7f);

        assert_eq!(exit_code_from_status(&success), 0);
        assert_eq!(exit_code_from_status(&nonzero), 7);
        assert_eq!(exit_code_from_status(&sigterm), 143);
        assert_eq!(exit_code_from_status(&sigint), 130);
        assert_eq!(exit_code_from_status(&unknown), -1);
    }
}
