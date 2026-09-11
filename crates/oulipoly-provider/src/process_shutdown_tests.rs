//! Deterministic terminal-branch schedules: workers cannot report a fault until
//! after the supervisor has selected a terminal path and stopped reading events.
use super::*;
use crate::custody::AttemptActorCustody;
use std::sync::mpsc;

#[derive(Clone, Copy, Debug)]
enum TerminalPath {
    Completed,
    CancelledCompleted,
    CancelledStreamCompleted,
    Timeout,
    ForcedCancel,
    WaitFailure,
    ProcessorFailure,
    CancelledProcessorFailure,
}

#[derive(Debug, Default)]
struct TestOutput {
    bytes: CapturedBytes,
    error: Option<ProviderClientError>,
}
impl StdoutDrainOutput for TestOutput {
    fn captured_bytes(&self) -> CapturedBytes {
        self.bytes.clone()
    }
    fn processor_error(&self) -> Option<&ProviderClientError> {
        self.error.as_ref()
    }
}

fn gated_worker<T: Send + 'static>(
    events: ProcessEventPublisher,
    fail: bool,
    work: impl FnOnce() -> T + Send + 'static,
) -> (mpsc::Sender<()>, thread::JoinHandle<T>) {
    let (release, gate) = mpsc::channel();
    let worker = thread::spawn(move || {
        run_process_worker(events, || {
            let output = work();
            gate.recv_timeout(Duration::from_secs(5))
                .expect("release selected terminal branch");
            assert!(!fail, "injected shutdown worker panic");
            output
        })
    });
    (release, worker)
}

