//! RC-1 (F1) reproduction: pipeline-status propagation gap on abnormal
//! upstream termination.
//!
//! ## Symptom
//!
//! When an `oulipoly-agent-runner` invocation runs inside a shell pipeline
//! that ends in `| tail -N` (the shape used by the Claude Code background
//! task harness — see incident transcript fc7d9c2c-…jsonl:1895 and
//! findings.md questions 1 & 5), abnormal termination of the upstream
//! `agents` process is invisible to the parent harness:
//!
//! 1. The pipeline exit status reflects the last stage (`tail`), not the
//!    upstream runner. Even after `agents` is killed, `bash -c '... | tail -3'`
//!    typically exits 0.
//! 2. `tail -3` buffers its input and only emits the final 3 lines on EOF,
//!    so the harness never sees the runner's stderr stream — only the early
//!    `OULIPOLY_INVOCATION=` header (which was the only line emitted before
//!    the kill).
//! 3. `state.db` retains the invocation in `status=running` with no
//!    `terminal_reason`, `success`, `exit_code`, or `finished_at`, because
//!    SIGKILL cannot run `finalize_invocation` (see
//!    `crates/oulipoly-state/src/db.rs:1768`).
//! 4. No filesystem artifact under `XDG_STATE_HOME`/`XDG_DATA_HOME` records
//!    the terminal status either — a repo-wide search for
//!    `OULIPOLY_(RESULT|TERMINAL|ENVELOPE|FINAL)` returns zero hits.
//!
//! The combined result is that the parent harness reports
//! `status=completed exit=0` while the upstream `agents` row is stuck
//! `running` forever, exactly matching the rows captured in the incident
//! evidence (state.db rows 5257, 5459, 5481).
//!
//! ## What this test reproduces
//!
//! This test mechanically reproduces the F1 symptom by:
//!
//! 1. Spawning `oulipoly-agent-runner` inside `bash -c '... 2>&1 | tail -3'`
//!    against a fake provider that hangs in `sleep`, so we can SIGKILL the
//!    runner mid-flight without racing against normal lifecycle completion.
//! 2. Discovering the runner PID via `/proc/*/cmdline` filtered by the
//!    fixture's unique tempdir marker (no production-code change required to
//!    expose a PID side-channel; see "Caveats" below).
//! 3. Sending SIGKILL to the runner — the realistic harness-kill signal.
//! 4. Asserting both halves of the symptom:
//!    - **stale-running**: the invocation row is left in `status=running`
//!      with no terminal metadata (this assertion PASSES on current main and
//!      documents the lifecycle gap).
//!    - **recoverability**: at least one of the three terminal-status
//!      recovery channels (Channel A = post-pipeline stdout, Channel B =
//!      raw stderr bypass, Channel C = filesystem artifact) yields a
//!      structured terminal envelope for the killed invocation. This
//!      assertion FAILS on current main and is the red half of the test.
//!
//! ## Fix-decision neutrality
//!
//! The recovery assertion deliberately ORs three independent channels so any
//! of the four design options the rca-orchestrator considers downstream
//! (terminal-envelope-last on stdout, mirrored stderr signal, filesystem
//! `.result` artifact, or a hybrid) will turn the test green when it emits
//! exact `OULIPOLY_RESULT=`, exact `OULIPOLY_TERMINAL_SIGNAL=`, or strict
//! filesystem result-artifact evidence for the killed invocation.
//!
//! ## Caveats / known gaps
//!
//! - `state.db` does not carry a `pid` column for `invocations`, and there
//!   is no existing `OULIPOLY_PID=` marker on stdout/stderr. Discovering the
//!   runner PID by scanning `/proc/*/cmdline` for the fixture tempdir marker
//!   is the cleanest mechanism available without changing production code,
//!   per the dispatch brief. If a future fix adds a PID side-channel, this
//!   helper can be simplified.
//! - The kill signal is SIGKILL because that is the realistic harness-kill
//!   shape (SIGKILL bypasses any drop-handler / panic-hook based finalize).
//!   A SIGTERM variant would exercise a different code path (graceful
//!   shutdown handlers, if any) and is out of scope here; it can be a
//!   follow-up scenario file under this same RCA module.
//! - This test does not exercise the orphaned provider child after the
//!   runner dies (F2/F3 raw-artifact preservation, structured lifecycle
//!   logs) — those are separate Work Units under this RCA.
//!
//! Declared roles: orchestration, formatter, validator, accessor, predicate,
//! mapper.

