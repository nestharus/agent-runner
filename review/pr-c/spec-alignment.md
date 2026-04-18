No spec-alignment findings.

The branch matches the current PR-C contract in `tmp/01-pr-c-contract.md`, including the amended `session_capture_method = "failed"` path and the four trace-time transcript states.

**Checks Passed**
- `SessionCapture` / `SessionCaptureKind` match the contract shape, and `validate()` enforces the required per-kind fields in [model.rs](/home/nes/projects/agent-runner/src-tauri/src/config/model.rs:32).
- `SessionSourceEntry` includes optional `transcript_locator`, with TOML parsing wired in [sessions.rs](/home/nes/projects/agent-runner/src-tauri/src/config/sessions.rs:12).
- `invocations.session_id`, `invocations.session_capture_method`, and `idx_invocations_provider_session` are present in both schema bootstrap and additive migration paths in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:455).
- `update_session_capture(id, session_id, method)` matches the contract signature and always persists the supplied method marker in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:826).
- `locate_transcript()` returns `Ok(None)` when no locator is configured and `Err(...)` on locator failures, reusing the shared session-script runner in [sessions/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/sessions/mod.rs:145).
- Executor dispatch is declarative on `SessionCaptureKind`, with forced-flag readback verification, stdout JSON-event capture, and tmpfile-based plain-text restoration in [cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:221).
- `ExecutionResult` is widened with `SessionCaptureResult` / `SessionCaptureMethod` in [executor/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/mod.rs:7).
- `main.rs` threads the capture result through lifecycle handling and calls `update_session_capture()` between executor return and `finalize_invocation()` in [main.rs](/home/nes/projects/agent-runner/src-tauri/src/main.rs:364).
- Trace integration accepts optional `&SessionsConfig` via the trace entrypoint signature, not via `TraceOptions`, which is allowed by the amended contract. The implementation resolves `unresolved`, `no_locator`, `missing`, and `available` in [trace/mod.rs](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:96).
- Reference locator scripts exist for Claude and Codex, and `pr_c_locator_scripts.rs` covers filename-match plus content-fallback paths in [claude-code-locate-transcript](/home/nes/projects/agent-runner/scripts/claude-code-locate-transcript:1), [codex-locate-transcript](/home/nes/projects/agent-runner/scripts/codex-locate-transcript:1), and [pr_c_locator_scripts.rs](/home/nes/projects/agent-runner/src-tauri/tests/pr_c_locator_scripts.rs:1).
- Anti-scope holds: `git diff --name-only main..HEAD` shows no PR-D sidechain capture work, no `claude-code-turns` changes, and no README sweep.
- No new Cargo dependencies were added; `git diff --name-only main..HEAD` contains no `Cargo.toml` or `Cargo.lock` changes.

**Assumption**
- I treated the amended PR-C contract as authoritative where older proposal text still mentions `unresolved` / `locator_error` wording. The implementation matches the amended contract's `failed` storage marker and four-state trace surface.

**Verification**
- `cargo test --manifest-path src-tauri/Cargo.toml --lib`
- `cargo test --manifest-path src-tauri/Cargo.toml --test pr_a_invocation_integration`
- `cargo test --manifest-path src-tauri/Cargo.toml --test pr_b_trace_integration`
- `cargo test --manifest-path src-tauri/Cargo.toml --test pr_c_locator_scripts`

All four commands passed.
