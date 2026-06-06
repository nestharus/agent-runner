# S10/S11 Provider Move-Out Scope

Status: scope draft for the atomic host cutover and deletion slices.

Sources checked:

- `planning/provider-extraction-resume/DECISIONS.md`
- `planning/provider-extraction-wu2-wu3/DECISIONS.md`
- `planning/provider-extraction-wu2-wu3/output/slice-sequence-v3.md`
- host worktree `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- external provider repos `/home/nes/projects/agent-runner-claude/trunk` and `/home/nes/projects/agent-runner-opencode/trunk`

## Executive Summary

1. S10 should move Claude Code first because the external `agent-runner-claude` CLI is self-contained.
2. S11 should move the OpenCode/Codex hybrid second because launch/session and quota are split across tools.
3. The host already has the generic external-provider dispatch seam, keyed by root model `provider = { ... }`.
4. The required config change is to add root model provider refs that point at `agent-runner-claude` and `agent-runner-opencode`.
5. Do not add Rust dependencies, Cargo workspace members, or manifest refs to the provider repos.
6. `contract/v1` is byte-identical between the host and both external provider repos at this scope pass.
7. Both external repos were clean when checked, so host work can treat provider artifacts as ready inputs.
8. The first host blocker is launch session capture: the external launch mapper currently drops `LaunchExit.session`.
9. OpenCode emits `session.provider_session_id` in the final launch exit; the host must persist it before deleting in-tree capture.
10. Claude currently does not emit a launch-exit session payload; S10 must either add that in the provider or call `session.capture`.
11. S10/S11 must be atomic cutovers: route live behavior through the external CLI and delete matching in-tree behavior in the same slice.
12. Provider-specific tests, docs, fixtures, examples, comments, and migration literals must move to provider repos or become neutral fake-provider tests.
13. S11 must delete the remaining Claude/Codex/OpenCode source anchors, not just runtime branches.
14. The live acceptance gate is dispatch -> capture -> notify/wake with the real external provider binaries.
15. Final acceptance still requires green Rust/frontend gates and the later whole-repo zero-grep cleanup.

## Scope Boundary

In scope:

- Host wiring needed to route existing Claude and OpenCode/Codex model execution through external provider CLIs.
- Model/config changes that activate `model.provider` based external dispatch.
- Deletion or neutralization of in-tree provider-specific behavior once the external provider owns it.
- Live E2E coverage proving dispatch, session capture, quota, and wake behavior with the external binaries.
- S11 deletion anchors for source, tests, examples, docs, and coverage specs.

Out of scope for this file:

- Rebuilding the external provider CLIs.
- S12 central config retirement beyond identifying fields that should not survive S11/S12.
- S13/S14 whole-repo neutralization details except where they affect S10/S11 acceptance.

## Current Wiring Map

| Stage | Current host anchor | What it does now | S10/S11 cutover requirement |
| --- | --- | --- | --- |
| Model root provider ref | `crates/oulipoly-config/src/model.rs:508-517`, `:582-594`, `:762-771`, `:1287-1305` | Parses and renders root `provider: Option<ProviderImplementationRef>`. | Add root `provider = { binary|path|script = ... }` to live/migrated model configs for extracted providers. |
| Provider ref shape | `crates/oulipoly-config/src/provider_implementation_ref.rs:5-16`, `:40-67` | Allows exactly one of `path`, `crate`, `binary`, or `script`; `crate` remains runtime-disabled. | Use `binary` for installed artifacts, `path` for local live gates, or `script` for wrapper scripts; do not use `crate`. |
| Runtime artifact conversion | `crates/oulipoly-runtime/src/provider_registry/conversion.rs:33-62` | Converts `path`, `binary`, and `script` to enabled runtime artifacts; disables `crate`. | No new registry type is needed. S10/S11 should use this existing converter. |
| Registry inventory | `crates/oulipoly-runtime/src/provider_registry/mod.rs:61-80`, `:202-230` | Builds model-name to artifact-key mapping from model configs with root provider refs. | Ensure the Tauri/runtime service construction includes the updated model configs before execution/quota/session services run. |
| Describe/cache lookup | `crates/oulipoly-runtime/src/provider_registry/mod.rs:116-151`, `:165-183` | Resolves enabled artifact, runs provider `describe`, and caches the current-process description. | External providers must advertise the capabilities needed by each path before host calls them. |
| Executor dispatch gate | `crates/oulipoly-runtime/src/executor/mod.rs:152-164`, `:306-314` | Routes to `external_provider::dispatch` only when `model.provider.is_some()`, otherwise legacy CLI execution. | S10/S11 cutover starts by making the affected model configs have `model.provider.is_some()`. |
| External launch orchestration | `crates/oulipoly-runtime/src/executor/external_provider/dispatch.rs:31-62` | Runs provider capability gate, `policy.evaluate`, `launch`, optional terminal classification, then maps to `ExecutionResult`. | Must preserve all legacy `ExecutionResult` semantics before deleting legacy provider behavior. |
| Launch request builder | `crates/oulipoly-runtime/src/executor/external_provider/request_builder.rs:24-90`, `:160-173` | Builds `PolicyEvaluateRequest` and `LaunchRequest`, including `known_provider_session_id` on resume. | Verify the provider receives the same effective prompt, argv, stdin, cwd, env, settings id, and known session id as legacy. |
| Launch JSONL client | `crates/oulipoly-provider/src/client.rs:301-347`, `crates/oulipoly-provider/src/stream.rs:51-64`, `:106-205` | Validates launch request, runs provider `launch`, parses JSONL events, and exposes `LaunchExit.session`. | Host must consume `LaunchExit.session`; current mapper does not. |
| Current launch result mapper | `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs:8-41` | Copies stdout/stderr/exit/terminal signal but sets `session_capture` to `None`. | S10 blocker: map `result.exit.session.provider_session_id` into `SessionCaptureResult` or call external `session.capture`. |
| Completed attempt ingestion | `src-tauri/src/run/balancing/finalization.rs:261-281` | Emits known session id from ingestion fallback or `input.result.session_capture.session_id`. | External launch must populate `result.session_capture.session_id` early enough for sidecar ownership and wake routing. |
| Quota external path | `crates/oulipoly-runtime/src/quota/external_provider/source_probe_orchestration.rs:29-52`, `:98-155` | Runs `quota.source`, `quota.probe`, and optional `quota.refresh_auth` through provider CLI. | S10/S11 should prefer this for extracted providers and delete adapter-derived local quota sources. |
| Legacy quota derivation | `crates/oulipoly-runtime/src/quota/adapter_derived_source.rs:1-120`, `quota/source.rs:11-68`, `quota/refresh.rs:97-168` | Derives `anthropic-usage` and `chatgpt-usage` scripts from Claude/Codex storage adapters. | Delete for extracted providers once external quota probe is live. |
| Setup brain | `crates/oulipoly-setup/src/agent.rs:93-126`, `:149-158`, `:266-289` | Hardcodes `claude`, `claude-sonnet-4-6`, and Claude stderr session parsing. | Route setup brain through the provider contract before deleting this path. |
| Setup detection/sync | `crates/oulipoly-setup/src/detection.rs:214-218`, `:334-376`, `:379-439`; `sync.rs:28-40` | Hardcodes known CLI names, auth files, profiles, and config dirs. | Provider-owned setup/discovery/settings must replace these branches for moved providers. |

## Registry And Config Change

The registry is already driven by `ModelConfig.provider`; S10/S11 should not create a parallel registry mechanism. The cutover changes are model/config data changes plus any migration plumbing needed to place those fields in user configs.

Claude live/migrated model shape:

```toml
provider = { binary = "agent-runner-claude" }