fn terminal_schedule(path: TerminalPath, failed_worker: Option<&str>) -> bool {
    let attempt_id = uuid::Uuid::new_v4();
    let custody = AttemptActorCustody::new(attempt_id);
    let guard = custody.begin("launch");
    let limits = ProcessLimits {
        custody: Some(guard.0.clone()),
        kill_after_grace: Duration::from_millis(20),
        ..ProcessLimits::default()
    };
    let command = ProcessCommand::new("/bin/sh").arg("-c").arg("sleep 30");
    let mut child = spawn_provider_process(
        &command,
        std::iter::empty::<(&str, &str)>(),
        limits.custody.clone(),
        limits.spawn_observer.is_none(),
    )
    .unwrap();
    let (publisher, events) = process_event_bus();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let processor_failure = matches!(
        path,
        TerminalPath::ProcessorFailure | TerminalPath::CancelledProcessorFailure
    );
    let (release_stdout, stdout) = gated_worker(
        publisher.clone(),
        failed_worker == Some("stdout"),
        move || TestOutput {
            bytes: drain_reader(stdout, ByteLimit::new(1024), None),
            error: processor_failure.then(|| {
                ProviderClientError::host_transport(
                    HostErrorKind::Other("fixture_processor_error".into()),
                    "launch",
                    Some("observed-wire-id".into()),
                    ProviderDiagnostics::with_description(
                        "fixture processor rejected output".into(),
                    ),
                )
            }),
        },
    );
    let (release_stderr, stderr) = gated_worker(
        publisher.clone(),
        failed_worker == Some("stderr"),
        move || drain_reader(stderr, ByteLimit::new(1024), None),
    );
    let (release_stdin, stdin) =
        gated_worker(publisher, failed_worker == Some("stdin"), move || {
            write_stdin(stdin, vec![])
        });
    let now = Instant::now();
    let mut supervisor = ProcessSupervisor {
        argv: command.argv(),
        child,
        command,
        threads: ProcessThreads {
            stdout,
            stderr,
            stdin,
        },
        events,
        timeout_mode: if matches!(path, TerminalPath::CancelledStreamCompleted) {
            TimeoutMode::StdoutLineGap
        } else {
            TimeoutMode::TotalRuntime
        },
        started: now,
        next_status_poll: now,
        last_stdout_line: now,
        cancellation_started: None,
        _cancellation_registration: None,
        limits: &limits,
    };
    if matches!(
        path,
        TerminalPath::CancelledCompleted
            | TerminalPath::CancelledStreamCompleted
            | TerminalPath::ForcedCancel
            | TerminalPath::CancelledProcessorFailure
    ) {
        supervisor.begin_cancellation();
    }
    let completed_status = if matches!(
        path,
        TerminalPath::Completed
            | TerminalPath::CancelledCompleted
            | TerminalPath::CancelledStreamCompleted
    ) {
        terminate_tree(&mut supervisor.child);
        Some(
            wait_for_terminated_process(&mut supervisor.child, limits.kill_after_grace)
                .status
                .unwrap(),
        )
    } else {
        None
    };
    // The terminal branch has been selected. No worker has published a failure;
    // release all gates only after the final event read, never via a sleep race.
    assert!(!supervisor.events.take_pending().worker_failed);
    for release in [release_stdout, release_stderr, release_stdin] {
        release.send(()).unwrap();
    }
    let result = match path {
        TerminalPath::Completed
        | TerminalPath::CancelledCompleted
        | TerminalPath::CancelledStreamCompleted => {
            supervisor.collect_completed(completed_status.unwrap())
        }
        TerminalPath::Timeout => {
            Err(supervisor.terminate_and_collect(HostErrorKind::Timeout, false))
        }
        TerminalPath::ForcedCancel => Err(supervisor.force_kill_and_collect()),
        TerminalPath::WaitFailure => {
            let original = wait_operation_error(
                &supervisor.command,
                "owned_wait",
                std::io::Error::from_raw_os_error(libc::ECHILD),
            );
            let error = ProviderClientError::host_transport(
                HostErrorKind::WaitFailed,
                "launch",
                Some("observed-wait-id".into()),
                original.diagnostics().clone(),
            );
            Err(supervisor.terminate_after_wait_failure(error))
        }
        TerminalPath::ProcessorFailure | TerminalPath::CancelledProcessorFailure => {
            Err(supervisor.terminate_after_stdout_processor_failure())
        }
    };
    match path {
        TerminalPath::Timeout => assert_eq!(
            result.as_ref().unwrap_err().transport_kind(),
            "host_timeout"
        ),
        TerminalPath::CancelledCompleted
        | TerminalPath::ForcedCancel
        | TerminalPath::CancelledProcessorFailure => assert_eq!(
            result.as_ref().unwrap_err().transport_kind(),
            "host_cancelled"
        ),
        TerminalPath::Completed | TerminalPath::CancelledStreamCompleted
            if failed_worker.is_some() =>
        {
            assert_eq!(result.as_ref().unwrap_err().transport_kind(), "wait_failed")
        }
        TerminalPath::WaitFailure => {
            let error = result.as_ref().unwrap_err();
            assert_eq!(error.transport_kind(), "wait_failed");
            assert_eq!(error.request_id(), Some("observed-wait-id"));
            assert!(error.diagnostics().process_was_reaped);
            let description = error.diagnostics().description.as_deref().unwrap();
            assert!(description.starts_with("first_failure: owned_wait: "));
            assert!(description.contains(&format!("errno={}", libc::ECHILD)));
            if let Some(worker) = failed_worker {
                assert!(description.contains(&format!(
                    "; collection: process worker thread panicked: {worker}"
                )));
            }
        }
        TerminalPath::Completed | TerminalPath::CancelledStreamCompleted => assert!(result.is_ok()),
        TerminalPath::ProcessorFailure if failed_worker != Some("stdout") => assert_eq!(
            result.as_ref().unwrap_err().request_id(),
            Some("observed-wire-id")
        ),
        TerminalPath::ProcessorFailure => (),
    }
    drop(guard);
    let receipt = custody.receipts().remove(0);
    assert_eq!(receipt.attempt_id, attempt_id);
    assert!(
        receipt.operation_finished
            && receipt.spawned
            && receipt.leader_reaped
            && receipt.process_tree_terminated,
        "{path:?}: {receipt:?}"
    );
    assert!(receipt.process_status.is_some() && receipt.exact_process_identity.is_some());
    assert_eq!(
        receipt.force_killed,
        matches!(path, TerminalPath::ForcedCancel)
    );
    let unsafe_receipt = failed_worker.is_some() || matches!(path, TerminalPath::WaitFailure);
    receipt.uncertain == unsafe_receipt && receipt.effect_incapable() != unsafe_receipt
}

