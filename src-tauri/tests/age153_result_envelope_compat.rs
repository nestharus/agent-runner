#![cfg(unix)]

mod age153_support;

use age153_support::{
    Age153Fixture, FORCE_TERMINAL_SIGNAL_KIND, assert_no_terminal_marker_on_stdout,
    assert_result_envelope_shape, assert_signal_consumer_source_wired, quota_body, success_body,
    toml_string,
};
use oulipoly_provider::stream::LaunchStreamLimits;
use serde_json::Value;
#[cfg(target_os = "linux")]
use std::fs::OpenOptions;
#[cfg(target_os = "linux")]
use std::io::{Read as _, Write as _};
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd as _, FromRawFd as _};
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt as _;
use std::process::Command;
#[cfg(target_os = "linux")]
use std::process::{Child, ExitStatus, Stdio};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

const EXTERNAL_PROVIDER: &str = concat!("clau", "de-age153-result");

#[test]
fn external_provider_spooled_success_emits_ordered_result_envelope() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-success.txt");
    fixture.write_model("age153-result", &[EXTERNAL_PROVIDER]);
    fixture.write_providers_with_bodies(&[(
        EXTERNAL_PROVIDER,
        &success_body(&marker, "result-compatible stdout"),
    )]);

    let output = fixture.run_one_shot("age153-result");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    assert_eq!(output.stdout, b"result-compatible stdout\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let envelope = assert_success_result_envelope(&stderr);
    assert_canonical_result_and_trace_agree(&fixture, &envelope);
    assert_delivered_delivery(&fixture);
    assert_signal_consumer_source_wired(
        "fn emit_result_envelope_line(",
        &["emit_marker_line(output, \"OULIPOLY_RESULT\""],
    );
    assert_signal_consumer_source_wired(
        "fn emit_marker_line(",
        &[
            "output.write_all(marker.as_bytes())",
            "output.write_all(b\"=\")",
            "output.write_all(json.as_bytes())",
        ],
    );
}

#[test]
fn external_provider_spooled_success_without_trailing_newline_preserves_stdout() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-no-newline.txt");
    fixture.write_model("age153-result-no-newline", &[EXTERNAL_PROVIDER]);
    let body = format!(
        "printf '%s\\n' ran >> {}\nprintf '%s' 'result-without-newline'",
        toml_string(&marker.display().to_string())
    );
    fixture.write_providers_with_bodies(&[(EXTERNAL_PROVIDER, &body)]);

    let output = fixture.run_one_shot("age153-result-no-newline");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"result-without-newline");
    assert_success_result_envelope(&String::from_utf8_lossy(&output.stderr));
}

#[test]
fn external_provider_spooled_binary_stdout_is_byte_exact() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-binary.txt");
    fixture.write_model("age153-result-binary", &[EXTERNAL_PROVIDER]);
    let body = format!(
        "printf '%s\\n' ran >> {}\nprintf '\\000\\377PNG\\r\\n\\032\\n\\200tail'",
        toml_string(&marker.display().to_string())
    );
    fixture.write_providers_with_bodies(&[(EXTERNAL_PROVIDER, &body)]);

    let output = fixture.run_one_shot("age153-result-binary");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        output.stdout, b"\x00\xffPNG\r\n\x1a\n\x80tail",
        "provider stdout must remain byte-exact"
    );
    assert_success_result_envelope(&String::from_utf8_lossy(&output.stderr));
}

#[test]
fn merged_tee_stream_orders_result_after_unterminated_provider_stdout() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-merged-tee.txt");
    let log = fixture.dir.path().join("result-merged-tee.log");
    fixture.write_model("age153-result-merged-tee", &[EXTERNAL_PROVIDER]);
    let body = format!(
        "printf '%s\\n' ran >> {}\nprintf '%s' 'merged-without-newline'",
        toml_string(&marker.display().to_string())
    );
    fixture.write_providers_with_bodies(&[(EXTERNAL_PROVIDER, &body)]);

    let output = run_one_shot_through_tee(&fixture, "age153-result-merged-tee", &log);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(std::fs::read(&log).expect("read tee log"), output.stdout);
    let merged = String::from_utf8(output.stdout).expect("merged text output");
    assert!(
        merged.contains("merged-without-newline\nOULIPOLY_RESULT="),
        "{merged}"
    );
    let envelope = assert_success_result_envelope(&merged);
    assert_canonical_result_and_trace_agree(&fixture, &envelope);
}

#[test]
fn fresh_merged_delivery_is_complete_beyond_retained_bound_and_precedes_result() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-large-fresh.txt");
    let log = fixture.dir.path().join("result-large-fresh.log");
    let stream_bytes = LaunchStreamLimits::default().retained_output_bytes + 1;
    fixture.write_model("age153-result-large-fresh", &[EXTERNAL_PROVIDER]);
    fixture.write_providers_with_bodies(&[(
        EXTERNAL_PROVIDER,
        &large_provider_output_body(&marker, stream_bytes),
    )]);

    let output = run_one_shot_through_tee(&fixture, "age153-result-large-fresh", &log);

    assert_complete_merged_spooled_delivery(&fixture, &output, &log, stream_bytes, false);
}

