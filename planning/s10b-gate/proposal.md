# s10b Gate Proposal

The S10B cutover closes host-side compatibility gaps exposed by external provider launch and resume. The delta makes binary provider references resolve from the production process `PATH`, relaxes the host `describe.concurrency` DTO to accept schema-valid free-form metadata, preserves inherited launch environment through policy transforms, sends provider-compatible policy/launch request shapes, records external launch session capture methods, maps provider settings errors from typed provider/client process status, and routes provider-ref headless resume back through the external launch path with the recorded runtime cwd. The final source-remediation commit also splits dense parser/mapper/validator/predicate/formatter helper responsibilities without changing behavior, so the implementation shape matches the code-quality contract declarations.

The implementation intentionally avoids a `state.db` schema change. External runtime cwd is stored in the existing mailbox sidecar metadata, and resume resolution reads that metadata before falling back to legacy storage cwd. Provider-ref resume skips legacy default migration/rotation unless the user explicitly requests migration, because the external provider itself owns the resume launch contract; the skip is bounded by provider-ref target validation that checks the resolved model, provider implementation reference, provider index, and selected provider identity before bypassing legacy resume/migration paths.

Live launch evidence exists for the installed external provider stack: `/tmp/s10-e2e/final.log` and `/tmp/s10-e2e/final2.log` both contain `S10-EXTERNAL-OK` and a successful final `OULIPOLY_RESULT`. No inspected `/tmp/s10-e2e` log contains `S10-RESUME-OK`; an isolated live launch+resume attempt after the post-commit gates produced no marker output and was terminated, so live resume is not claimed here. Resume behavior is instead bound to deterministic S10 integration evidence in `src-tauri/tests/s10_external_provider_resume.rs`.

## Proof plan

Evidence log: `planning/s10b-gate/evidence/runtime-tests.log`.

Runtime claim: Binary provider references resolve through supplied process `PATH` entries, and missing or unset PATH inputs preserve `missing_artifact` behavior without panicking.

Proof method: `crates/oulipoly-runtime/tests/provider_registry.rs::binary_ref_resolves_from_process_path_entries`, `crates/oulipoly-runtime/tests/provider_registry.rs::absent_binary_from_process_path_entries_preserves_missing_artifact`, and `crates/oulipoly-runtime/tests/provider_registry.rs::unset_process_path_entries_preserves_missing_artifact_without_panic`.

Evidence-class match: unit/integration registry fixtures; the tests materialize a temporary executable, pass resolver PATH entries, assert successful describe lookup, assert absent binaries remain `missing_artifact`, and assert unset PATH does not panic or resolve. Evidence log records these tests as covered by the isolated `cargo test --workspace` run.

Runtime claim: The S10B source guard covers every production registry construction site that must opt into process `PATH` binary resolution.

Proof method: `src-tauri/tests/age244_s7b_production_wiring_source_guard.rs::s10_production_provider_registries_populate_path_entries_from_process_path`.

Evidence-class match: source invariant; this claim is about construction-site coverage, so a source guard is the matching evidence class. Runtime behavior for PATH-derived binary resolution is covered separately by the provider registry resolver tests, and live external launch evidence shows the installed CLI path reached the external provider binary.

Runtime claim: External provider `describe.concurrency` accepts schema-valid free-form concurrency metadata instead of requiring legacy host-only fields.

Proof method: `crates/oulipoly-provider/tests/client_invoke.rs::invoke_describe_accepts_schema_valid_freeform_concurrency_metadata`.

Evidence-class match: provider-client subprocess fixture; the test returns a describe response with `launch_streams`, `process_model`, and `state_serialization`, then asserts the host accepts the response and preserves metadata. Evidence log records this test as covered by the isolated workspace run.

Runtime claim: Provider settings error reporting maps process status only from typed protocol surfaces, not by reparsing raw provider stdout.

Proof method: `crates/oulipoly-runtime/tests/provider_settings_host.rs::settings_host_preserves_conflict_error_details_and_diagnostics`.

