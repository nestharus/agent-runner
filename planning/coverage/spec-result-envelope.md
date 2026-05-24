# spec-result-envelope — Result markers, failure identity, pre-invocation failures

## Source files

- `crates/oulipoly-state/src/result_envelope.rs`
- `crates/oulipoly-state/src/db.rs`
- `src-tauri/src/main.rs`

## Preconditions

- A balanced CLI invocation has reached either a committed invocation row or
  a pre-invocation fast-fail stage before the row exists.
- For committed invocations: the stdout emitter and raw `.result`
  artifact writer both build `OULIPOLY_RESULT` payloads through the shared
  result-envelope DTO/builder in `oulipoly-state`.
- For failure identity: provider account, provider session, and chain
  identifiers are supplied from already-known execution/session state only.

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| Successful committed invocation. | stdout emits exactly one `OULIPOLY_RESULT=` payload with the frozen seven-key success ABI: `error_category`, `exit_code`, `finished_at`, `id`, `status`, `success`, `terminal_reason`; no failure identity fields are present. |
| Failed committed invocation. | stdout emits exactly one `OULIPOLY_RESULT=` payload with the seven common keys plus `agent_runner_invocation_id`, `provider_name`, `provider_session_id`, `agent_runner_chain_id`; raw `<uuid>.result` uses the same key set and identity nullability. |
| Failure identity is assembled for a committed invocation. | `agent_runner_invocation_id == id`; provider and session fields reflect best in-scope values; unavailable values are JSON `null`. |
| Provider session has an existing chain. | `agent_runner_chain_id` is populated from `StateDb::chain_id_for_segment(provider_name, provider_session_id)`. |
| Provider session has no existing chain. | `agent_runner_chain_id` is JSON `null`; result-envelope construction does not mint a chain solely for JSON identity. |
| Spawn/setup, zero-turn, typed terminal, returned-artifact, or generic non-quota failure path emits a result. | The failure result carries identity from the best branch-local source after session capture/update work that can affect row-backed identity. |
| Provider selection, provider resolution, or pool exhaustion fails before an invocation UUID exists. | stdout emits `OULIPOLY_FAILURE=` with `failure_kind: "pre_invocation"`, status/terminal fields for pre-invocation failure, all four identity fields as JSON `null`, and no forged `OULIPOLY_RESULT.id`. |
| Pre-invocation detail records attempted providers. | `provider_selection` reports `[]`; `provider_resolution` reports only the selected provider at `provider_index`; `pool_exhausted` reports the attempted pool. |
| Settled failure category is `unknown`. | stderr emits legacy `[diagnostics] unknown: ...` and an additional `OULIPOLY_UNKNOWN_DIAGNOSTIC=` payload after the final category is known, with redacted/truncated stderr excerpt and controlled retry disposition. |

## Edge cases

- `finished_at` may differ between stdout emission time and raw artifact
  finalize time; key set and failure identity nullability must stay in
  lockstep across the two producers.
- Start-known provider session identity must survive spawn/setup failure
  handling and appear in the failure result instead of being dropped.
- Returned-artifact persistence failure can override the settled category
  to `returned_artifacts`; `OULIPOLY_UNKNOWN_DIAGNOSTIC` must not be
  emitted from a stale pre-artifact `unknown` classification.
- Unknown diagnostic stderr excerpts are redacted before byte truncation,
  include at most four non-empty lines, and respect UTF-8 boundaries.
- Strict result recognizers accept success exact-seven and failure
  exact-eleven shapes only; unrelated extra keys remain rejected.

## Error conditions

- `ResultEnvelopeSerializationFailed` — marker JSON serialization failed;
  emit a warning without panicking.
- `ResultArtifactWriteFailed` — raw `.result` artifact write failed; emit
  an artifact warning while preserving invocation finalization semantics.
- `PreInvocationFailure` — provider selection, provider resolution, or
  pool exhaustion failed before invocation row creation; emit
  `OULIPOLY_FAILURE` rather than `OULIPOLY_RESULT`.
- `UnknownDiagnosticReadFailed` — quota/window state lookup for the
  unknown diagnostic failed; include the read error string in the
  diagnostic payload instead of suppressing the diagnostic.

## Boundaries

- Result-envelope construction does NOT classify terminal output or invent
  new error categories; it consumes the settled category from diagnostics
  and runtime paths.
- Result-envelope construction does NOT decide retry/rotation policy.
- Result-envelope construction does NOT mint provider-session chains only
  to populate failure JSON identity.
- Success ABI is frozen: success `OULIPOLY_RESULT` remains byte-compatible
  with the AGE-153 golden shape.
- `OULIPOLY_FAILURE` is outside terminal-result recovery; strict
  `OULIPOLY_RESULT` recognizers and raw `.result` recovery do not accept it.
- Prompt-resolution `OULIPOLY_FAILURE` handling is out of scope here and
  tracked by AGE-181.

## Declared test patterns

Per `~/ai/conventions/testing.md`: shared-builder shape tests,
stdout/raw artifact lockstep tests, every failure emit-site fixture,
pre-invocation marker fixtures, unknown diagnostic redaction/truncation,
and strict recognizer compatibility.

- `src-tauri/tests/age175_failure_response_identity.rs`
- `src-tauri/tests/age153_result_envelope_compat.rs`
- `src-tauri/tests/age153_support/mod.rs`
- `src-tauri/tests/age154_marker_compatibility.rs`
- `src-tauri/tests/age35_routing_lifecycle_characterization.rs`
- `src-tauri/tests/pipeline_status_propagation_rca/mod.rs`
- `src-tauri/tests/pipeline_status_propagation_rca/recognizer_tightening_tests.rs`

## Cross-references

- Deliberate union: `src-tauri/src/main.rs` is broadly owned by
  `planning/coverage/spec-tauri-client.md`; this spec owns only the
  AGE-175 result marker, failure identity, pre-invocation failure, and
  unknown-diagnostic behavior implemented there.
- Deliberate union: `crates/oulipoly-state/src/db.rs` is broadly owned by
  `planning/coverage/spec-state-db.md`; this spec owns only the raw
  `.result` artifact builder and failure-identity derivation behavior.
- `planning/coverage/spec-diagnostics.md` — supplies the settled
  `unknown` category consumed by `OULIPOLY_UNKNOWN_DIAGNOSTIC`.
- `planning/coverage/spec-recognizer.md` — strict marker recognition and
  `OULIPOLY_FAILURE` non-recognition.
- `AGENTS.md` § Rust Workspace Structure.