#[test]
fn unterminated_provider_stderr_alone_separates_final_result_in_direct_and_merged_capture() {
    const PROVIDER_STDERR: &[u8] = b"stderr-only-without-newline";

    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-stderr-no-newline.txt");
    let log = fixture.dir.path().join("result-stderr-no-newline.log");
    fixture.write_model("age153-result-stderr-no-newline", &[EXTERNAL_PROVIDER]);
    let body = format!(
        "printf '%s\\n' ran >> {}\nprintf '%s' 'stderr-only-without-newline' >&2",
        toml_string(&marker.display().to_string())
    );
    fixture.write_providers_with_bodies(&[(EXTERNAL_PROVIDER, &body)]);

    let direct = fixture.run_one_shot("age153-result-stderr-no-newline");

    assert_eq!(direct.status.code(), Some(0), "{direct:?}");
    assert!(direct.stdout.is_empty(), "{direct:?}");
    let direct_stderr = String::from_utf8(direct.stderr.clone()).expect("direct stderr utf8");
    let direct_invocation_id = invocation_id_from_capture(&direct_stderr);
    assert_provider_stderr_precedes_anchored_result(
        &direct.stderr,
        PROVIDER_STDERR,
        &direct_stderr,
    );
    assert_eq!(
        matching_result_count(&direct_stderr, &direct_invocation_id),
        1,
        "{direct_stderr}"
    );
    assert_genuine_runner_success(&final_matching_result_envelope(
        &direct_stderr,
        &direct_invocation_id,
    ));

    let merged = run_one_shot_through_tee(&fixture, "age153-result-stderr-no-newline", &log);

    assert_eq!(merged.status.code(), Some(0), "{merged:?}");
    assert!(merged.stderr.is_empty(), "{merged:?}");
    assert_eq!(std::fs::read(&log).expect("read tee log"), merged.stdout);
    let merged_capture = String::from_utf8(merged.stdout.clone()).expect("merged capture utf8");
    let merged_invocation_id = invocation_id_from_capture(&merged_capture);
    assert_provider_stderr_precedes_anchored_result(
        &merged.stdout,
        PROVIDER_STDERR,
        &merged_capture,
    );
    assert_eq!(
        matching_result_count(&merged_capture, &merged_invocation_id),
        1,
        "{merged_capture}"
    );
    let result = final_matching_result_envelope(&merged_capture, &merged_invocation_id);
    assert_genuine_runner_success(&result);
    assert_canonical_result_and_trace_agree(&fixture, &result);
}

#[test]
fn fresh_hostile_matching_provider_markers_preserve_bytes_and_leave_final_result_authoritative() {
    let fixture = hostile_fresh_fixture("fresh-hostile");

    let direct = fixture.run_one_shot("age153-result-hostile-fresh");

    assert_hostile_direct_capture(&direct, "fresh-hostile");

    let log = fixture.dir.path().join("result-hostile-fresh.log");
    let merged = run_one_shot_through_tee(&fixture, "age153-result-hostile-fresh", &log);

    assert_hostile_merged_capture(&fixture, &merged, &log, "fresh-hostile");
}

#[test]
fn resume_external_provider_spooled_success_emits_ordered_result_envelope() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-resume.txt");
    fixture.write_resume_pool(
        "age153-result-resume",
        &[(
            EXTERNAL_PROVIDER,
            success_body(&marker, "resume result-compatible stdout"),
        )],
    );
    fixture.stage_active_session_jsonl(EXTERNAL_PROVIDER);
    fixture.seed_active_chain(EXTERNAL_PROVIDER, "age153-result-resume");

    let output = fixture.run_resume("age153-result-resume");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    assert_eq!(output.stdout, b"resume result-compatible stdout\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let envelope = assert_success_result_envelope(&stderr);
    assert_canonical_result_and_trace_agree(&fixture, &envelope);
    assert_delivered_delivery(&fixture);
}

#[test]
fn resume_external_provider_without_trailing_newline_preserves_stdout() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("resume-result-no-newline.txt");
    let body = format!(
        "printf '%s\\n' ran >> {}\nprintf '%s' 'resume-without-newline'",
        toml_string(&marker.display().to_string())
    );
    fixture.write_resume_pool(
        "age153-resume-result-no-newline",
        &[(EXTERNAL_PROVIDER, body)],
    );
    fixture.stage_active_session_jsonl(EXTERNAL_PROVIDER);
    fixture.seed_active_chain(EXTERNAL_PROVIDER, "age153-resume-result-no-newline");

    let output = fixture.run_resume("age153-resume-result-no-newline");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"resume-without-newline");
    assert_success_result_envelope(&String::from_utf8_lossy(&output.stderr));
}

#[test]
fn resume_merged_delivery_is_complete_beyond_retained_bound_and_precedes_result() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-large-resume.txt");
    let log = fixture.dir.path().join("result-large-resume.log");
    let stream_bytes = LaunchStreamLimits::default().retained_output_bytes + 1;
    fixture.write_resume_pool(
        "age153-result-large-resume",
        &[(
            EXTERNAL_PROVIDER,
            large_provider_output_body(&marker, stream_bytes),
        )],
    );
    fixture.stage_active_session_jsonl(EXTERNAL_PROVIDER);
    fixture.seed_active_chain(EXTERNAL_PROVIDER, "age153-result-large-resume");

    let output = run_resume_through_tee(&fixture, "age153-result-large-resume", &log);

    assert_complete_merged_spooled_delivery(&fixture, &output, &log, stream_bytes, true);
}

#[test]
fn resume_hostile_matching_provider_markers_preserve_bytes_and_leave_final_result_authoritative() {
    let fixture = hostile_resume_fixture("resume-hostile");

    let direct = fixture.run_resume("age153-result-hostile-resume");

    assert_hostile_direct_capture(&direct, "resume-hostile");

    let log = fixture.dir.path().join("result-hostile-resume.log");
    let merged = run_resume_through_tee(&fixture, "age153-result-hostile-resume", &log);

    assert_hostile_merged_capture(&fixture, &merged, &log, "resume-hostile");
}

#[test]
fn terminal_signal_marker_stays_on_stderr_when_spooled_success_emits_result() {
    let fixture = Age153Fixture::new();
    let first_marker = fixture.dir.path().join("result-quota-a.txt");
    let sibling_marker = fixture.dir.path().join("result-quota-b.txt");
    fixture.write_model(
        "age153-result-quota",
        &["claude-age153-a", "claude-age153-b"],
    );
    fixture.write_providers_with_bodies(&[
        ("claude-age153-a", &quota_body(&first_marker, 42)),
        (
            "claude-age153-b",
            &success_body(&sibling_marker, "result sibling stdout"),
        ),
    ]);

    let output = fixture.run_one_shot_with_env(
        "age153-result-quota",
        &[(FORCE_TERMINAL_SIGNAL_KIND, "QuotaExhaustedInband,None")],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    assert_eq!(output.stdout, b"result sibling stdout\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OULIPOLY_TERMINAL_SIGNAL="), "{stderr}");
    assert_success_result_envelope(&stderr);
}

#[cfg(target_os = "linux")]
#[test]
fn fresh_payload_write_failure_marks_output_delivery_failed() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-payload-write-failure.txt");
    fixture.write_model("age153-result-payload-failure", &[EXTERNAL_PROVIDER]);
    fixture.write_providers_with_bodies(&[(
        EXTERNAL_PROVIDER,
        &success_body(&marker, "payload that cannot be delivered"),
    )]);

    let output = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("age153-result-payload-failure")
        .arg("prompt")
        .stdout(Stdio::from(dev_full()))
        .output()
        .expect("run with failing stdout");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to deliver provider output"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_no_matching_success_result_on_surviving_channels(&output);
    assert_failed_delivery(&fixture, b"payload that cannot be delivered\n");
}

