# S11 Gate Proposal

S11 completes the external-provider wake cutover. The delta makes detached wake resumes use the same selected provider settings, policy, launch environment, registry root, and captured provider session identity as the original external launch. It also stops treating a spawned wake as delivered until delivery is confirmed by provider-ingested turn evidence or by a signed submitted-turn marker, and the attempt-4 delta rotates external-provider transport/account-unavailable failures across the configured pool before terminal failure.

The implementation intentionally avoids a `state.db` schema change. New wake ownership and delivery-attempt state lives in the existing sidecar mailbox/pid-identity database, while state-db reads are limited to existing session-turn/body data. The audited functional cutoff is `95699d6..1593ef8`; the audited net source is `95699d6..549daaa`, including the reverted out-of-scope `opencode-turns` SQLite fallback history, the public-CLI executable-bit remediation, external-provider transport rotation and heartbeat-gap fixes, the committed function-classification helper splits, and the structural lint fixture exclusion for quoted gate patch artifacts.

Live evidence exists for external wake dispatch, confirmed delivery in one attempt, resumed notification output, S10 external smoke artifacts, and successful xhigh external smoke. Deterministic S10/S11/transport tests remain the binding proof; live `/tmp` artifacts are secondary evidence because they are not committed fixtures.

## Proof plan

Evidence log: `planning/s11-gate/evidence/runtime-tests.log`.

Runtime claim: External provider launch records the provider child process identity and captured provider session id so `agent-messenger notify` can resolve the sidecar owner and spawn a detached wake.

Proof method: `src-tauri/tests/s11_external_provider_wake.rs::external_provider_launch_notify_uses_captured_sidecar_owner_and_wakes` plus live `/tmp/s11-e2e/initial10.log` and `/tmp/s11-e2e/fresh10-notify2.stdout`.

Evidence-class match: full CLI/external-provider integration plus live smoke. The shipped test launches through the external provider fixture, captures sidecar identity, enqueues a notification, and asserts wake spawn. The live `/tmp/s11-e2e/initial10.log` run shows `DISPATCHED ab_19e9eb3c740_11963_58a0b4405542b714` and successful initial `OULIPOLY_RESULT`; paired live sidecar/export artifacts for that smoke family show confirmed delivery in one attempt and `WOKE 0` output.

Runtime claim: A headless wake is not marked delivered merely because resume ran; delivery is confirmed only when the resumed provider output yields a matching submitted-turn marker or exact ingested user turn evidence.

Proof method: `src-tauri/tests/s11_external_provider_wake.rs::external_provider_wake_does_not_mark_delivered_when_resume_produces_no_turn`, `::external_provider_wake_confirms_delivery_from_submitted_turn_marker`, `::external_provider_wake_ignores_submitted_turn_marker_for_different_payload`, `src-tauri/tests/wu_b_mailbox_integration.rs::resume_marks_delivered_from_exact_ingested_user_turn_without_assistant_delta`, and `::resume_rejects_different_ingested_user_turn_without_assistant_delta`.

Evidence-class match: integration tests exercise the exact resume/mailbox delivery path and state assertions, not a proxy. They cover the negative no-turn path, positive nonce/hash marker confirmation, wrong-payload rejection, and built-in exact-user-turn confirmation.

Runtime claim: Failed or rate-limited external wake attempts remain pending, record delivery-attempt/error evidence, release the wake claim, and can be retried.

Proof method: `src-tauri/tests/s11_external_provider_wake.rs::external_provider_failed_wake_releases_claim_and_retries_pending_mailbox`, `::external_provider_rate_limited_wake_records_error_and_retries_pending_mailbox`, and `crates/oulipoly-state/src/mailbox.rs::tests::mark_delivery_failed_records_attempt_without_delivery`.

Evidence-class match: full wake integration plus sidecar unit coverage. The tests assert `delivery_attempts` increments, `delivered_at` remains absent on failure, `delivery_error` records the failure class, claims are released, and the pending mailbox row is eligible for retry.

Runtime claim: External-provider transport timeout, launch heartbeat-gap timeout, and provider unavailable/timeout capability failures rotate to the next pool account, while schema/protocol/policy/capability-shape failures remain terminal.

Proof method: `crates/oulipoly-runtime/tests/age246_external_transport_rotation.rs::{external_transport_timeout_rotates_to_next_account_and_succeeds,external_launch_heartbeat_gap_timeout_rotates_to_next_account_and_succeeds,external_provider_unavailable_rotates_to_next_account_and_succeeds,external_transport_all_slow_pool_is_bounded_terminal_failure_with_honest_category}` plus the transport/heartbeat-gap TDD evidence recorded in `planning/s11-gate/evidence/source-commits.log`.

