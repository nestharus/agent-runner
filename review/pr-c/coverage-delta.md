# PR-C Coverage Delta Audit

## Findings

1. Medium: `trace::build_trace_session()` still has untested `TranscriptState::Missing` branches in [src-tauri/src/trace/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:278). The suite covers `unresolved`, `no_locator`, and `available` in [src-tauri/src/trace/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:817), [src-tauri/src/trace/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:894), and [src-tauri/src/trace/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:959), but there is no test for either `Ok(Some(_path))` where the file is absent or `Err(err)` from `locate_transcript()`. That leaves the user-visible degraded state required by `V10` unexercised.

2. Medium: `executor::cli` coverage stops short of several capture edge paths in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:388). The new tests cover FFV happy/mismatch and JsonEvent happy/missing-event in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:766), [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:821), [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:870), and [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:925), but there is still no assertion for the `CapturePlan::None` path, no negative test for tmpfile restoration fallback in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:429), no test for `system.init` non-match/missing-`session_id` in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:453), and no test for dotted/missing `lookup_json_path()` traversal in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:493).

3. Low: `locate_transcript()` only has `None`, success, and non-zero-exit coverage in [src-tauri/src/sessions/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/sessions/mod.rs:404). The timeout branch in `run_session_script()` and malformed locator stdout branches in [src-tauri/src/sessions/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/sessions/mod.rs:168) are still untested.

4. Low: the reference script integration test covers happy paths only in [src-tauri/tests/pr_c_locator_scripts.rs](/home/nes/projects/agent-runner/src-tauri/tests/pr_c_locator_scripts.rs:30). There is no negative case asserting the contract when a session is not found, even though both scripts return exit `1` for that condition.

5. Low: `SessionCapture::validate()` has good required-field coverage for FFV and JsonEvent in [src-tauri/src/config/model.rs](/home/nes/projects/agent-runner/src-tauri/src/config/model.rs:716), but there is still no direct test for the `SessionCaptureKind::None => Ok(())` branch in [src-tauri/src/config/model.rs](/home/nes/projects/agent-runner/src-tauri/src/config/model.rs:61).

## Checkpoints

1. `SessionCapture::validate()`:
Status: Partial.
Evidence: FFV missing-`flag` is covered in [src-tauri/src/config/model.rs](/home/nes/projects/agent-runner/src-tauri/src/config/model.rs:716). JsonEvent missing `json_flag`, `last_message_flag`, `event_type`, and `event_id_path` are covered in [src-tauri/src/config/model.rs](/home/nes/projects/agent-runner/src-tauri/src/config/model.rs:733). The `kind = none` success branch at [src-tauri/src/config/model.rs](/home/nes/projects/agent-runner/src-tauri/src/config/model.rs:63) is not directly tested.

2. Executor dispatch in `cli.rs`:
Status: Partial.
Evidence: FFV happy and mismatch are covered in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:766) and [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:821). JsonEvent happy and missing-event are covered in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:870) and [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:925). There is no dedicated assertion for the `CapturePlan::None` path in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:390).

3. Codex tmpfile restoration:
Status: Partial.
Evidence: Happy path is covered in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:870). The fallback path when the tmpfile is missing/unreadable in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:437) is untested.

4. Claude readback parsing:
Status: Partial.
Evidence: A direct `system.init` match is exercised indirectly by [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:766). There is no test for a non-match path in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:453), either “no `system.init` line” or “`system.init` present but missing `session_id`”.

5. `lookup_json_path` dotted traversal:
Status: Missing.
Evidence: The only JsonEvent success test uses a top-level `thread_id` field in [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:884), so [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:493) is not directly exercised for dotted traversal or missing-key failure.

6. `locate_transcript`:
Status: Partial.
Evidence: `Ok(None)`, `Ok(Some(_))`, and an `Err` from non-zero exit are covered in [src-tauri/src/sessions/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/sessions/mod.rs:404), [src-tauri/src/sessions/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/sessions/mod.rs:413), and [src-tauri/src/sessions/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/sessions/mod.rs:435). The underlying timeout path in `run_session_script()` is not covered, and neither are empty-stdout or multi-line-stdout errors from [src-tauri/src/sessions/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/sessions/mod.rs:168).

7. `update_session_capture`:
Status: Covered.
Evidence: `Some(session_id)` is covered in [src-tauri/src/state/db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2197). `None` is covered in [src-tauri/src/state/db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2230).

8. Trace `transcript_state` resolution:
Status: Partial.
Evidence: `Unresolved` is covered in [src-tauri/src/trace/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:817). `NoLocator` is covered in [src-tauri/src/trace/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:959). `Available` is covered in [src-tauri/src/trace/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:894). `Missing` from [src-tauri/src/trace/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:303) and [src-tauri/src/trace/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:315) is not covered.

9. Reference scripts:
Status: Partial.
Evidence: Claude content-match, Claude filename-match, and Codex happy path are covered in [src-tauri/tests/pr_c_locator_scripts.rs](/home/nes/projects/agent-runner/src-tauri/tests/pr_c_locator_scripts.rs:30), [src-tauri/tests/pr_c_locator_scripts.rs](/home/nes/projects/agent-runner/src-tauri/tests/pr_c_locator_scripts.rs:58), and [src-tauri/tests/pr_c_locator_scripts.rs](/home/nes/projects/agent-runner/src-tauri/tests/pr_c_locator_scripts.rs:85). There is no negative integration test for “session not found”.

## Verification

Passed targeted checks:

- `cargo test executor::cli::tests -- --nocapture`
- `cargo test locate_transcript -- --nocapture`
- `cargo test json_output_reports_ -- --nocapture`
- `cargo test update_session_capture -- --nocapture`
- `cargo test --test pr_c_locator_scripts -- --nocapture`