Evidence-class match: provider-settings host integration fixture; the fake provider emits a typed `process_status` envelope and the host preserves conflict details, diagnostics, and exit status through `ProviderCapabilityError::provider_reported_process_status`. Evidence log records this targeted test and the isolated workspace run.

Runtime claim: External provider policy and launch requests preserve inherited environment, expose provider-compatible launch metadata, and keep model-local provider args on `params.model.provider_args` rather than duplicating them as base launch args.

Proof method: `crates/oulipoly-runtime/tests/s10_external_launch_session.rs::external_launch_exit_session_populates_capture_and_resume_request`.

Evidence-class match: external-provider executor integration fixture; the fake provider captures policy and launch requests, and assertions cover `PATH` preservation, policy launch shape, model provider args, launch env, captured session id, and known-session relaunch. Evidence log records this test as covered by the isolated workspace run.

Runtime claim: External LaunchExit session metadata persists a launch capture method and accepts both `provider_session_id` and `session_id` as the session id field.

Proof method: `crates/oulipoly-runtime/tests/s10_external_launch_session.rs::external_launch_exit_session_populates_capture_and_resume_request` and `src-tauri/tests/s10_external_provider_resume.rs::external_launch_session_id_alias_persists_external_capture_method_without_session_capability`.

Evidence-class match: integration; the executor test proves normal `provider_session_id` capture, while the S10B pre-fix regression proves a `session_id` alias is persisted with `session_capture_method='external_provider_launch'` and `provider_session_capture_method='external_provider_launch'` even when the external provider has no session capability. Evidence log records the RED/GREEN pre-fix and isolated workspace run.

Runtime claim: Provider-ref headless resume uses the external launch executor path with `known_provider_session_id`, not the legacy CLI resume path.

Proof method: `src-tauri/tests/s10_external_provider_resume.rs::external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd`.

Evidence-class match: full CLI integration; the fixture performs launch then resume against an external provider script, asserts the resume call is a `launch` request with `known_provider_session_id`, asserts no `rotation.assess`, `rotation.apply`, `settings.migrate`, or legacy resume provider calls occurred, and asserts final invocation rows preserve launch and resumed capture metadata. Evidence log records this test as covered by the isolated workspace run.

Runtime claim: Provider-ref headless resume bypasses legacy resume/migration only after validating the replacement provider-ref target invariant.

Proof method: `src-tauri/src/run/resume/orchestration.rs::{validate_headless_resume_target,validate_provider_ref_headless_resume_target,migrate_resume_target}` and `src-tauri/tests/s10_external_provider_resume.rs::external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd`.

Evidence-class match: source invariant plus full CLI integration; the source validation requires a resolved model, a root provider implementation reference, an in-range provider index, and selected-provider agreement between the resolved pool member and loaded provider before allowing the provider-ref branch. The integration fixture exercises the valid provider-ref path and proves the branch then uses external launch with the recorded cwd instead of legacy resume/migration.

Runtime claim: Provider-ref resume uses the original recorded launch cwd from `session_runtime.effective_cwd`, not the caller cwd.

Proof method: `src-tauri/tests/s10_external_provider_resume.rs::external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd`.

Evidence-class match: full CLI integration; the launch runs from a hostile cwd fixture but records the intended project cwd, then resume is invoked from the hostile cwd and asserts the external provider launch request uses the original project cwd. Evidence log records this test as covered by the isolated workspace run.

Runtime claim: Live installed external launch reached the external provider binary and returned the requested marker.

Proof method: `/tmp/s10-e2e/final.log` and `/tmp/s10-e2e/final2.log`.

Evidence-class match: live smoke; both logs contain provider result JSON with `result":"S10-EXTERNAL-OK"` and final `OULIPOLY_RESULT.success=true`. Both logs also include the known non-fatal `session.read_turns: missing_host_home` warning, which is outside the launch success claim. Evidence log records the inspected files and prior failing logs.