use std::io::Read;
use std::process::{Child, ExitStatus, Output, Stdio};
use std::thread;
use std::thread::sleep;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use oulipoly_state::{InvocationRecord, InvocationStatus};

use super::{
    filesystem_artifact_recovers_terminal, find_runner_pid_for_fixture, list_files_recursive,
    poll_first_running_uuid, sigkill_pid, text_contains_terminal_envelope, PipelineFixture,
};

/// Slow enough to give us a wide kill window even on a loaded CI runner; the
/// test is bounded by the kill timing, not the sleep duration.
const PROVIDER_HANG_SECONDS: u32 = 10;

/// The fixture-script body: emit a heartbeat byte so the runner sees output
/// activity and forwards it onto its own stderr pipe, then block in `sleep`
/// long enough for the test to detect the invocation row and kill the
/// runner. The heartbeat is on stderr because the runner pipes provider
/// stderr separately and the fixture pattern in `pr_a_invocation_integration`
/// already demonstrates this is harmless.
fn hanging_provider_script() -> String {
    format!(
        r#"printf 'fixture-provider-running\n' >&2
sleep {PROVIDER_HANG_SECONDS}
printf 'fixture-provider-unexpectedly-survived\n' >&2
exit 0"#
    )
}

struct KillRunOutcome {
    invocation_uuid: String,
    pipeline_stdout: String,
    pipeline_stderr: String,
    pipeline_exit_code: Option<i32>,
    runner_pid: i32,
}

/// Spawn the runner inside `bash -c '<agents> 2>&1 | tail -3'`, wait for the
/// invocation row to appear in state.db, SIGKILL the runner, then collect
/// the pipeline's stdout (which is `tail -3`'s output of merged
/// stdout+stderr) and exit status.
fn run_pipeline_then_kill_runner(fixture: &PipelineFixture) -> KillRunOutcome {
    let child = spawn_tail_pipeline(fixture);
    let invocation_uuid = wait_for_pipeline_invocation_uuid(fixture);
    pause_before_killing_runner();
    let runner_pid = locate_pipeline_runner_pid(fixture);
    kill_pipeline_runner_or_panic(runner_pid);
    let output = wait_for_killed_child_output(
        child,
        Duration::from_secs(30),
        "bash pipeline did not exit within 30s of SIGKILL to runner",
    );
    outcome_from_process_output(invocation_uuid, output, runner_pid)
}

/// Spawn the runner directly (no bash, no `2>&1 | tail -3`) so stderr is
/// captured raw. Used to evaluate Channel B (raw-stderr bypass) of the
/// recovery assertion.
fn run_direct_then_kill_runner(fixture: &PipelineFixture) -> KillRunOutcome {
    let child = spawn_direct_runner(fixture);
    let invocation_uuid = wait_for_direct_invocation_uuid(fixture);
    pause_before_killing_runner();
    let runner_pid = child.id() as i32;
    kill_direct_runner_or_panic(runner_pid);
    let output = wait_for_killed_child_output(
        child,
        Duration::from_secs(30),
        "direct runner did not exit within 30s of SIGKILL",
    );
    outcome_from_process_output(invocation_uuid, output, runner_pid)
}

fn format_tail_pipeline_command(fixture: &PipelineFixture) -> String {
    let runner = env!("CARGO_BIN_EXE_oulipoly-agent-runner");
    format!(
        "{runner} --models-dir {models_dir} --model fixture ping 2>&1 | tail -3",
        models_dir = fixture.models_dir.display(),
    )
}

fn spawn_tail_pipeline(fixture: &PipelineFixture) -> Child {
    let mut cmd = std::process::Command::new("bash");
    cmd.arg("-c").arg(format_tail_pipeline_command(fixture));
    configure_capture_command(&mut cmd);
    fixture.apply_env(&mut cmd);
    cmd.spawn().expect("failed to spawn bash pipeline")
}

fn spawn_direct_runner(fixture: &PipelineFixture) -> Child {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    cmd.arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("fixture")
        .arg("ping");
    configure_capture_command(&mut cmd);
    fixture.apply_env(&mut cmd);
    cmd.spawn().expect("failed to spawn agent-runner directly")
}

fn configure_capture_command(cmd: &mut std::process::Command) {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
}

fn wait_for_pipeline_invocation_uuid(fixture: &PipelineFixture) -> String {
    poll_first_running_uuid(fixture, Duration::from_secs(15))
        .expect("invocation row never appeared in state.db within 15s")
}

fn wait_for_direct_invocation_uuid(fixture: &PipelineFixture) -> String {
    poll_first_running_uuid(fixture, Duration::from_secs(15))
        .expect("invocation row never appeared in state.db within 15s (direct run)")
}