Evidence-class match: runtime integration fixture. The test suite creates multiple provider accounts, forces the selected account to time out, hit a launch heartbeat-gap timeout, or return an unavailable/auth-expired class, asserts dispatch retries the next account and succeeds, and asserts an all-slow pool attempts every account before returning a bounded `transport` / `host_timeout` terminal error.

Runtime claim: External-provider policy and launch requests carry the selected provider settings identity, hybrid launch shape, host linkage environment, and account-specific OpenCode auth while excluding ambient OpenAI/XDG leakage.

Proof method: `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs::external_provider_policy_evaluate_runs_before_launch_and_uses_selected_provider_settings`, `::external_provider_policy_request_passes_hybrid_launch_shape`, `::external_provider_launch_env_carries_host_linkage_without_openai_keys`, `::external_provider_launch_env_uses_selected_opencode_auth_context_without_openai_keys`, `::external_provider_launch_env_omits_ambient_xdg_for_default_opencode_auth_context`, and `::external_provider_policy_rejection_skips_launch`.

Evidence-class match: executor/provider-client integration fixtures capture the actual policy and launch protocol requests, environment maps, and rejection branch. This directly validates the request-shaping and environment boundary being claimed.

Runtime claim: Detached wake resume reloads the external provider registry from the launch-time model/config roots and can use ingested external sessions when the original launch capture is missing.

Proof method: `src-tauri/tests/s11_external_provider_wake.rs::external_provider_launch_notify_uses_captured_sidecar_owner_and_wakes` and `::external_provider_runtime_uses_ingested_session_when_launch_capture_missing`.

Evidence-class match: CLI/external-provider integration uses isolated model/config/data roots and proves the wake path supplies the correct models dir and session source. The fallback test exercises the runtime path where launch capture is absent but ingested external session metadata is available.

Runtime claim: S11 preserves S10 external provider launch/resume compatibility.

Proof method: `src-tauri/tests/s10_external_provider_resume.rs::external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd`, `::external_launch_session_id_alias_persists_external_capture_method_without_session_capability`, and live `/tmp/s10-e2e/final.log`, `/tmp/s10-e2e/final2.log`, plus any locally retained `/tmp` S10 smoke artifacts containing `S10-FINAL-OK` or `S10-RESUME-OK`.

Evidence-class match: deterministic CLI integration remains the binding resume proof and asserts external launch resume with recorded cwd and capture rows. Live smoke is secondary: currently retained `/tmp/s10-e2e/final.log` and `/tmp/s10-e2e/final2.log` contain `S10-EXTERNAL-OK` and successful result markers; if retained `S10-FINAL-OK` / `S10-RESUME-OK` logs are unavailable locally, this gate does not substitute them for the shipped resume tests.

Runtime claim: The external xhigh route still reaches the external provider path after S11.

Proof method: `/tmp/claude-1000/-home-nes-projects-agent-runner/45ccb26a-8bb6-4486-9b1e-2226e29292a0/tasks/bvnh4nja0.output`.

Evidence-class match: live runtime smoke. The retained task output records `gpt-xhigh flipped`, `=== smoke: gpt-xhigh through external ===`, `XHIGH-EXTERNAL-OK`, and `"status":"succeeded"`.

Runtime claim: Live S11 wake delivery reached the resumed provider and the sidecar recorded confirmed delivery in one attempt.

Proof method: `/tmp/s11-e2e/fresh10-opencode-argv.log`, `/tmp/s11-e2e/fresh10-xdg-data/oulipoly-agent-runner/pid-identity.db`, `/tmp/s11-e2e/fresh10-xdg-data/oulipoly-agent-runner/invocations/7a46a1a5-844d-45e8-bd67-aecaf9cf9194.result`, `/tmp/s11-e2e/fresh13-export.json`, and workload marker files under `/tmp/s11-e2e`.

Evidence-class match: live sidecar/runtime artifacts. The argv log shows the resumed prompt with `[OULIPOLY NOTIFICATIONS]`; the mailbox rows have `delivered_at` set, `delivered_by_invocation_uuid='7a46a1a5-844d-45e8-bd67-aecaf9cf9194'`, `delivery_attempts=1`, and `delivery_error=NULL`; the resumed invocation result is `success:true`; the exported session contains `WOKE 0`; workload marker files contain `S11-WAKE-OK`.

Runtime claim: S11 does not require a durable `state.db` schema migration.

Proof method: touched-file inventory plus `crates/oulipoly-state/src/schema.rs`, `crates/oulipoly-state/src/migrations.rs`, `crates/oulipoly-state/src/db.rs::tests::has_session_user_text_turn_requires_exact_user_body_match`, and `crates/oulipoly-state/src/mailbox.rs::tests::mark_delivery_failed_records_attempt_without_delivery`.

Evidence-class match: source invariant plus state/mailbox tests. The S11 touched list does not include schema or migration files; the current schema version remains unchanged, and the new behavior uses existing session-turn body reads plus existing sidecar mailbox columns.