[[providers]]
name = "claude"
args = ["--model", "sonnet"]
```

OpenCode/Codex hybrid live/migrated model shape:

```toml
provider = { binary = "agent-runner-opencode" }

[[providers]]
name = "opencode1"
args = ["--variant", "high"]
```

Local live gates may use absolute `path` refs instead of `binary`:

```toml
provider = { path = "/home/nes/projects/agent-runner-claude/trunk/target/debug/agent-runner-claude" }
```

Rules:

- Keep provider repos out of host manifests; external providers remain process artifacts, not Rust dependencies.
- Prefer `binary` for released/user-installed artifacts and `path` for local live E2E gates.
- Keep `[[providers]].name` as the account/settings identity passed to the external provider as `settings_id`.
- Move provider-specific example TOML files such as `examples/models/claude-resume.toml` and `examples/models/codex-resume.toml` into provider repos or replace them with neutral fixtures before the final zero-grep gate.
- After adding root provider refs, the dispatch gate at `crates/oulipoly-runtime/src/executor/mod.rs:157-160` must route those models through `external_provider::dispatch` without any legacy fallback for the moved provider.

## Atomic Cutover Order

### Precondition For Both Slices

1. Build the external provider binary from a clean repo and record the exact commit SHA in the slice notes.
2. Verify `diff -qr contract/v1 <provider-repo>/contract/v1` is empty.
3. Run provider conformance directly: `describe`, `schema`, `policy.evaluate`, `launch`, `terminal.classify`, `quota.source`, `quota.probe`, `session.capture`, `session.read_turns`, `session.export`, `settings.*`, and setup/discovery subcommands used by the host.
4. Patch host launch session capture before deleting legacy behavior. `LaunchExit.session` exists in the contract, and OpenCode already writes `provider_session_id`; `launch_result_mapper.rs` currently drops it.
5. Add neutral fake-provider tests for the host mapper and lifecycle seam before provider-specific tests are deleted.

### S10: Claude Code First

1. Update local/test model config to include `provider = { binary = "agent-runner-claude" }` or an absolute `path` for the built binary.
2. Ensure `agent-runner-claude` returns enough launch session evidence for the host to emit `known_session_id`, either by adding `session.provider_session_id` to the final launch exit or by having host call external `session.capture` after launch.
3. Run a live dispatch through the host and prove execution used `external_provider::dispatch`.
4. Prove session capture: the invocation row must have a provider session id and a non-`none` capture method.
5. Prove resume: `known_provider_session_id` must reach provider launch via `LaunchParams.session` and resume the same provider session.
6. Prove quota through external `quota.source` and `quota.probe`; do not use `anthropic-usage` derivation from in-tree adapters for the cutover model.
7. Move Claude-specific parity tests/docs/fixtures into `agent-runner-claude` or convert host tests to neutral fake-provider coverage.
8. Delete Claude-specific host source surfaces in the same slice; do not leave a provider-specific fallback branch.
9. Run scoped grep over host code/tests touched by S10 and record any remaining Claude references as either deleted now or explicitly owned by S11/S13 if they are docs/planning only.

### S11: OpenCode/Codex Hybrid Second

1. Update local/test model config to include `provider = { binary = "agent-runner-opencode" }` or an absolute `path` for the built binary.
2. Keep the hybrid split intact: OpenCode owns launch/session/policy/terminal; Codex-owned auth files provide usage/quota attribution inside the provider CLI.
3. Verify the settings id/account map in `agent-runner-opencode/src/account.rs` is the provider-owned source of truth, not host config branches.
4. Run live host dispatch for each relevant account wrapper (`opencode1` through `opencode5` if available in the environment).
5. Prove OpenCode launch session capture from final `session.provider_session_id` is persisted by host lifecycle code.
6. Prove idle notify and mid-turn notify wake paths with the external provider binary, using the behavior pinned by `src-tauri/tests/wu_d_proactive_wake_integration.rs:591-718` as the acceptance shape.
7. Prove quota through provider `quota.source`/`quota.probe`, including the Codex auth-path mapping, with no host `codex-cwd`/`chatgpt-usage` derivation.
8. Move OpenCode/Codex parity tests/docs/fixtures to `agent-runner-opencode` or convert host tests to neutral fake-provider coverage.
9. Delete all remaining Claude/Codex/OpenCode host source surfaces listed below, then run the zero-grep inventory that feeds S13/S14.

## S11 Deletion Surfaces And Grep Anchors

Run these anchors before and after S11. The post-S11 expectation is no provider-specific source behavior in the host; any remaining literal should be in a planned S13 docs/planning neutralization list, not live runtime code.

| Surface | Grep anchor | Current examples | S11 action |
| --- | --- | --- | --- |
| Executor recognizer re-exports | `git grep -n "ClaudeRecognizer\|CodexRecognizer\|OpenCodeRecognizer" -- crates/oulipoly-runtime/src` | `executor/mod.rs:34-37`, `:583-590` | Delete provider recognizer exports/tests or replace with neutral fake provider. |
| Provider recognizer files | `git grep -n -iE "claude|codex|opencode" -- crates/oulipoly-runtime/src/executor/providers` | `providers/claude.rs`, `providers/codex.rs`, `providers/opencode.rs` | Move behavior/tests to provider repos; keep only neutral/open generic recognizers if still needed. |
| Provider-specific policy appenders | `git grep -n "append_claude_provider_policy\|append_codex_provider_policy" -- crates/oulipoly-runtime/src` | `provider_specific/policy/*.rs`, `executor/cli/policy/orchestration.rs:22-44` | Delete in-tree argv/prompt policy formatting; external `policy.evaluate` owns it. |
| Resume missing-session phrases | `git grep -n "output_reports_missing_session\|no conversation found\|no session found" -- crates/oulipoly-runtime/src` | `provider_specific/resume_acceptance.rs:5-33`, `executor/cli/resume/acceptance.rs:23-39` | Move phrase vocabulary to provider repos or neutral contract errors. |
| Session capture scrub | `git grep -n "remove_unsanctioned_money_fields" -- crates/oulipoly-runtime/src` | `provider_specific/session_capture/telemetry_scrub.rs`, `executor/cli/result.rs:32-85` | Delete host scrub if provider-owned stream/capture owns provider telemetry shaping. |
| Built-in transcript locators | `git grep -n "ClaudeStorageLocator\|CodexStorageLocator\|LocatorSource::Claude\|LocatorSource::Codex" -- crates/oulipoly-runtime/src/session_metadata` | `locator.rs:29-35`, `locator/claude.rs`, `locator/codex.rs`, `transcript.rs:102-110` | Route through external `session.locate_transcript`; delete private storage scans and provider-specific errors. |
| Provider storage enum branches | `git grep -n "SessionStorage::ClaudeCode\|SessionStorage::Codex\|claude_code\|codex_session" -- crates/oulipoly-config/src crates/oulipoly-runtime/src` | `model.rs:318-425`, `providers.rs:377-397`, session metadata tests | Retire provider-owned storage branches or migrate them to script/generic/provider settings. |
| Tool restriction schema | `git grep -n "ClaudeRestrictions\|CodexRestrictions\|ToolRestrictionKind::Claude\|ToolRestrictionKind::Codex\|claude_tool_filter" -- crates/oulipoly-config/src` | `model.rs:104-153`, `providers.rs:487-549`, `claude_tool_filter.rs`, `lib.rs:3-15` | Move provider policy schema/validation into provider settings + `policy.evaluate`. |
| Codex model arg overlap | `git grep -n "CodexArgPart\|validate_codex_model_arg_overlap\|Codex flags" -- crates/oulipoly-config/src/model.rs` | `model.rs:1337-1467` | Delete or neutralize; provider-owned policy/model validation should enforce it. |
| Adapter-derived quota | `git grep -n "anthropic-usage\|chatgpt-usage\|claude-code-cwd\|codex-cwd" -- crates/oulipoly-runtime/src/quota` | `adapter_derived_source.rs:41-63`, tests `:95-119` | Delete after external quota probe replaces script derivation. |
| Setup brain hardcode | `git grep -n "claude-sonnet\|Command::new(\"claude\")\|Claude CLI" -- crates/oulipoly-setup/src` | `agent.rs:93-126`, `:149-158`, `:266-289` | Route setup brain through provider contract, then delete hardcoded Claude invocation/session parser. |
| Setup detection/profile branches | `git grep -n -iE "claude|codex|opencode" -- crates/oulipoly-setup/src` | `detection.rs:214-218`, `:334-376`, `:379-439`, `sync.rs:28-40`, `context.rs`, `schemas.rs` | External provider discovery/setup/settings own this; host keeps neutral setup surfaces. |
| Tauri integration fixtures | `git grep -n -iE "claude|codex|opencode" -- src-tauri/tests` | `wu_d_proactive_wake_integration.rs:591-718`, `initiative_05_migration.rs`, `age33_config_state_characterization.rs`, `pr_f_resume_integration.rs` | Move parity fixtures to provider repos or rewrite with neutral fake providers plus one opt-in live gate. |
| Examples and docs | `git grep -n -iE "claude|codex|opencode" -- examples docs README.md conventions planning/coverage` | `examples/models/claude-resume.toml`, `examples/models/codex-resume.toml`, provider convention docs | Move provider-specific docs/examples to provider repos or rewrite as neutral contract docs before S13/S14. |
| Whole host inventory | `git grep -n -iE "claude|codex|opencode" -- .` | Current count is large and includes tests/docs/planning | S11 should leave no live host behavior anchors; S13/S14 cleans remaining docs/planning/manifests/fixtures. |

## Live E2E Strategy

Live E2E is not a broad UI smoke test. It is a targeted proof that real external binaries preserve the lifecycle that legacy in-tree provider code used to own.

Required environment:

- Fresh temp `OULIPOLY_CONFIG_DIR`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, and state DB.
- `PATH` or absolute model `provider.path` pointing at freshly built `agent-runner-claude` and `agent-runner-opencode` binaries.
- Real underlying CLI wrappers/accounts available for opt-in live tests, or a documented skip when credentials are absent.
- Contract schemas checked against both provider repos before execution.

Provider-level preflight:

- Invoke `<provider> describe` and assert required capabilities are true.
- Invoke `<provider> schema` for settings schema id used by `describe`.
- Invoke `<provider> policy.evaluate` with representative launch params.
- Invoke `<provider> launch` and assert valid JSONL ending in one `exit` event.
- Invoke `<provider> terminal.classify` for nonzero/rate/quota examples.
- Invoke quota/session/setup commands used by the host path.

Host-level E2E cases:

1. Dispatch: run a model with root `provider = { path = ... }` and assert `external_provider::dispatch` is the execution path by checking provider `describe`/`launch` diagnostics or test spy output.
2. Capture: assert `LaunchExit.session.provider_session_id` or `session.capture` produces `ExecutionResult.session_capture.session_id`, then `finalization.rs:261-281` emits the known session id into state.
3. Resume: seed or capture a provider session id, run resume, and assert `LaunchParams.session.known_provider_session_id` is sent to the external provider.
4. Quota: route refresh through `quota.source`/`quota.probe`; assert no adapter-derived `anthropic-usage` or `chatgpt-usage` script is used for the cutover model.
5. Notify/wake idle: reproduce the acceptance shape of `opencode_notify_idle_wakes_resume_with_ses_session` with the external provider path.
6. Notify/wake mid-turn: reproduce `opencode_mid_turn_notify_resolves_capture_time_sidecar_owner`, especially owner session id, `sidecar_session_id`, busy wake status, delivered mailbox row, matched PID, and idle final runtime.
7. Terminal behavior: assert clean exit, nonzero exit, signal/cancellation, rate limit, and quota outcomes map to the same host `TerminalSignal` semantics.
8. Missing provider UX: configure a bad `binary` or `path` and assert the error is generic/actionable without provider-specific fallback.

Minimum commands for host verification after code changes:

```bash
cargo test --workspace
bunx tsc --noEmit
bun run test
```

Run `bun run test:e2e` before UI/setup-flow merge fallout, and keep real-provider live gates opt-in if credentials are required.

## Risks And Required Mitigations

| Risk | Evidence | Mitigation |
| --- | --- | --- |
| Host drops external launch session id | `launch_result_mapper.rs:31-34` hardcodes `SessionCaptureResult { session_id: None, method: None }`. | Fix mapper or call external `session.capture` before any deletion; add neutral mapper/lifecycle tests. |
| Claude external launch lacks final session payload | `agent-runner-claude/src/launch/events.rs:42-108` writes exit without `session`. | Add provider exit session payload or host follow-up `session.capture`; live S10 cannot pass without known-session persistence. |
| OpenCode hybrid quota attribution can drift | Account map lives in `agent-runner-opencode/src/account.rs:19-55`; launch/session use OpenCode while quota uses Codex auth files. | Treat provider repo account map as source of truth; host should pass opaque `settings_id` only. |
| Legacy fallback masks incomplete cutover | `executor/mod.rs:157-160` silently falls back to legacy when `model.provider` is absent. | For cutover models, require root provider ref and assert external dispatch occurred. |
| Provider-specific tests get deleted without replacement | Grep shows broad provider-specific Tauri/runtime tests. | Move parity tests to provider repos and keep neutral fake-provider host tests for contract/lifecycle seams. |
| Zero-grep can break docs/planning after behavior is moved | Decisions require final whole-repo grep, including docs/manifests/fixtures. | S10/S11 delete live behavior; S13/S14 neutralize remaining docs/planning/examples deliberately. |
| Data loss in session export/replace/rotation | S10/S11 touch transcript location, export, replace, rotation, and migration. | Use copy-on-write/temp fixtures, compare canonical transcripts, and run provider repo parity tests before host deletion. |
| Setup brain remains hardcoded | `crates/oulipoly-setup/src/agent.rs` still shells out to Claude directly. | Externalize setup brain before deleting provider-specific setup paths; no hardcoded brain bridge may remain. |
| Provider state concurrency | Contract requires stateless one-shot CLIs with provider-owned persisted settings. | Provider repo tests must prove file locks/atomic writes for settings and migration paths. |
| Artifact resolution differs in packaged app | S14 notes packaged Tauri PATH/executable permissions risk. | Test `binary`, `path`, and `script` resolution; ship generic missing-provider UX. |

## Acceptance Checklist

- `provider = { binary|path|script = ... }` activates external dispatch for the moved provider model.
- External provider `describe` advertises every capability the host path calls.
- External launch output maps stdout/stderr/exit/terminal signal and session capture into `ExecutionResult`.
- Completed attempt finalization emits the known provider session id from the external path.
- Resume sends `known_provider_session_id` to the external provider and resumes the captured session.
- Quota refresh for moved models uses external `quota.*` subcommands, not adapter-derived local scripts.
- Notify/wake idle and mid-turn flows pass with real external provider binaries or documented opt-in credentials.
- Provider-specific behavior/tests/docs/fixtures are moved to provider repos or neutralized in host tests.
- No provider-specific live host fallback remains for S10 after Claude move-out or for S11 after OpenCode/Codex move-out.
- `cargo test --workspace`, `bunx tsc --noEmit`, and `bun run test` pass after each slice.