#[cfg(target_os = "linux")]
#[test]
fn fresh_control_record_write_failure_marks_output_delivery_failed() {
    assert_control_write_stays_failed_until_delivery_is_confirmed(false);
}

#[cfg(target_os = "linux")]
#[test]
fn resume_control_record_write_failure_marks_output_delivery_failed() {
    assert_control_write_stays_failed_until_delivery_is_confirmed(true);
}

#[cfg(target_os = "linux")]
#[test]
fn resume_payload_write_failure_marks_output_delivery_failed() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("resume-payload-write-failure.txt");
    fixture.write_resume_pool(
        "age153-resume-payload-failure",
        &[(
            EXTERNAL_PROVIDER,
            success_body(&marker, "resume payload that cannot be delivered"),
        )],
    );
    fixture.stage_active_session_jsonl(EXTERNAL_PROVIDER);
    fixture.seed_active_chain(EXTERNAL_PROVIDER, "age153-resume-payload-failure");

    let output = fixture
        .command()
        .arg("-m")
        .arg("age153-resume-payload-failure")
        .arg("--resume")
        .arg(age153_support::SESSION_ID)
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("continue after quota")
        .current_dir(fixture.dir.path())
        .stdout(Stdio::from(dev_full()))
        .output()
        .expect("run resume with failing stdout");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to deliver provider output"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_no_matching_success_result_on_surviving_channels(&output);
    assert_failed_delivery(&fixture, b"resume payload that cannot be delivered\n");
}

#[cfg(target_os = "linux")]
#[test]
fn shared_fresh_and_resume_delivery_owner_fails_closed_while_payload_write_is_blocked() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("result-blocked-delivery.txt");
    let (mut stdout, stdout_sink, pipe_capacity) = stdout_pipe();
    let stream_bytes = pipe_capacity * 2 + 1;
    fixture.write_model("age153-result-blocked-delivery", &[EXTERNAL_PROVIDER]);
    fixture.write_providers_with_bodies(&[(
        EXTERNAL_PROVIDER,
        &format!(
            "printf '%s\\n' ran >> {}\npython3 -c 'import os; os.write(1, b\"P\" * {stream_bytes})'",
            toml_string(&marker.display().to_string())
        ),
    )]);

    let mut child = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("age153-result-blocked-delivery")
        .arg("prompt")
        .stdout(stdout_sink)
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn runner with held stdout pipe");

    let mut first_provider_byte = [0];
    stdout
        .read_exact(&mut first_provider_byte)
        .expect("production delivery must begin writing provider stdout");
    assert_eq!(first_provider_byte, [b'P']);
    assert!(
        child.try_wait().expect("inspect blocked runner").is_none(),
        "runner must remain blocked in the deliberately undrained provider payload write"
    );

    let unconfirmed = latest_output_delivery(&fixture);
    assert_eq!(
        unconfirmed.provider_outcome_state, "settled",
        "{unconfirmed:?}"
    );
    assert_eq!(unconfirmed.delivery_state, "failed", "{unconfirmed:?}");
    assert_eq!(unconfirmed.delivered_at, None, "{unconfirmed:?}");
    assert_eq!(
        unconfirmed.delivery_failure_stage.as_deref(),
        Some("delivery_confirmation"),
        "{unconfirmed:?}"
    );
    assert_eq!(
        unconfirmed.delivery_failure_kind.as_deref(),
        Some("unconfirmed"),
        "{unconfirmed:?}"
    );
    assert_shared_spooled_delivery_owner_for_fresh_and_resume();

    let mut provider_stdout = first_provider_byte.to_vec();
    stdout
        .read_to_end(&mut provider_stdout)
        .expect("release and drain held provider payload");
    let status = child.wait().expect("wait for released delivery");
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("runner stderr")
        .read_to_end(&mut stderr)
        .expect("read runner stderr");

    assert_eq!(status.code(), Some(0));
    assert_eq!(provider_stdout, vec![b'P'; stream_bytes]);
    let result = assert_success_result_envelope(&String::from_utf8(stderr).expect("stderr utf8"));
    assert_canonical_result_and_trace_agree(&fixture, &result);
    let delivered = latest_output_delivery(&fixture);
    assert_eq!(delivered.invocation_id, unconfirmed.invocation_id);
    assert_eq!(delivered.provider_outcome_state, "settled", "{delivered:?}");
    assert_eq!(delivered.delivery_state, "delivered", "{delivered:?}");
    assert!(delivered.delivered_at.is_some(), "{delivered:?}");
    assert_eq!(delivered.delivery_failure_stage, None, "{delivered:?}");
    assert_eq!(delivered.delivery_failure_kind, None, "{delivered:?}");
}

fn large_provider_output_body(marker: &std::path::Path, stream_bytes: usize) -> String {
    format!(
        "printf '%s\\n' ran >> {}\npython3 -c 'import os; n={stream_bytes}; os.write(1, b\"O\" * n + b\"\\n\"); os.write(2, b\"E\" * n + b\"\\n\")'",
        toml_string(&marker.display().to_string())
    )
}

