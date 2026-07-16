# spec-diagnostics — Runtime diagnostics, trace, services, ports

## Source files

- `crates/oulipoly-runtime/src/lib.rs`
- `crates/oulipoly-runtime/src/diagnostics/mod.rs`
- `crates/oulipoly-runtime/src/observability/invocation.rs`
- `crates/oulipoly-runtime/src/trace/mod.rs`
- `crates/oulipoly-runtime/src/services/adapters.rs`
- `crates/oulipoly-runtime/src/services/dtos.rs`
- `crates/oulipoly-runtime/src/services/error.rs`
- `crates/oulipoly-runtime/src/services/lock.rs`
- `crates/oulipoly-runtime/src/services/marker.rs`
- `crates/oulipoly-runtime/src/services/migration.rs`
- `crates/oulipoly-runtime/src/services/mod.rs`
- `crates/oulipoly-runtime/src/services/ports.rs`
- `crates/oulipoly-runtime/src/services/session_lifecycle.rs`
- `crates/oulipoly-runtime/src/services/session_warning.rs`
- `crates/oulipoly-runtime/src/services/session_window.rs`
- `crates/oulipoly-runtime/src/services/trace_failure.rs`
- `crates/oulipoly-runtime/src/ports/mod.rs`

## Preconditions

- A configured `StateDb` connection used as the diagnostics sink.
- The runtime caller has registered the relevant service adapters at
  startup (via `wiring.rs`); ports + adapters compose the runtime's
  outward-facing service interface.
- For trace operations: an in-flight or completed invocation whose
  inputs, outputs, and timings should be recorded.

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| Runtime emits a per-attempt diagnostics record. | `diagnostics/mod.rs` formats a `DiagnosticsEnvelope` carrying provider/account/signal/reason; persisted via the trace surface. |
| Service caller invokes a port through the adapter layer. | Adapter translates the DTO, calls the underlying repository/service, returns a typed result. |
| A typed service error is raised. | `services/error.rs` defines the variant; downstream caller pattern-matches. |
| Session warning is emitted (e.g. window approaching exhaustion). | `services/session_warning.rs` records a structured warning that does not abort the invocation. |
| Trace failure occurs (e.g. trace sink is unreachable). | `services/trace_failure.rs` emits a typed `TraceFailure` carrying the cause without surfacing through the user-facing return path. |
| Marker handling (OULIPOLY marker in stream). | `services/marker.rs` parses + buffers; downstream consumers (recognizer) read parsed markers, never raw bytes. |
| Heuristic category fallback sees generic auth wording. | `diagnostics/mod.rs` classifies only specific auth-expired sentinels such as `unauthorized` and `token expired`; generic `auth` text is left for more-specific classifiers or falls through rather than becoming `AuthExpired`. |
| A live-only observability snapshot reads chronological logical children after terminal history fills the invocation cap. | The invocation projection stably prioritizes durable `Running` candidates before terminal candidates, then applies the existing finite traversal cap and exact PID/boot/start-time liveness validation. Terminal-inclusive snapshots retain the chronological child order. |

## Edge cases

- Diagnostics write fails (state DB locked) — caller should not panic;
  the diagnostics module returns an error variant and the runtime
  continues without diagnostic side effects.
- Trace serialization encounters a non-UTF-8 byte sequence — fallback to
  base64 envelope per the typed schema.
- Service window query overlaps the boundary of a session migration —
  `services/session_window.rs` returns the union of pre- and
  post-migration windows.
- Unknown or ambiguous failure text that contains the substring `auth`
  but lacks a specific expired/unauthorized sentinel must not be upgraded
  to `AuthExpired`; this preserves unknown observability and avoids hiding
  provider failures behind an overbroad auth bucket.
- Durable `Running` status only changes candidate order in a live-only
  projection. Missing, dead, or mismatched process identity still fails
  closed, and unrelated invocations remain outside the logical subtree.

## Error conditions

- `DiagnosticsWriteFailed` — DB write into the diagnostics log failed.
- `ServiceAdapterError` — adapter could not translate DTO ↔ domain type.
- `TraceFailure` (typed) — trace emission failed; non-fatal.
- `PortNotRegistered` — caller invoked a port whose adapter is not
  registered (programmer error; should never ship).

## Boundaries

- Diagnostics does NOT decide whether to retry — it records; the
  balancer decides.
- Diagnostics does NOT create new user-facing categories for AGE-175
  unknown observability; `unknown` remains the settled category when no
  narrower classifier applies.
- Trace does NOT mutate session state — it is a side-channel sink.
- Services / ports layer is the contract surface for the runtime; it
  does NOT bypass `oulipoly-state` for persistence.
- Service adapters do NOT call provider executables — that is the
  executor's domain.
- Observability does NOT infer authority or liveness from PPID ancestry,
  bare PID, recency, cwd, or filenames.

## Declared test patterns

Per `~/ai/conventions/testing.md`: parity tests per service, contract
tests on the ports surface, fixture tests on the trace envelope schema.

- `crates/oulipoly-runtime/tests/age144_trace_output_schema_doc.rs`
- `crates/oulipoly-runtime/tests/age34_runtime_diagnostics_service_routing.rs`
- `crates/oulipoly-runtime/tests/age_149_typed_trace_failure_characterization.rs`
- `crates/oulipoly-runtime/tests/age37_trace_service_parity.rs`
- `crates/oulipoly-runtime/tests/ports_contract.rs`
- `crates/oulipoly-runtime/tests/observability_snapshot.rs`
- `crates/oulipoly-runtime/tests/service_traits_compile.rs`
- `src-tauri/tests/age27_diagnostics_effective_provider.rs`
- `src-tauri/tests/age_54_trace_row_preservation.rs`
- `src-tauri/tests/pr_b_trace_integration.rs`
- `src-tauri/tests/pipeline_status_propagation_rca/age158_characterization.rs`
- `src-tauri/tests/pipeline_status_propagation_rca/age158_rc1_characterization.rs`
- `src-tauri/tests/age175_failure_response_identity.rs`
- `src-tauri/tests/pipeline_status_propagation_rca/rc1_abnormal_termination_under_tail_pipeline.rs`
- `src-tauri/tests/initiative_09_internal_unification.rs`

## Cross-references

- `planning/coverage/spec-state-db.md` — diagnostics sink + service
  repository layer.
- `planning/coverage/spec-session-lifecycle.md` — session_lifecycle
  service is a top consumer.
- `planning/coverage/spec-executor.md` — executor emits diagnostics
  inputs.
- `planning/coverage/spec-result-envelope.md` — consumes settled
  `unknown` diagnostics for the AGE-175 structured stderr marker.
- `AGENTS.md` § Rust Workspace Structure.