fn pause_before_killing_runner() {
    // Tiny grace period so the runner is actually inside `wait()` on the
    // provider child rather than mid-startup when we kill it.
    sleep(Duration::from_millis(150));
}

fn locate_pipeline_runner_pid(fixture: &PipelineFixture) -> i32 {
    find_runner_pid_for_fixture(fixture).unwrap_or_else(|| {
        panic!(
            "could not locate runner PID via /proc/*/cmdline using fixture marker {:?}",
            fixture.fixture_marker
        )
    })
}

fn kill_pipeline_runner_or_panic(runner_pid: i32) {
    // SIGKILL is the realistic harness-kill signal (incident transcript
    // recorded `status=killed` with no signal trace, consistent with
    // SIGKILL from a background-task supervisor). The runner is a child of
    // the bash wrapper, not of this test process, so we cannot wait on it
    // directly; instead we wait on the bash pipeline which reaps the runner
    // zombie as part of its own shutdown.
    assert!(
        sigkill_pid(runner_pid),
        "kill -KILL {runner_pid} failed; runner may have already exited"
    );
}

fn kill_direct_runner_or_panic(runner_pid: i32) {
    assert!(
        sigkill_pid(runner_pid),
        "kill -KILL {runner_pid} failed (direct run)"
    );
}

fn wait_for_killed_child_output(child: Child, timeout: Duration, failure_message: &str) -> Output {
    // Tail will see EOF on stdin once the runner's stdout fd closes; bash
    // then reaps tail and the runner and exits. Bound the wait so a hung
    // pipeline produces a useful failure rather than the test deadlining.
    wait_with_timeout(child, timeout).expect(failure_message)
}

fn outcome_from_process_output(
    invocation_uuid: String,
    output: Output,
    runner_pid: i32,
) -> KillRunOutcome {
    KillRunOutcome {
        invocation_uuid,
        pipeline_stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        pipeline_stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        pipeline_exit_code: output.status.code(),
        runner_pid,
    }
}

#[test]
fn terminal_status_recoverable_after_external_kill_under_tail_pipeline() {
    // Scenario 1: the actual incident shape — runner under `bash -c
    // '... 2>&1 | tail -3'`. Channels A (post-pipeline stdout) and C
    // (filesystem artifact) are evaluated against this run.
    let fixture_pipeline = PipelineFixture::with_script_body(&hanging_provider_script());
    let pipeline = run_pipeline_then_kill_runner(&fixture_pipeline);

    // ---- Stale-running half. This documents the F1 lifecycle gap and is
    // expected to PASS on current main. It is included so Phase 2 / Phase 3
    // can use the same test as a regression fixture after the fix lands.
    let row = assert_stale_running_after_sigkill(&fixture_pipeline, &pipeline);

    // Scenario 2: the same scenario WITHOUT the `2>&1 | tail -3` wrapper.
    // A fresh fixture is used so state.db rows do not collide between the
    // two scenarios — each fixture has its own data_home / state_home, so a
    // recovery channel that writes a per-invocation artifact in one
    // scenario will not satisfy the OR for the other scenario.
    let fixture_direct = PipelineFixture::with_script_body(&hanging_provider_script());
    let direct = run_direct_then_kill_runner(&fixture_direct);

    assert_recovery_predicate(compute_recovery_predicate(
        &fixture_pipeline,
        &pipeline,
        &fixture_direct,
        &direct,
        &row,
    ));
}

struct RecoveryPredicateResult {
    recovered: bool,
    diagnostics: String,
}

fn compute_recovery_predicate(
    fixture_pipeline: &PipelineFixture,
    pipeline: &KillRunOutcome,
    fixture_direct: &PipelineFixture,
    direct: &KillRunOutcome,
    row: &InvocationRecord,
) -> RecoveryPredicateResult {
    // Demonstrate the masking shape: the bash pipeline almost certainly
    // exited 0 (because tail succeeded) even though the upstream runner was
    // killed. This is informational, not load-bearing for the OR below.
    let pipeline_masking = format_pipeline_masking(pipeline);

    // Channel A: did the pipeline's post-tail stdout surface a terminal
    // envelope for the killed invocation?
    let channels = evaluate_recovery_channels(fixture_pipeline, pipeline, fixture_direct, direct);

    // Snapshot the filesystem-artifact candidate trees for the failure
    // message — Phase 2 reads these to confirm no artifact exists.
    let listing_pipeline = collect_fixture_file_listings(fixture_pipeline);
    let listing_direct = collect_fixture_file_listings(fixture_direct);

    RecoveryPredicateResult {
        recovered: channels.recovered(),
        diagnostics: format_recovery_failure_diagnostics(
            pipeline,
            direct,
            &pipeline_masking,
            &channels,
            &listing_pipeline,
            &listing_direct,
            row,
        ),
    }
}