fn assert_complete_merged_spooled_delivery(
    fixture: &Age153Fixture,
    output: &std::process::Output,
    log: &std::path::Path,
    stream_bytes: usize,
    is_resume: bool,
) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        std::fs::read(log).expect("read large tee log"),
        output.stdout
    );

    let merged = String::from_utf8(output.stdout.clone()).expect("large merged capture utf8");
    let invocation_id = invocation_id_from_capture(&merged);
    assert_eq!(matching_result_count(&merged, &invocation_id), 1);
    let result = final_matching_result_envelope(&merged, &invocation_id);
    assert_genuine_runner_success(&result);

    let final_line = merged.lines().last().expect("final merged result line");
    let final_result: Value = serde_json::from_str(
        final_line
            .strip_prefix("OULIPOLY_RESULT=")
            .unwrap_or_else(|| panic!("final merged line is not an anchored result: {final_line}")),
    )
    .expect("parse final merged result");
    assert!(result_envelope_has_valid_shape(&final_result));
    assert_eq!(final_result["id"], invocation_id);
    assert_eq!(final_result, result);
    let mut final_record = final_line.as_bytes().to_vec();
    final_record.push(b'\n');
    let result_start = output
        .stdout
        .len()
        .checked_sub(final_record.len())
        .expect("capture must contain final result record");
    assert_eq!(&output.stdout[result_start..], final_record);

    let mut expected_stderr = vec![b'E'; stream_bytes];
    expected_stderr.push(b'\n');
    let provider_start = output.stdout[..result_start]
        .windows(expected_stderr.len())
        .position(|window| window == expected_stderr)
        .expect("complete provider stderr replay must delimit provider output");
    assert_runner_records_before_provider(
        &output.stdout[..provider_start],
        &invocation_id,
        is_resume,
    );

    let mut expected_provider_region = expected_stderr;
    expected_provider_region.extend(std::iter::repeat_n(b'O', stream_bytes));
    expected_provider_region.push(b'\n');
    assert_eq!(
        &output.stdout[provider_start..result_start],
        expected_provider_region,
        "provider-output region must be exactly one complete stderr replay followed by one complete stdout replay"
    );
    assert_canonical_result_and_trace_agree(fixture, &result);
    assert_delivered_delivery(fixture);
}

fn assert_runner_records_before_provider(prefix: &[u8], invocation_id: &str, is_resume: bool) {
    assert!(
        prefix.ends_with(b"\n"),
        "runner record prefix must be line-framed"
    );
    let prefix = std::str::from_utf8(prefix).expect("runner record prefix utf8");
    let mut invocation_records = 0;
    let mut session_records = 0;
    let mut admission_states = Vec::new();
    let mut resume_short_lines = 0;
    for line in prefix.lines() {
        if let Some(payload) = line.strip_prefix("OULIPOLY_INVOCATION=") {
            let value: Value = serde_json::from_str(payload).expect("invocation record JSON");
            assert_eq!(value["id"], invocation_id);
            invocation_records += 1;
        } else if let Some(payload) = line.strip_prefix("OULIPOLY_SESSION=") {
            let value: Value = serde_json::from_str(payload).expect("session record JSON");
            assert_eq!(value["id"], invocation_id);
            assert_eq!(value["agent_runner_invocation_id"], invocation_id);
            session_records += 1;
        } else if let Some(payload) = line.strip_prefix("OULIPOLY_SESSION_ADMISSION=") {
            let value: Value = serde_json::from_str(payload).expect("session admission JSON");
            assert_eq!(value["registration_identity"], invocation_id);
            match value["state"].as_str() {
                Some("queued") => {
                    assert_eq!(value["reason"], "fifo_wait");
                    assert_eq!(value["queue_sequence"], 1);
                    assert_eq!(value.as_object().expect("admission object").len(), 4);
                    admission_states.push("queued");
                }
                Some("launching") => {
                    if is_resume {
                        assert_eq!(value["session_id"], age153_support::SESSION_ID);
                    } else {
                        assert!(value["session_id"].is_null(), "{value}");
                    }
                    assert_eq!(value.as_object().expect("admission object").len(), 3);
                    admission_states.push("launching");
                }
                state => panic!("unexpected session admission state {state:?}: {value}"),
            }
        } else if line == format!("[resume] -> {EXTERNAL_PROVIDER}") {
            resume_short_lines += 1;
        } else {
            panic!("unexpected bytes before provider-output region: {line:?}");
        }
    }
    assert_eq!(invocation_records, 1, "{prefix}");
    assert_eq!(session_records, 0, "{prefix}");
    assert_eq!(admission_states, ["queued", "launching"], "{prefix}");
    assert_eq!(resume_short_lines, usize::from(is_resume), "{prefix}");
}

#[cfg(target_os = "linux")]
fn assert_no_matching_success_result_on_surviving_channels(output: &std::process::Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let invocation_id = [stdout.as_ref(), stderr.as_ref()]
        .into_iter()
        .find_map(invocation_id_from_capture_opt)
        .unwrap_or_else(|| panic!("missing invocation marker in surviving output: {output:?}"));

    assert_no_matching_success_result_in_streams(&output.stdout, &output.stderr, &invocation_id);
}

#[cfg(target_os = "linux")]
fn assert_no_matching_success_result_in_streams(stdout: &[u8], stderr: &[u8], invocation_id: &str) {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);

    for (channel, stream) in [("stdout", stdout.as_ref()), ("stderr", stderr.as_ref())] {
        assert!(
            matching_result_envelopes(stream, invocation_id)
                .all(|result| result["success"] != true),
            "delivery failure emitted a matching shape-valid success result on {channel}:\n{stream}"
        );
    }
}