#[test]
fn shutdown_join_uncertainty_is_sticky_across_terminal_paths_and_workers() {
    let mut mismatches = Vec::new();
    for path in [
        TerminalPath::Completed,
        TerminalPath::CancelledCompleted,
        TerminalPath::CancelledStreamCompleted,
        TerminalPath::Timeout,
        TerminalPath::ForcedCancel,
        TerminalPath::ProcessorFailure,
        TerminalPath::CancelledProcessorFailure,
    ] {
        for worker in [None, Some("stdout"), Some("stderr"), Some("stdin")] {
            if !terminal_schedule(path, worker) {
                mismatches.push(format!("{path:?}/{worker:?}"));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "unsafe shutdown receipts: {mismatches:?}"
    );
}

struct FinishProcessor(bool);
impl StdoutProcessor for FinishProcessor {
    type Output = CapturedBytes;
    fn push(&mut self, _: &[u8]) -> Result<(), ProviderClientError> {
        Ok(())
    }
    fn finish(self, _: Option<ProviderClientError>) -> CapturedBytes {
        assert!(!self.0, "injected EOF finish panic");
        CapturedBytes::default()
    }
}

#[test]
fn timeout_pipe_eof_worker_panic_is_unsafe_but_healthy_neighbor_is_settled() {
    let mut mismatches = Vec::new();
    for panic in [false, true] {
        let custody = AttemptActorCustody::new(uuid::Uuid::new_v4());
        let guard = custody.begin("launch");
        let error = ProcessRunner::new(ProcessLimits {
            custody: Some(guard.0.clone()),
            timeout: Duration::from_millis(100),
            ..ProcessLimits::default()
        })
        .run_with_timeout_mode_and_stdout_processor(
            ProcessCommand::new("/bin/sh").arg("-c").arg("sleep 30"),
            vec![],
            std::iter::empty::<(&str, &str)>(),
            TimeoutMode::TotalRuntime,
            FinishProcessor(panic),
        )
        .expect_err("timeout");
        assert_eq!(error.transport_kind(), "host_timeout");
        assert_eq!(error.request_id(), None);
        assert!(error.diagnostics().process_was_reaped);
        if panic {
            assert!(
                error
                    .diagnostics()
                    .description
                    .as_deref()
                    .unwrap()
                    .contains("stdout")
            );
        }
        drop(guard);
        let receipt = custody.receipts().remove(0);
        assert!(receipt.leader_reaped && receipt.process_tree_terminated);
        if receipt.uncertain != panic || receipt.effect_incapable() == panic {
            mismatches.push(panic);
        }
    }
    assert!(
        mismatches.is_empty(),
        "incorrect EOF receipts for panic={mismatches:?}"
    );
}

#[test]
fn first_wait_failure_survives_collection_and_cannot_make_custody_safe() {
    for worker in [None, Some("stdout"), Some("stderr"), Some("stdin")] {
        assert!(terminal_schedule(TerminalPath::WaitFailure, worker));
    }
}

#[test]
fn first_wait_failure_survives_pending_collection_without_inventing_errno_or_status() {
    let command = ProcessCommand::new("/not-logged")
        .arg("launch")
        .arg("secret-argv");
    let original = wait_operation_error(
        &command,
        "cleanup_admission_or_signal",
        std::io::Error::other("process cleanup admission unavailable"),
    );
    let diagnostics = termination_diagnostics_from_joined(
        JoinedProcessThreads::<CapturedBytes> {
            stdout: None,
            stderr: None,
            stdin_closed_early: None,
            failed_workers: vec!["stderr"],
            cleanup_pending: true,
        },
        TerminatedProcess {
            status: None,
            force_killed: false,
        },
        false,
    );
    let error = wait_failure_with_process_context(original, diagnostics, None);
    assert_eq!(error.transport_kind(), "wait_failed");
    assert_eq!(error.request_id(), None);
    assert_eq!(error.process_status(), None);
    assert!(!error.diagnostics().process_was_reaped);
    assert!(!error.diagnostics().process_was_force_killed);
    assert_eq!(
        error.diagnostics().description.as_deref(),
        Some(
            "first_failure: cleanup_admission_or_signal: process cleanup admission unavailable; errno=unavailable; collection: process_cleanup_pending; worker output may be incomplete; process worker thread panicked: stderr"
        )
    );
}

#[test]
fn supervisor_preserves_actual_waitid_error_after_owned_reap() {
    let custody = AttemptActorCustody::new(uuid::Uuid::new_v4());
    let guard = custody.begin("launch");
    let limits = ProcessLimits {
        custody: Some(guard.0.clone()),
        ..ProcessLimits::default()
    };
    let command = ProcessCommand::new("/bin/true").arg("launch");
    let mut child = spawn_provider_process(
        &command,
        std::iter::empty::<(&str, &str)>(),
        limits.custody.clone(),
        limits.spawn_observer.is_none(),
    )
    .unwrap();
    let (publisher, events) = process_event_bus();
    let threads = start_process_threads(
        &mut child,
        vec![],
        limits.stderr_limit,
        publisher,
        None,
        ByteCaptureProcessor::new(limits.stdout_limit),
    );
    // Deterministic fixture: the same owner reaps first. No other waiter steals
    // status, and this is not a proposed explanation of any historical failure.
    child.wait().unwrap();
    let now = Instant::now();
    let error = ProcessSupervisor {
        argv: command.argv(),
        child,
        command,
        threads,
        events,
        timeout_mode: TimeoutMode::TotalRuntime,
        started: now,
        next_status_poll: now,
        last_stdout_line: now,
        cancellation_started: None,
        _cancellation_registration: None,
        limits: &limits,
    }
    .run()
    .expect_err("already reaped child must fail waitid");
    let detail = error.diagnostics().description.as_deref().unwrap();
    assert!(
        detail.starts_with("first_failure: waitid_wnowait: "),
        "{error:?}"
    );
    assert!(
        detail.contains(&format!("errno={}", libc::ECHILD)),
        "{error:?}"
    );
    assert_eq!(error.transport_kind(), "wait_failed");
    drop(guard);
    let receipt = custody.receipts().remove(0);
    assert!(receipt.leader_reaped && receipt.uncertain);
    assert!(!receipt.effect_incapable());
}