fn assert_recovery_predicate(predicate_result: RecoveryPredicateResult) {
    assert!(
        predicate_result.recovered,
        "{}",
        predicate_result.diagnostics
    );
}

fn format_pipeline_masking(pipeline: &KillRunOutcome) -> String {
    format!(
        "pipeline exit_code={:?} (tail-3 output is {} bytes; bash pipeline succeeded \
         despite SIGKILL of upstream runner pid={})",
        pipeline.pipeline_exit_code,
        pipeline.pipeline_stdout.len(),
        pipeline.runner_pid
    )
}

fn assert_stale_running_after_sigkill(
    fixture_pipeline: &PipelineFixture,
    pipeline: &KillRunOutcome,
) -> InvocationRecord {
    let row = read_invocation_row_after_pipeline(fixture_pipeline, pipeline);

    assert_eq!(
        row.status,
        InvocationStatus::Running,
        "stale-running half: invocation row should still be `running` after SIGKILL \
         (row={row:?})"
    );
    assert_eq!(
        row.success, None,
        "stale-running half: success column should be NULL on abnormal kill (row={row:?})"
    );
    assert_eq!(
        row.exit_code, None,
        "stale-running half: exit_code should be NULL on abnormal kill (row={row:?})"
    );
    assert_eq!(
        row.terminal_reason, None,
        "stale-running half: terminal_reason should be NULL on abnormal kill (row={row:?})"
    );
    assert_eq!(
        row.finished_at, None,
        "stale-running half: finished_at should be NULL on abnormal kill (row={row:?})"
    );

    row
}

fn read_invocation_row_after_pipeline(
    fixture_pipeline: &PipelineFixture,
    pipeline: &KillRunOutcome,
) -> InvocationRecord {
    let db = fixture_pipeline.open_db();
    db.get_invocation_by_uuid(&pipeline.invocation_uuid)
        .expect("state.db query failed")
        .unwrap_or_else(|| {
            panic!(
                "no invocation row found for UUID {} after pipeline shape; \
                 pipeline_stdout={:?} pipeline_stderr={:?}",
                pipeline.invocation_uuid, pipeline.pipeline_stdout, pipeline.pipeline_stderr
            )
        })
}

struct RecoveryChannels {
    channel_a: bool,
    channel_b: bool,
    channel_c_pipeline: bool,
    channel_c_direct: bool,
}

impl RecoveryChannels {
    fn channel_c(&self) -> bool {
        self.channel_c_pipeline || self.channel_c_direct
    }

    fn recovered(&self) -> bool {
        self.channel_a || self.channel_b || self.channel_c()
    }
}

fn evaluate_recovery_channels(
    fixture_pipeline: &PipelineFixture,
    pipeline: &KillRunOutcome,
    fixture_direct: &PipelineFixture,
    direct: &KillRunOutcome,
) -> RecoveryChannels {
    RecoveryChannels {
        channel_a: pipeline_stdout_has_terminal_envelope(pipeline),
        channel_b: direct_stderr_has_terminal_envelope(direct),
        channel_c_pipeline: fixture_artifact_recovers_terminal(fixture_pipeline, pipeline),
        channel_c_direct: fixture_artifact_recovers_terminal(fixture_direct, direct),
    }
}

fn pipeline_stdout_has_terminal_envelope(pipeline: &KillRunOutcome) -> bool {
    text_contains_terminal_envelope(&pipeline.pipeline_stdout, &pipeline.invocation_uuid)
}

fn direct_stderr_has_terminal_envelope(direct: &KillRunOutcome) -> bool {
    text_contains_terminal_envelope(&direct.pipeline_stderr, &direct.invocation_uuid)
}

fn fixture_artifact_recovers_terminal(fixture: &PipelineFixture, outcome: &KillRunOutcome) -> bool {
    filesystem_artifact_recovers_terminal(fixture.dir.path(), &outcome.invocation_uuid)
}

fn collect_fixture_file_listings(fixture: &PipelineFixture) -> Vec<String> {
    list_files_recursive(fixture.dir.path())
        .into_iter()
        .map(|p| p.display().to_string())
        .collect()
}