fn run_one_shot_through_tee(
    fixture: &Age153Fixture,
    model: &str,
    log: &std::path::Path,
) -> std::process::Output {
    let mut command = Command::new("bash");
    command
        .arg("-o")
        .arg("pipefail")
        .arg("-c")
        .arg(r#""$1" --models-dir "$2" --model "$3" prompt 2>&1 | tee "$4""#)
        .arg("age330-merged-tee")
        .arg(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .arg(&fixture.models_dir)
        .arg(model)
        .arg(log)
        .env("XDG_CONFIG_HOME", &fixture.config_home)
        .env("XDG_DATA_HOME", &fixture.data_home)
        .env(
            "OULIPOLY_DATA_DIR",
            fixture.data_home.join("oulipoly-agent-runner"),
        )
        .env("HOME", &fixture.data_home)
        .env_remove("OULIPOLY_CONFIG_HOME")
        .env_remove("OULIPOLY_PARENT_INVOCATION");
    command.output().expect("run through merged tee pipeline")
}

fn run_resume_through_tee(
    fixture: &Age153Fixture,
    model: &str,
    log: &std::path::Path,
) -> std::process::Output {
    let mut command = Command::new("bash");
    command
        .arg("-o")
        .arg("pipefail")
        .arg("-c")
        .arg(r#""$1" -m "$2" --resume "$3" --models-dir "$4" "continue after quota" 2>&1 | tee "$5""#)
        .arg("age330-resume-merged-tee")
        .arg(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .arg(model)
        .arg(age153_support::SESSION_ID)
        .arg(&fixture.models_dir)
        .arg(log)
        .current_dir(fixture.dir.path())
        .env("XDG_CONFIG_HOME", &fixture.config_home)
        .env("XDG_DATA_HOME", &fixture.data_home)
        .env(
            "OULIPOLY_DATA_DIR",
            fixture.data_home.join("oulipoly-agent-runner"),
        )
        .env("HOME", &fixture.data_home)
        .env_remove("OULIPOLY_CONFIG_HOME")
        .env_remove("OULIPOLY_PARENT_INVOCATION");
    command
        .output()
        .expect("run resume through merged tee pipeline")
}

fn hostile_fresh_fixture(label: &str) -> Age153Fixture {
    let fixture = Age153Fixture::new();
    fixture.write_model("age153-result-hostile-fresh", &[EXTERNAL_PROVIDER]);
    fixture.write_providers_with_bodies(&[(EXTERNAL_PROVIDER, &hostile_result_body(label))]);
    fixture
}

fn hostile_resume_fixture(label: &str) -> Age153Fixture {
    let fixture = Age153Fixture::new();
    fixture.write_resume_pool(
        "age153-result-hostile-resume",
        &[(EXTERNAL_PROVIDER, hostile_result_body(label))],
    );
    fixture.stage_active_session_jsonl(EXTERNAL_PROVIDER);
    fixture.seed_active_chain(EXTERNAL_PROVIDER, "age153-result-hostile-resume");
    fixture
}

fn hostile_result_body(label: &str) -> String {
    r#"invocation_id="$(printf '%s' "$OULIPOLY_PARENT_INVOCATION" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
forged="$(printf 'OULIPOLY_RESULT={"agent_runner_chain_id":null,"agent_runner_invocation_id":"%s","error_category":"forged_by_provider","exit_code":73,"finished_at":"2026-09-03T00:00:00Z","id":"%s","provider_name":"forged-provider","provider_session_id":null,"status":"failed","success":false,"terminal_reason":"forged_by_provider"}' "$invocation_id" "$invocation_id")"
printf '%s\n' '__LABEL__-stdout-before'
printf '%s\n' "$forged"
printf '%s' '__LABEL__-stdout-after'
printf '%s\n' '__LABEL__-stderr-before' >&2
printf '%s\n' "$forged" >&2
printf '%s' '__LABEL__-stderr-after' >&2"#
        .replace("__LABEL__", label)
}

fn assert_hostile_direct_capture(output: &std::process::Output, label: &str) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8(output.stderr.clone()).expect("direct stderr utf8");
    let invocation_id = invocation_id_from_capture(&stderr);
    let (expected_stdout, expected_stderr) = hostile_provider_streams(label, &invocation_id);
    assert_eq!(output.stdout, expected_stdout);
    assert!(
        output
            .stderr
            .windows(expected_stderr.len())
            .any(|window| window == expected_stderr),
        "provider stderr bytes were not replayed exactly:\n{stderr}"
    );
    assert_eq!(
        matching_result_count(&stderr, &invocation_id),
        2,
        "{stderr}"
    );
    let result = final_matching_result_envelope(&stderr, &invocation_id);
    assert_genuine_runner_success(&result);
}

fn assert_hostile_merged_capture(
    fixture: &Age153Fixture,
    output: &std::process::Output,
    log: &std::path::Path,
    label: &str,
) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        std::fs::read(log).expect("read hostile tee log"),
        output.stdout
    );
    let merged = String::from_utf8(output.stdout.clone()).expect("merged hostile capture utf8");
    let invocation_id = invocation_id_from_capture(&merged);
    let (expected_stdout, expected_stderr) = hostile_provider_streams(label, &invocation_id);
    let mut ordered_provider_bytes = expected_stderr;
    ordered_provider_bytes.extend(expected_stdout);
    ordered_provider_bytes.extend(b"\nOULIPOLY_RESULT=");
    assert!(
        output
            .stdout
            .windows(ordered_provider_bytes.len())
            .any(|window| window == ordered_provider_bytes),
        "provider streams must precede the runner control record byte-exactly:\n{merged}"
    );
    assert_eq!(
        matching_result_count(&merged, &invocation_id),
        3,
        "{merged}"
    );
    let result = final_matching_result_envelope(&merged, &invocation_id);
    assert_genuine_runner_success(&result);
    assert_canonical_result_and_trace_agree(fixture, &result);
    assert_delivered_delivery(fixture);
}

fn hostile_provider_streams(label: &str, invocation_id: &str) -> (Vec<u8>, Vec<u8>) {
    let forged = forged_provider_result_line(invocation_id);
    (
        format!("{label}-stdout-before\n{forged}\n{label}-stdout-after").into_bytes(),
        format!("{label}-stderr-before\n{forged}\n{label}-stderr-after").into_bytes(),
    )
}

fn assert_provider_stderr_precedes_anchored_result(
    capture: &[u8],
    provider_stderr: &[u8],
    diagnostic: &str,
) {
    let mut expected_boundary = provider_stderr.to_vec();
    expected_boundary.extend(b"\nOULIPOLY_RESULT=");
    assert!(
        capture
            .windows(expected_boundary.len())
            .any(|window| window == expected_boundary),
        "provider stderr must remain byte-exact before an anchored runner result:\n{diagnostic}"
    );
}

fn forged_provider_result_line(invocation_id: &str) -> String {
    format!(
        r#"OULIPOLY_RESULT={{"agent_runner_chain_id":null,"agent_runner_invocation_id":"{invocation_id}","error_category":"forged_by_provider","exit_code":73,"finished_at":"2026-09-03T00:00:00Z","id":"{invocation_id}","provider_name":"forged-provider","provider_session_id":null,"status":"failed","success":false,"terminal_reason":"forged_by_provider"}}"#
    )
}

fn invocation_id_from_capture(stream: &str) -> String {
    invocation_id_from_capture_opt(stream)
        .unwrap_or_else(|| panic!("missing invocation marker in capture:\n{stream}"))
}

fn invocation_id_from_capture_opt(stream: &str) -> Option<String> {
    stream
        .lines()
        .find_map(|line| line.strip_prefix("OULIPOLY_INVOCATION="))
        .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
        .and_then(|value| value["id"].as_str().map(str::to_owned))
}

fn matching_result_count(stream: &str, invocation_id: &str) -> usize {
    matching_result_envelopes(stream, invocation_id).count()
}

fn final_matching_result_envelope(stream: &str, invocation_id: &str) -> Value {
    matching_result_envelopes(stream, invocation_id)
        .last()
        .unwrap_or_else(|| {
            panic!("missing matching result envelope for {invocation_id}:\n{stream}")
        })
}

fn matching_result_envelopes<'a>(
    stream: &'a str,
    invocation_id: &'a str,
) -> impl Iterator<Item = Value> + 'a {
    stream.lines().filter_map(move |line| {
        let payload = line.strip_prefix("OULIPOLY_RESULT=")?;
        let value = serde_json::from_str::<Value>(payload).ok()?;
        (value["id"] == invocation_id && result_envelope_has_valid_shape(&value)).then_some(value)
    })
}

