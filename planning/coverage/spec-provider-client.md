# spec-provider-client — Provider artifact client and launch stream

## Source files

- `crates/oulipoly-provider/src/client.rs`
- `crates/oulipoly-provider/src/error.rs`
- `crates/oulipoly-provider/src/process.rs`
- `crates/oulipoly-provider/src/resolver.rs`
- `crates/oulipoly-provider/src/stream.rs`
- `crates/oulipoly-provider/src/testkit.rs`

## Preconditions

- The caller has a neutral provider artifact reference and a contract request
  envelope for a known subcommand.
- The provider crate remains independent from runtime, config, state, Tauri,
  provider repositories, and provider-specific dependencies.
- Provider-client execution is not wired into production runtime dispatch in
  this work unit.

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| Executable path, binary from explicit search roots, or executable script. | Resolver returns deterministic argv shape `<artifact> <subcommand>`. |
| Runtime-disabled crate artifact. | Resolver returns a structured runtime-disabled error. |
| Valid non-launch success envelope on stdout. | Client returns the success envelope or typed result and preserves bounded stderr diagnostics. |
| Valid non-launch error envelope on stdout. | Client returns a provider capability error preserving category, code, message, retryable, details, request id, diagnostics, and process status. |
| Provider closes stdin early but emits a valid envelope. | Valid success or provider error envelope wins over the stdin transport failure. |
| Provider closes stdin early without a valid envelope. | Client returns `provider_closed_stdin_early`. |
| Host timeout or cancellation. | Process tree is terminated and diagnostics record bounded stdout/stderr and cleanup state. |
| Valid launch JSONL sequence ending in one final exit event. | Stream reader returns ordered decoded events, raw stdout/stderr bytes, and the authoritative launch exit. |
| Provider process exits nonzero after a valid final launch exit event. | Launch result succeeds and records provider nonzero as diagnostics only. |

## Edge cases

- Byte limits are applied by bytes, not characters, and preserve truncation
  metadata for NUL and high-bit payloads.
- Non-launch stdout must be exactly one JSON object; leading logs, multiple
  objects, trailing junk, invalid JSON, invalid UTF-8, and empty stdout are
  protocol errors.
- Launch stdout/stderr event payloads decode base64 to raw bytes without UTF-8
  coercion.
- Launch sequence numbers must be strict `1..n`; duplicate, skipped, and
  decreasing sequence numbers are rejected.
- A forced kill during launch with no final exit is host cancellation when the
  host cancellation path caused the kill.

## Error conditions

- `spawn_failed`, `host_timeout`, `host_cancelled`, `provider_process_nonzero`,
  `provider_process_nonzero_with_success`, and `provider_closed_stdin_early`
  identify host transport failures.
- `schema_invalid_request`, `schema_invalid_response`,
  `schema_invalid_error_response`, `mismatched_contract`,
  `mismatched_request_id`, and launch JSONL ordering/base64/finality failures
  identify host protocol failures.
- Provider-side `timeout` category remains a provider capability error and is
  not conflated with host deadline expiry.
- Missing, non-executable, unsafe, and runtime-disabled artifact references are
  resolver errors before process spawn.

## Boundaries

- Provider-client code does NOT parse stderr as contract data.
- Provider-client code does NOT add runtime/Tauri/setup/model execution call
  sites or switch existing output behavior.
- Resolver code does NOT scan ambient provider names or shell-wrap scripts by
  default.
- The provider crate does NOT depend on internal runtime/config/state crates or
  provider-specific crates.

## Declared test patterns

Per `~/ai/conventions/testing.md`: process-backed lifecycle tests, resolver
matrix tests, strict envelope protocol tests, launch JSONL protocol tests,
error-precedence tables, output-limit tests, neutral fake-provider testkit
tests, and public API shape tests.

- `crates/oulipoly-provider/tests/process_lifecycle.rs`
- `crates/oulipoly-provider/tests/error_mapping.rs`
- `crates/oulipoly-provider/tests/testkit_fake_provider.rs`
- `crates/oulipoly-provider/tests/resolver.rs`
- `crates/oulipoly-provider/tests/client_invoke.rs`
- `crates/oulipoly-provider/tests/client_error_precedence.rs`
- `crates/oulipoly-provider/tests/client_limits.rs`
- `crates/oulipoly-provider/tests/launch_stream.rs`
- `crates/oulipoly-provider/tests/launch_stream_protocol.rs`
- `crates/oulipoly-provider/tests/launch_stream_lifecycle.rs`
- `crates/oulipoly-provider/tests/provider_client_api.rs`

## Cross-references

- `AGENTS.md` § Provider Contract Crate (`oulipoly-provider`).
- `planning/coverage/spec-executor.md` — existing runtime CLI execution path
  remains separate and output-preserving in AGE-213.