fn format_recovery_failure_diagnostics(
    pipeline: &KillRunOutcome,
    direct: &KillRunOutcome,
    pipeline_masking: &str,
    channels: &RecoveryChannels,
    listing_pipeline: &[String],
    listing_direct: &[String],
    row: &InvocationRecord,
) -> String {
    format!(
        "F1 recovery half: none of channels A/B/C recovered terminal status for the killed \
         invocations.\n\
         \n\
         pipeline invocation_uuid: {}\n\
         direct   invocation_uuid: {}\n\
         \n\
         {pipeline_masking}\n\
         \n\
         Channel A (post-pipeline `tail -3` stdout) matched: {channel_a}\n\
           captured pipeline_stdout (truncated to 4096B): {pipeline_stdout_trunc:?}\n\
           captured pipeline_stderr (truncated to 4096B): {pipeline_stderr_trunc:?}\n\
         \n\
         Channel B (raw stderr bypass) matched: {channel_b}\n\
           captured direct_stderr (truncated to 4096B): {direct_stderr_trunc:?}\n\
           captured direct_stdout (truncated to 4096B): {direct_stdout_trunc:?}\n\
           direct pipeline exit_code: {:?}\n\
         \n\
         Channel C (filesystem artifact under XDG_*) matched: {channel_c} \
         (pipeline={channel_c_pipeline}, direct={channel_c_direct})\n\
           pipeline fixture file listing ({n_pipeline} files): {listing_pipeline:?}\n\
           direct   fixture file listing ({n_direct} files): {listing_direct:?}\n\
         \n\
         state.db row for pipeline invocation: {row:?}\n",
        pipeline.invocation_uuid,
        direct.invocation_uuid,
        direct.pipeline_exit_code,
        channel_a = channels.channel_a,
        channel_b = channels.channel_b,
        channel_c = channels.channel_c(),
        channel_c_pipeline = channels.channel_c_pipeline,
        channel_c_direct = channels.channel_c_direct,
        pipeline_stdout_trunc = truncate(&pipeline.pipeline_stdout, 4096),
        pipeline_stderr_trunc = truncate(&pipeline.pipeline_stderr, 4096),
        direct_stderr_trunc = truncate(&direct.pipeline_stderr, 4096),
        direct_stdout_trunc = truncate(&direct.pipeline_stdout, 4096),
        n_pipeline = listing_pipeline.len(),
        n_direct = listing_direct.len(),
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…[truncated {} more bytes]", &s[..max], s.len() - max)
    }
}

/// Bounded analogue of `Child::wait_with_output` that fails the test (via
/// `None`) instead of blocking forever if the child never exits. Reads
/// stdout/stderr in background threads so a blocked pipe cannot deadlock
/// the wait.
fn wait_with_timeout(mut child: Child, timeout: Duration) -> Option<Output> {
    let readers = spawn_child_pipe_readers(&mut child);
    let status = wait_child_until_deadline(&mut child, timeout);
    let stdout_bytes = join_pipe_reader_bytes(readers.stdout);
    let stderr_bytes = join_pipe_reader_bytes(readers.stderr);

    status.map(|status| assemble_process_output(status, stdout_bytes, stderr_bytes))
}

struct PipeReaders {
    stdout: Option<JoinHandle<Vec<u8>>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

fn spawn_child_pipe_readers(child: &mut Child) -> PipeReaders {
    PipeReaders {
        stdout: spawn_pipe_reader(child.stdout.take()),
        stderr: spawn_pipe_reader(child.stderr.take()),
    }
}

fn spawn_pipe_reader<R>(pipe: Option<R>) -> Option<JoinHandle<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    pipe.map(|mut stream| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf);
            buf
        })
    })
}

fn wait_child_until_deadline(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {}
            Err(_) => break None,
        }
        if child_wait_deadline_expired(deadline) {
            kill_timed_out_child(child);
            break None;
        }
        sleep(Duration::from_millis(50));
    }
}

fn child_wait_deadline_expired(deadline: Instant) -> bool {
    Instant::now() >= deadline
}

fn kill_timed_out_child(child: &mut Child) {
    // Best-effort: SIGKILL the wrapper before giving up so we do not leak the
    // bash process.
    let _ = child.kill();
    let _ = child.wait();
}

fn join_pipe_reader_bytes(handle: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default()
}

fn assemble_process_output(
    status: ExitStatus,
    stdout_bytes: Vec<u8>,
    stderr_bytes: Vec<u8>,
) -> Output {
    Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    }
}

#[cfg(test)]
#[path = "age158_rc1_characterization.rs"]
mod age158_rc1_characterization;