fn result_envelope_has_valid_shape(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let mut expected = std::collections::BTreeSet::from([
        "error_category",
        "exit_code",
        "finished_at",
        "id",
        "status",
        "success",
        "terminal_reason",
    ]);
    if value["success"] == false {
        expected.extend([
            "agent_runner_invocation_id",
            "provider_name",
            "provider_session_id",
            "agent_runner_chain_id",
        ]);
    } else if value["success"] != true {
        return false;
    }
    object
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>()
        == expected
}

fn assert_genuine_runner_success(result: &Value) {
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["success"], true);
    assert_eq!(result["exit_code"], 0);
    assert!(result["error_category"].is_null());
    assert!(result["terminal_reason"].is_null());
}

#[cfg(target_os = "linux")]
fn assert_control_write_stays_failed_until_delivery_is_confirmed(is_resume: bool) {
    const MODEL: &str = "age153-result-control-failure";
    const RESULT_MARKER: &[u8] = b"OULIPOLY_RESULT";
    const BOUND: Duration = Duration::from_secs(10);

    let fixture = Age153Fixture::new();
    let (mut stdout, stdout_sink, stdout_capacity) = stdout_pipe();
    let provider_stdout_len = stdout_capacity * 2 + 1;
    let mut expected_provider_stdout = vec![b'P'; provider_stdout_len];
    *expected_provider_stdout
        .last_mut()
        .expect("nonempty payload") = b'\n';
    let body = format!(
        "python3 -c 'import sys; sys.stdout.buffer.write(b\"P\" * ({provider_stdout_len} - 1) + b\"\\n\"); sys.stdout.buffer.flush()'"
    );
    if is_resume {
        fixture.write_resume_pool(MODEL, &[(EXTERNAL_PROVIDER, body)]);
        fixture.stage_active_session_jsonl(EXTERNAL_PROVIDER);
        fixture.seed_active_chain(EXTERNAL_PROVIDER, MODEL);
    } else {
        fixture.write_model(MODEL, &[EXTERNAL_PROVIDER]);
        fixture.write_providers_with_bodies(&[(EXTERNAL_PROVIDER, &body)]);
    }

    let (mut stderr, stderr_sink, mut stderr_filler) = stderr_pipe();
    let mut command = fixture.command();
    if is_resume {
        command
            .arg("-m")
            .arg(MODEL)
            .arg("--resume")
            .arg(age153_support::SESSION_ID)
            .arg("--models-dir")
            .arg(&fixture.models_dir)
            .arg("continue after quota")
            .current_dir(fixture.dir.path());
    } else {
        command
            .arg("--models-dir")
            .arg(&fixture.models_dir)
            .arg("--model")
            .arg(MODEL)
            .arg("prompt");
    }
    command
        .stdout(stdout_sink)
        .stderr(stderr_sink)
        .process_group(0);
    let mut child = ProcessGroupChild::new(command.spawn().expect("spawn controlled runner"));
    drop(command);

    wait_until_pipe_full(&stdout, stdout_capacity, BOUND);
    assert!(
        child
            .child_mut()
            .try_wait()
            .expect("inspect stdout-blocked runner")
            .is_none(),
        "runner must remain blocked in production provider-stdout delivery"
    );
    let captured_stderr = drain_available(&mut stderr);
    let captured_stderr_text =
        String::from_utf8(captured_stderr.clone()).expect("pre-control stderr utf8");
    let invocation_id = invocation_id_from_capture(&captured_stderr_text);
    fill_pipe(&stderr, &mut stderr_filler);
    drop(stderr_filler);

    let mut provider_stdout = vec![0; provider_stdout_len];
    stdout
        .read_exact(&mut provider_stdout)
        .expect("read complete provider stdout delivery");
    assert_eq!(provider_stdout, expected_provider_stdout);

    wait_for_blocked_write(child.id(), libc::STDERR_FILENO, RESULT_MARKER, BOUND);
    assert_eq!(
        pipe_bytes_available(&stdout),
        0,
        "complete provider stdout must be drained before inspecting the control write"
    );
    let unconfirmed = output_delivery_for_invocation(&fixture, &invocation_id);
    assert_eq!(
        unconfirmed.provider_outcome_state, "settled",
        "{unconfirmed:?}"
    );
    assert_eq!(unconfirmed.delivery_state, "failed", "{unconfirmed:?}");
    assert_eq!(unconfirmed.delivered_at, None, "{unconfirmed:?}");
    assert_eq!(
        unconfirmed.delivery_failure_stage.as_deref(),
        Some("delivery_confirmation"),
        "{unconfirmed:?}"
    );
    assert_eq!(
        unconfirmed.delivery_failure_kind.as_deref(),
        Some("unconfirmed"),
        "{unconfirmed:?}"
    );

    drop(stderr);
    let status = child.wait_bounded(BOUND);
    stdout
        .read_to_end(&mut provider_stdout)
        .expect("finish captured provider stdout");

    assert!(
        !status.success(),
        "control-write failure must fail the runner"
    );
    assert_eq!(provider_stdout, expected_provider_stdout);
    assert_no_matching_success_result_in_streams(
        &provider_stdout,
        &captured_stderr,
        &invocation_id,
    );
    assert_failed_delivery_for_invocation(&fixture, &invocation_id, &provider_stdout);
}

fn assert_success_result_envelope(stream: &str) -> Value {
    let envelope = assert_result_envelope_shape(stream);
    assert_eq!(envelope["status"], "succeeded");
    assert_eq!(envelope["success"], true);
    assert_eq!(envelope["exit_code"], 0);
    assert!(envelope["error_category"].is_null());
    assert!(envelope["terminal_reason"].is_null());
    envelope
}

