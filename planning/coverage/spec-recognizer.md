# spec-recognizer — Terminal-signal taxonomy per provider

## Source files

- `crates/oulipoly-runtime/src/executor/terminal_signal.rs`
- `crates/oulipoly-runtime/src/executor/providers/claude.rs`
- `crates/oulipoly-runtime/src/executor/providers/codex.rs`
- `crates/oulipoly-runtime/src/executor/providers/openai_compat.rs`
- `crates/oulipoly-runtime/src/executor/providers/mod.rs`

## Preconditions

- A child provider process has exited (cleanly or via signal); the runtime
  has captured its stdout, stderr, and exit status.
- The provider identity (`claude`, `codex`, `openai_compat`) is known so
  the matching per-provider recognizer is dispatched.
- Provider-specific marker conventions (OULIPOLY markers, JSON envelopes,
  exit-code semantics) are known to the recognizer.

## Input → Expected output

The recognizer maps `(provider, exit_status, stdout, stderr)` to one of a
finite signal taxonomy:

| Signal | Meaning | Trigger examples |
|--------|---------|-------------------|
| `ok` | Clean completion with a result. | Exit 0 + recognized result marker + non-empty body. |
| `aborted` | Cooperative cancellation by the user or harness. | Exit non-zero with an `aborted` marker, or a known abort signal. |
| `timeout` | Hit the harness-imposed wall-clock or idle timeout. | Watchdog killed the child before completion. |
| `auth_required` | Provider rejected credentials. | Stderr or JSON envelope carries an auth-prompt or 401/403 sentinel. |
| `rate_limited` | Provider rejected the request as rate-limited at request scope (not account-wide exhaustion). | Stderr matches the per-provider rate-limit heuristic and the response is retry-after-able. |
| `quota_exhausted` | Provider rejected the request because the account window is fully consumed. | Per-provider window-exhaustion sentinel — distinct from transient `rate_limited`. |
| `error` | Generic non-recoverable provider error. | Exit non-zero with no more-specific marker. |

## Edge cases

- Stderr contains both a rate-limit hint AND a quota-exhausted marker —
  prefer the more-specific `quota_exhausted` classification (window-level
  beats request-level).
- Empty stdout with exit 0 — classify as `error` with a "missing result"
  reason; do NOT classify as `ok`.
- Stdout has a result marker but the marker payload is truncated mid-JSON —
  classify as `error`; the recognizer is JSON-aware.
- Provider emits OULIPOLY markers but the body is empty — classify as
  `error` with a "empty body" reason; do NOT default to `ok`.
- Provider exits with SIGTERM and partial output — classify as `aborted`
  if a cooperative-shutdown sentinel is present, otherwise `error`.

## Error conditions

The recognizer itself returns `Result<Signal, RecognizerError>`. Failure
classes:

- `UnknownProvider` — `provider` identity does not have a registered
  recognizer (programmer error; should never occur in shipped code).
- `MalformedTerminalEnvelope` — provider emitted a recognizable marker but
  the payload could not be parsed (handled as `error` signal carrying the
  envelope tail; recognizer does NOT panic on garbage input).

## Boundaries

- Recognizer does NOT decide retry policy — that is `balancer/mod.rs`'s
  job. It returns a classification only.
- Recognizer does NOT refresh quota — even on `quota_exhausted` the
  refresh path is owned by `quota/mod.rs` and is invoked by the balancer
  or by an explicit caller.
- Recognizer does NOT mutate process state — it is a pure function of the
  exited child's outputs.
- Recognizer does NOT classify across multiple invocations — each call is
  scoped to one process exit.

## Declared test patterns

Per `~/ai/conventions/testing.md`: exhaustive marker-fixture tests per
provider, per-signal boundary tests, and stress tests on partial-output.

- `crates/oulipoly-runtime/tests/age34_runtime_executor_service_routing.rs`
- `crates/oulipoly-runtime/tests/age153_balancer_signal_isolation.rs`
- `crates/oulipoly-runtime/tests/executor_return_channel.rs`
- `src-tauri/tests/age153_one_shot_terminal_signal.rs`
- `src-tauri/tests/age153_repl_terminal_signal.rs`
- `src-tauri/tests/age153_resume_terminal_signal.rs`
- `src-tauri/tests/age153_result_envelope_compat.rs`
- `src-tauri/tests/age153_terminal_signal_marker.rs`
- `src-tauri/tests/age153_captured_child_supervision.rs`
- `src-tauri/tests/age154_marker_compatibility.rs`
- `src-tauri/tests/age154_age140_carried_regression.rs`
- `src-tauri/tests/age162_dispatch_stderr_marks_exhausted.rs`
- `src-tauri/tests/pipeline_status_propagation_rca/recognizer_tightening_tests.rs`

## Cross-references

- `planning/coverage/spec-balancer.md` — the consumer of these signals.
- `planning/coverage/spec-executor.md` — owner of the process model that
  emits the inputs to the recognizer.
- `planning/coverage/spec-quota.md` — `quota_exhausted` signal often
  prompts a quota refresh by the caller.
- `AGENTS.md` § Testing.