fn assert_canonical_result_and_trace_agree(fixture: &Age153Fixture, envelope: &Value) {
    let invocation_id = envelope["id"].as_str().expect("result invocation id");
    let artifact_path = fixture
        .data_home
        .join("oulipoly-agent-runner")
        .join("invocations")
        .join(format!("{invocation_id}.result"));
    let artifact: Value = serde_json::from_slice(
        &std::fs::read(&artifact_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", artifact_path.display())),
    )
    .expect("parse result artifact");
    assert_terminal_outcome_matches(envelope, &artifact);

    let trace = fixture
        .command()
        .arg("trace")
        .arg(invocation_id)
        .arg("--json")
        .output()
        .expect("run trace");
    assert_eq!(trace.status.code(), Some(0), "{trace:?}");
    let trace: Value = serde_json::from_slice(&trace.stdout).expect("parse trace");
    let invocation = &trace["root"]["invocation"];
    assert_eq!(invocation["id"], invocation_id);
    assert_terminal_outcome_matches(envelope, invocation);
}

fn assert_terminal_outcome_matches(expected: &Value, actual: &Value) {
    for field in [
        "id",
        "status",
        "success",
        "exit_code",
        "error_category",
        "terminal_reason",
    ] {
        assert_eq!(actual[field], expected[field], "terminal field {field}");
    }
}

#[derive(Debug)]
struct OutputDeliveryRow {
    invocation_id: i64,
    provider_outcome_state: String,
    delivery_state: String,
    delivered_at: Option<String>,
    delivery_failure_stage: Option<String>,
    delivery_failure_kind: Option<String>,
}

fn latest_output_delivery(fixture: &Age153Fixture) -> OutputDeliveryRow {
    fixture
        .conn()
        .query_row(
            "SELECT invocation_id, provider_outcome_state, delivery_state, delivered_at,
                    delivery_failure_stage, delivery_failure_kind
             FROM invocation_output_deliveries
             ORDER BY invocation_id DESC
             LIMIT 1",
            [],
            |row| {
                Ok(OutputDeliveryRow {
                    invocation_id: row.get(0)?,
                    provider_outcome_state: row.get(1)?,
                    delivery_state: row.get(2)?,
                    delivered_at: row.get(3)?,
                    delivery_failure_stage: row.get(4)?,
                    delivery_failure_kind: row.get(5)?,
                })
            },
        )
        .expect("read invocation output delivery")
}

#[cfg(target_os = "linux")]
fn output_delivery_for_invocation(
    fixture: &Age153Fixture,
    invocation_uuid: &str,
) -> OutputDeliveryRow {
    fixture
        .conn()
        .query_row(
            "SELECT invocation_id, provider_outcome_state, delivery_state, delivered_at,
                    delivery_failure_stage, delivery_failure_kind
             FROM invocation_output_deliveries
             WHERE invocation_uuid = ?1",
            [invocation_uuid],
            |row| {
                Ok(OutputDeliveryRow {
                    invocation_id: row.get(0)?,
                    provider_outcome_state: row.get(1)?,
                    delivery_state: row.get(2)?,
                    delivered_at: row.get(3)?,
                    delivery_failure_stage: row.get(4)?,
                    delivery_failure_kind: row.get(5)?,
                })
            },
        )
        .expect("read exact invocation output delivery")
}

fn assert_delivered_delivery(fixture: &Age153Fixture) {
    let row = latest_output_delivery(fixture);
    assert_eq!(row.provider_outcome_state, "settled", "{row:?}");
    assert_eq!(row.delivery_state, "delivered", "{row:?}");
    assert!(row.delivered_at.is_some(), "{row:?}");
    assert_eq!(row.delivery_failure_stage, None, "{row:?}");
    assert_eq!(row.delivery_failure_kind, None, "{row:?}");
}

#[cfg(target_os = "linux")]
fn stdout_pipe() -> (std::fs::File, Stdio, usize) {
    let mut fds = [-1; 2];
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    assert_eq!(rc, 0, "pipe2 failed: {}", std::io::Error::last_os_error());
    let read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let write = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    let capacity = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETPIPE_SZ) };
    assert!(
        capacity > 0,
        "F_GETPIPE_SZ failed: {}",
        std::io::Error::last_os_error()
    );
    (read, Stdio::from(write), capacity as usize)
}

#[cfg(target_os = "linux")]
fn stderr_pipe() -> (std::fs::File, Stdio, std::fs::File) {
    let mut fds = [-1; 2];
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    assert_eq!(rc, 0, "pipe2 failed: {}", std::io::Error::last_os_error());
    let read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let write = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    let filler = write.try_clone().expect("clone stderr pipe writer");
    (read, Stdio::from(write), filler)
}

#[cfg(target_os = "linux")]
fn drain_available(read: &mut std::fs::File) -> Vec<u8> {
    let flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
    assert!(
        flags >= 0,
        "F_GETFL failed: {}",
        std::io::Error::last_os_error()
    );
    let rc = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
    assert_eq!(rc, 0, "F_SETFL failed: {}", std::io::Error::last_os_error());

    let mut captured = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        match read.read(&mut buffer) {
            Ok(0) => panic!("stderr pipe closed before the control-delivery intervention"),
            Ok(count) => captured.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("drain pre-control stderr: {error}"),
        }
    }
    captured
}

#[cfg(target_os = "linux")]
fn fill_pipe(read: &std::fs::File, write: &mut std::fs::File) {
    assert_eq!(pipe_bytes_available(read), 0, "stderr pipe must be empty");
    let capacity = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETPIPE_SZ) };
    assert!(
        capacity > 0,
        "F_GETPIPE_SZ failed: {}",
        std::io::Error::last_os_error()
    );
    write
        .write_all(&vec![b'X'; capacity as usize])
        .expect("fill stderr pipe to its exact capacity");
    assert_eq!(
        pipe_bytes_available(read),
        capacity,
        "stderr pipe must be full before control delivery"
    );
}

#[cfg(target_os = "linux")]
fn pipe_bytes_available(read: &std::fs::File) -> i32 {
    let mut bytes = 0;
    let rc = unsafe { libc::ioctl(read.as_raw_fd(), libc::FIONREAD, &mut bytes) };
    assert_eq!(
        rc,
        0,
        "FIONREAD failed: {}",
        std::io::Error::last_os_error()
    );
    bytes
}

#[cfg(target_os = "linux")]
fn wait_until_pipe_full(read: &std::fs::File, capacity: usize, timeout: Duration) {
    let started = Instant::now();
    let mut descriptor = libc::pollfd {
        fd: read.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut descriptor, 1, timeout.as_millis() as i32) };
    assert_eq!(
        rc, 1,
        "provider stdout delivery did not begin within {timeout:?}"
    );
    assert_ne!(descriptor.revents & (libc::POLLIN | libc::POLLHUP), 0);

    let mut last_available = 0;
    while started.elapsed() < timeout {
        last_available = pipe_bytes_available(read);
        if last_available == capacity as i32 {
            return;
        }
        std::thread::yield_now();
    }
    panic!(
        "provider stdout pipe did not reach its exact {capacity}-byte capacity within {timeout:?}; last available bytes: {last_available}"
    );
}

#[cfg(target_os = "linux")]
fn wait_for_blocked_write(pid: u32, fd: i32, expected: &[u8], timeout: Duration) {
    let started = Instant::now();
    let mut last_syscall = String::new();
    while started.elapsed() < timeout {
        if let Ok(syscall) = std::fs::read_to_string(format!("/proc/{pid}/syscall")) {
            last_syscall = syscall;
            let fields = last_syscall.split_whitespace().collect::<Vec<_>>();
            if fields.len() >= 4
                && fields[0].parse::<i64>().ok() == Some(libc::SYS_write)
                && parse_proc_number(fields[1]) == Some(fd as u64)
                && parse_proc_number(fields[3]) == Some(expected.len() as u64)
            {
                let address = parse_proc_number(fields[2]).expect("blocked write buffer address");
                let mut actual = vec![0; expected.len()];
                let local = libc::iovec {
                    iov_base: actual.as_mut_ptr().cast(),
                    iov_len: actual.len(),
                };
                let remote = libc::iovec {
                    iov_base: address as *mut libc::c_void,
                    iov_len: actual.len(),
                };
                let read = unsafe { libc::process_vm_readv(pid as i32, &local, 1, &remote, 1, 0) };
                assert_eq!(
                    read,
                    expected.len() as isize,
                    "read blocked write buffer: {}",
                    std::io::Error::last_os_error()
                );
                assert_eq!(
                    actual, expected,
                    "runner must be blocked in result-control delivery"
                );
                return;
            }
        }
        std::thread::yield_now();
    }
    panic!(
        "runner did not block in the expected stderr control write within {timeout:?}; last syscall: {last_syscall}"
    );
}

#[cfg(target_os = "linux")]
fn parse_proc_number(value: &str) -> Option<u64> {
    value.strip_prefix("0x").map_or_else(
        || value.parse().ok(),
        |hex| u64::from_str_radix(hex, 16).ok(),
    )
}

#[cfg(target_os = "linux")]
struct ProcessGroupChild {
    child: Child,
    reaped: bool,
}

#[cfg(target_os = "linux")]
impl ProcessGroupChild {
    fn new(child: Child) -> Self {
        Self {
            child,
            reaped: false,
        }
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    fn wait_bounded(&mut self, timeout: Duration) -> ExitStatus {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if let Some(status) = self.child.try_wait().expect("wait for controlled runner") {
                self.reaped = true;
                return status;
            }
            std::thread::yield_now();
        }
        panic!("controlled runner did not exit within {timeout:?}");
    }
}

#[cfg(target_os = "linux")]
impl Drop for ProcessGroupChild {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "linux")]
fn assert_shared_spooled_delivery_owner_for_fresh_and_resume() {
    let fresh = include_str!("../src/run/balancing/finalization.rs");
    let resume = include_str!("../src/run/resume/finalization.rs");
    let shared_call = "crate::run::spooled_success_delivery::settle(";
    assert!(
        fresh.contains(shared_call),
        "fresh success must use the shared delivery settlement owner"
    );
    assert!(
        resume.contains(shared_call),
        "resume success must use the same shared delivery settlement owner exercised by this test"
    );
}

#[cfg(target_os = "linux")]
fn dev_full() -> std::fs::File {
    OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full")
}

#[cfg(target_os = "linux")]
fn assert_failed_delivery(fixture: &Age153Fixture, expected_stdout: &[u8]) {
    let row = fixture
        .conn()
        .query_row(
            "SELECT i.status, i.success, i.exit_code,
                    d.provider_outcome_state, d.delivery_state,
                    d.delivery_failure_stage, d.delivery_failure_kind, d.stdout_path
             FROM invocation_output_deliveries d
             JOIN invocations i ON i.id = d.invocation_id
             ORDER BY d.invocation_id DESC
             LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .expect("read invocation output delivery");

    assert_eq!(row.0, "succeeded");
    assert_eq!(row.1, 1);
    assert_eq!(row.2, 0);
    assert_eq!(row.3, "settled");
    assert_eq!(row.4, "failed");
    assert_eq!(row.5.as_deref(), Some("payload_or_control"));
    assert!(
        row.6.as_ref().is_some_and(|kind| !kind.is_empty()),
        "{row:?}"
    );
    assert_eq!(
        std::fs::read(&row.7).expect("read retained stdout"),
        expected_stdout
    );
}

#[cfg(target_os = "linux")]
fn assert_failed_delivery_for_invocation(
    fixture: &Age153Fixture,
    invocation_uuid: &str,
    expected_stdout: &[u8],
) {
    let row = fixture
        .conn()
        .query_row(
            "SELECT i.status, i.success, i.exit_code,
                    d.provider_outcome_state, d.delivery_state,
                    d.delivery_failure_stage, d.delivery_failure_kind, d.stdout_path
             FROM invocation_output_deliveries d
             JOIN invocations i ON i.id = d.invocation_id
             WHERE d.invocation_uuid = ?1",
            [invocation_uuid],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .expect("read exact failed invocation output delivery");

    assert_eq!(row.0, "succeeded");
    assert_eq!(row.1, 1);
    assert_eq!(row.2, 0);
    assert_eq!(row.3, "settled");
    assert_eq!(row.4, "failed");
    assert_eq!(row.5.as_deref(), Some("payload_or_control"));
    assert!(
        row.6.as_ref().is_some_and(|kind| !kind.is_empty()),
        "{row:?}"
    );
    assert_eq!(
        std::fs::read(&row.7).expect("read retained stdout"),
        expected_stdout
    );
}
