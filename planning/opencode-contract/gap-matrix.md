# OpenCode Provider Contract Gap Matrix

Read-only audit date: 2026-06-04

Scope: audited the provider contract schemas in `contract/v1/*.schema.json`, the Rust trait boundary in `crates/oulipoly-provider`, the runtime/config consumers for launch, policy, quota, rotation, session capture, resume, terminal classification, session locate/export/replace, setup/discovery/migration, and the local config under `~/.config/oulipoly-agent-runner`. No real `agents` or `opencode` command was run against production DBs.

## Verified Inputs

Reference provider wiring:

| Provider family | Launch args | Prompt mode | Resume | Session capture | Session storage | Quota | Policy/tool restrictions |
|---|---|---|---|---|---|---|---|
| `claude*` | `-p --dangerously-skip-permissions --output-format json` | `stdin` | `kind = "flag", flag = "--resume"` | `forced_flag_verified`, `--session-id` | `claude_code` scripts: `claude-code-cwd`, `claude-code-locate-transcript`; `sessions.toml` has `claude-code-turns` | `anthropic-usage ~/.claude*/.credentials.json`, `auth_refresh_command = "claude auth status"` | `system_prompt_override`; `tool_restrictions.kind = "claude"`, `Task` disallowed |
| `codex*` | `exec --dangerously-bypass-approvals-and-sandbox` | `arg` | `kind = "subcommand", subcommand = ["resume"]` | not in `providers.toml`; session IDs come from Codex storage/ingest path | `codex_session` scripts: `codex-cwd`, `codex-locate-transcript`; `sessions.toml` has `codex-turns` | `chatgpt-usage ~/.codex*/auth.json`, `auth_refresh_command = "/bin/false"` | `system_prompt_override`; `tool_restrictions.kind = "codex"` |
| `opencode*` | `run --dangerously-skip-permissions` plus model `-m openai/gpt-5.5 --variant <none|low|medium|high|xhigh>` | `arg` | `kind = "flag", flag = "--session"` | absent | `storage_type = "claude_code"`, `cwd_script = "/bin/false"`, `transcript_script = "/bin/false"`; no `sessions.toml` entries | `chatgpt-usage` against Codex auth files, with non-sequential mapping for accounts 2-5; `auth_refresh_command = "/bin/false"` | absent |

OpenCode model pool:

| Model TOML | Providers |
|---|---|
| `~/.config/oulipoly-agent-runner/models/gpt-none.toml` | `opencode`, `opencode2`, `opencode3`, `opencode4`, `opencode5` |
| `~/.config/oulipoly-agent-runner/models/gpt-low.toml` | `opencode`, `opencode2`, `opencode3`, `opencode4`, `opencode5` |
| `~/.config/oulipoly-agent-runner/models/gpt-medium.toml` | `opencode`, `opencode2`, `opencode3`, `opencode4`, `opencode5` |
| `~/.config/oulipoly-agent-runner/models/gpt-high.toml` | `opencode`, `opencode2`, `opencode3`, `opencode4`, `opencode5` |
| `~/.config/oulipoly-agent-runner/models/gpt-xhigh.toml` | `opencode`, `opencode2`, `opencode3`, `opencode4`, `opencode5` |

OpenCode JSON event shape from docs/source, not from a local production invocation:

| OpenCode `run --format json` event | Session ID field | Notes |
|---|---|---|
| `step_start` | top-level `sessionID`; duplicate at `part.sessionID` | Best capture source because it is the first documented event and carries `ses_...`. |
| `tool_use` | top-level `sessionID`; duplicate at `part.sessionID` | Emitted when a tool finishes. |
| `text` | top-level `sessionID`; duplicate at `part.sessionID` | Contains model output in `part.text`. |
| `step_finish` | top-level `sessionID`; duplicate at `part.sessionID` | Final step has `part.reason = "stop"`; tool continuation has `"tool-calls"`. |
| `error` | top-level `sessionID` | Error details appear under `error.name` and `error.data.message`; rate-limit examples include status code 429. |

Recommended capture selector: `event_type = "step_start"`, `event_id_path = "sessionID"`. The field is camelCase `sessionID`, not `session_id`. OpenCode session IDs are `ses_<opaque>` and are not UUIDs.

Risk already established and preserved here: a direct `opencode1 run --format json` probe hung at 120 seconds. Because the current runner parses stdout JSON events after process exit, deterministic capture depends on the opencode process exiting. Streaming early capture would reduce this risk.

## Code Wiring Notes

| Surface | Current code behavior |
|---|---|
| Provider config load | `ProviderEntry` owns `quota_script`, `auth_refresh_command`, `command`, `args`, `prompt_mode`, `resume`, `session_capture`, `resume_acceptance`, `session_storage`, `system_prompt_override`, `tool_restrictions`, and `invocation_mode` in `crates/oulipoly-config/src/providers.rs`. Effective providers merge base provider args with model TOML args. |
| Launch argv | Runtime builds command from `provider.command`, appends provider/model args, input args, capture args, tail args, then renders prompt. For opencode this currently forms `opencodeN run --dangerously-skip-permissions -m openai/gpt-5.5 --variant <x> <prompt>`. |
| `stdout_json_event` capture | Config schema supports either legacy `json_flag` or multi-token `json_args`, plus optional `last_message_flag`. Runtime can append `json_args = ["--format", "json"]` and capture `step_start.sessionID` without requiring a last-message sidecar. |
| Capture parsing | Parser already supports arbitrary event type plus dotted path. It can parse `step_start.sessionID` if argv/config can get JSONL onto stdout. |
| Resume argv | `kind = "flag", flag = "--session"` composes `--session <active_session_id>`. This part is wired. |
| Resume input validation | `src-tauri/src/run/resume/validator.rs` rejects blank input only. `StateDb::resolve_resume` is the real validation surface and resolves chain UUIDs or active provider session IDs such as `ses_...`, while preserving wrong-ID-kind rejection. |
| Wake | Wake records and respawns using the active provider session ID. For opencode that can be `ses_...`; the remediated resume validator lets DB resolution validate it instead of rejecting it as non-UUID. |
| Provider identity | Runtime recognizes Claude, Codex, OpenCode, and fallback `openai_compat`. OpenCode is selected by provider name or command executable prefix. |
| Terminal classification | OpenCode has a provider-specific recognizer for structured JSON `error` events. It maps 429/rate-limit evidence to `RateLimited` and persistent quota wording to `QuotaExhaustedInband`, while preserving generic terminal-status handling. |
| Quota | Explicit `quota_script` is enough for routing refresh. Adapter-derived quota only recognizes Claude/Codex storage scripts. |
| Session locate/export/replace | Script storage locators are generic, but opencode points at `/bin/false` and declares `claude_code` canonical format. Export/replace only support `claude_code` and `codex_session` canonical parsing/rendering, or external-provider session dispatch. |
| Session read/turn scan | A bundled `scripts/opencode-turns` adapter now maps public `opencode export <sessionID>` output into normalized JSONL. Deployment still needs `sessions.toml` entries for each opencode account. |

## Gap Matrix

| Capability | Claude/Codex reference wiring | OpenCode current state | Gap | Concrete fix | Concrete test |
|---|---|---|---|---|---|
| Common envelope, `describe`, `schema` | Contract schemas and generated Rust types exist in `crates/oulipoly-provider`; built-in Claude/Codex are not external provider binaries and do not advertise `describe` at runtime. | Missing as an external provider; same built-in runtime pattern as Claude/Codex, with no opencode provider implementation artifact. | If opencode is expected to satisfy the external provider contract directly, there is no `describe`/`schema` implementation advertising its capabilities. | Either declare opencode as built-in/config-driven only, or implement an external opencode provider binary/crate that serves `describe` and `schema` and advertises only supported capabilities. | New contract test: `cargo test -p oulipoly-provider opencode_describe_schema_contract -- --nocapture`. Flow: invoke fake opencode provider `describe`, validate against `contract/v1/describe.schema.json`, then request its settings schema by `schema_id`. |
| Launch | Claude and Codex launch through `providers.toml` command/args plus model-provider args; prompt transport is `stdin` for Claude and `arg` for Codex. | Wired for basic launch. `opencode*` commands and args exist; `gpt-*` models reference all five accounts. `stdout_json_event` can now append `json_args = ["--format", "json"]`. | Direct live `--format json` probe hung at 120s, so deterministic completion remains a live OpenCode sandbox risk even though runner argv/capture support is present. | Keep base launch as-is. Configure opencode capture with multi-token JSON args and optional sidecar, then verify live OpenCode completion in an isolated sandbox. Consider streaming early session ID capture. | Runtime fake-provider test: `cargo test -p oulipoly-runtime --test age_164_c5_resume_capture opencode_launch_argv_uses_format_json_and_captures_session -- --nocapture`. Flow: fake `opencode1` asserts argv contains `run --dangerously-skip-permissions -m openai/gpt-5.5 --variant high --format json <prompt>`, emits `{"type":"step_start","sessionID":"ses_fixture"}` and exits 0. |
| Policy | Claude injects `--append-system-prompt` and tool filters; Codex prepends policy text into the prompt and can append Codex config/disable flags. | Missing. No `system_prompt_override` or `tool_restrictions` on opencode accounts. Runtime has only `ToolRestrictionKind::{Claude,Codex}` and OpenCode is `openai_compat`. | OpenCode cannot receive the child-agent safety policy or tool restrictions through current policy machinery. This is a safety/functionality gap versus Claude/Codex. | Add opencode policy support if OpenCode needs equivalent restrictions. Candidate mechanisms: `OPENCODE_PERMISSION` env, a dedicated generated agent via `--agent`, or a prompt-prefix policy. Extend config with `tool_restrictions.kind = "opencode"` rather than misusing Claude/Codex. | New policy dispatch test: `cargo test -p oulipoly-runtime --test age217_s6a_policy_launch_dispatch opencode_policy_injects_permissions_or_prompt -- --nocapture`. Flow: fake provider config with opencode policy; assert launch plan receives the chosen env/args/prompt transform and rejects cross-kind config. |
| Quota source/probe/refresh_auth | Claude uses `anthropic-usage` plus `claude auth status`; Codex uses `chatgpt-usage` against each `~/.codex*/auth.json`. Runtime can refresh explicit `quota_script` and retry after `auth_refresh_command`. | Partial. All five opencode accounts have explicit `chatgpt-usage` scripts, but they point to Codex auth files. `auth_refresh_command` is `/bin/false`. | If opencode wrappers truly use Codex account auth files, quota may work. If they use native OpenCode auth at `~/.local/share/opencode/auth.json`, quota is pointed at the wrong source. No auth refresh. Non-sequential account mapping needs verification. | Verify account-to-auth-file mapping. Either keep and document Codex-backed wrappers, or write `opencode-usage` for native OpenCode auth. Replace `/bin/false` with a real refresh/status command if OpenCode can refresh OAuth non-interactively. | New quota test: `cargo test -p oulipoly-runtime --test age35_routing_characterization opencode_five_account_quota_scripts_route_healthy_accounts -- --nocapture`. Flow: temp `providers.toml` with five opencode accounts and fake quota scripts emitting window JSON; assert exhausted accounts are skipped and healthy account is selected. |
| Rotation, assess/materialize, load balancing | Built-in routing is generic over model provider pools; Claude/Codex account pools rotate based on quota windows, invocation counts, session scans, and terminal outcomes. External provider rotation contract exists separately. | Partial. Each `gpt-*` model has a five-account opencode pool, so generic selection can run. Quota and session scan inputs are incomplete. No opencode external `rotation.assess/materialize`. | Pool topology is present, but quality depends on quota correctness and session turns. No OpenCode-specific materialized transcript/session rotation. | No code change needed for basic pool selection if quota scripts are valid. Add opencode turn scripts and session storage to improve scoring/zero-turn behavior. Implement external `rotation.*` only if moving to provider contract binaries. | Routing matrix test: `cargo test -p oulipoly-runtime --test routing_matrix opencode_pool_five_accounts_quota_and_recency -- --nocapture`. Flow: in-memory state with five opencode providers, varied quota windows and last-used data; assert selected index and exhausted behavior. |
| `session.capture` | Claude uses `forced_flag_verified` with `--session-id`; Codex-style stdout JSON event support exists in runtime. | Remediated in code for OpenCode-style capture. Runtime parser can read `step_start.sessionID`; capture argv can express `--format json`; last-message sidecar is optional. | Deployment still needs `[opencode*.session_capture]` config and isolated live OpenCode confirmation. | Add opencode capture config using `kind = "stdout_json_event"`, `json_args = ["--format", "json"]`, `event_type = "step_start"`, and `event_id_path = "sessionID"`. | Capture parser unit test: `cargo test -p oulipoly-runtime --test age_164_c5_resume_capture opencode_stdout_json_event_step_start_session_id -- --nocapture`. Flow: feed JSONL containing `step_start` with `sessionID = "ses_494719016ffe85dkDMj0FPRbHK"`; assert `SessionCaptureMethod::StdoutJsonEvent` and stored provider session ID. |
| Resume | Claude resumes with `--resume <id>`; Codex resumes with `resume <id>`. Runtime supports flag and subcommand strategies. | Remediated for runner-owned OpenCode IDs. Config uses `kind = "flag", flag = "--session"`, runtime composes `--session <active_session_id>`, Tauri accepts non-empty resume input, and `StateDb::resolve_resume` resolves active provider session IDs such as `ses_...`. | No opencode `resume_acceptance` phrases are enabled yet because live missing-session wording is unverified. | Keep resume lookup by provider session ID strings. Add opencode resume acceptance checks only after OpenCode prints a verified recognizable session-not-found/mismatch error. | Resume composition test: `cargo test -p oulipoly-runtime --test age_164_c5_resume_capture opencode_resume_flag_composes_session -- --nocapture`. End-to-end Tauri test: `cargo test -p oulipoly-agent-runner --test pr_f_resume_integration opencode_resume_accepts_ses_provider_session_id -- --nocapture`. Flow: in-memory/temp DB binds active provider `opencode` to `ses_fixture`; fake provider asserts argv includes `--session ses_fixture`. |
| Async-bash wake path | Claude/Codex wake uses captured/stored active provider session IDs that are UUID-like or otherwise accepted by resume path. Mailbox runtime records the active session and detached wake spawns `agents resume --session-id <session_id>`. | Remediated for runner-owned fake-provider flow. Capture can return `ses_...`, wake can spawn resume with `ses_...`, and the public validator no longer rejects non-UUID input before DB resolution. | Live OpenCode deterministic capture still needs isolated sandbox confirmation because the direct probe previously hung. | Preserve non-UUID provider-session resolution and keep production proof isolated from real `agents`/`opencode` until live sandbox evidence is available. | Wake integration test: `cargo test -p oulipoly-agent-runner --test wu_d_proactive_wake_integration opencode_notify_idle_wakes_resume_with_ses_session -- --nocapture`. Flow: fake mailbox pending row for `ses_fixture`, fake opencode provider, detached wake command captured without spawning real agents, assert validator accepts and resume argv uses `--session ses_fixture`. |
| `session.locate_transcript` | Claude and Codex have working `cwd_script` and `transcript_script`; built-in and script locators can locate JSONL paths. | Stubbed. `opencode*.session_storage` declares script storage with `storage_type = "claude_code"`, but both scripts are `/bin/false`. | Locate always fails. `storage_type = "claude_code"` is likely wrong for OpenCode's native storage/export. | Implement `opencode-cwd` and `opencode-locate-transcript` scripts, or use external provider transcript locator. Use a correct canonical type only if OpenCode transcript is actually Claude/Codex-compatible; otherwise introduce `opencode_session` or external export/replace. | Locator test: `cargo test -p oulipoly-runtime --test age243_s7a_session_dispatch opencode_locate_transcript_script_contract -- --nocapture`. Flow: temp OpenCode data dir with an `info` record containing `"id":"ses_fixture"`; fake locator emits one absolute path; assert locate succeeds and missing/ambiguous cases are reported. |
| `session.read_turns` | Claude/Codex have `sessions.toml` turn scripts that emit normalized JSONL turns; balancer scans them. | Missing. `sessions.toml` has no `[opencode*]` entries. | No opencode turn ingestion, no direct-session turn counts, no `session.read_turns` parity, weaker routing/zero-turn classification. | Add `opencode-turns` scripts for all five accounts, or external provider `session.read_turns`. The script must emit normalized JSONL with `session_id`, `turn_id`, `timestamp`, `role`, and optional canonical `body`. | Turn script test: `cargo test -p oulipoly-runtime --test age243_s7a_session_dispatch opencode_read_turns_ingests_normalized_jsonl -- --nocapture`. Flow: fake `opencode-turns` emits one user and one assistant turn for `ses_fixture`; assert `StateDb::count_session_turns("opencode", "ses_fixture")` sees assistant count 1. |
| `session.export` | Claude/Codex export canonical JSONL from native transcript formats or DB fallback. Export metadata resolves storage type and transcript path. | Stubbed/incorrect. Since locator scripts are `/bin/false`, metadata resolution fails. If locator worked, `storage_type = "claude_code"` would make export parse OpenCode data as Claude Code, probably wrong. | No safe OpenCode export. Wrong canonical parser risks malformed or lossy export. | Define OpenCode canonical export mapping. Options: parse `opencode export <sessionID>` output into canonical records, parse native DB/files, or implement external provider `session.export`. Do not claim `claude_code` unless verified identical. | Export test: `cargo test -p oulipoly-runtime --test age244_s7b_export_replace_dispatch opencode_export_canonical_jsonl -- --nocapture`. Flow: fixture OpenCode export JSON/session store -> canonical records -> assert roles/content/timestamps/session ID. |
| `session.replace` | Claude/Codex import-replace can render canonical records back to supported native storage with locking/journaling. | Missing. No OpenCode renderer/import path. | Cannot replace or migrate OpenCode sessions through current built-ins. | Implement OpenCode renderer/importer or external provider `session.replace`. If unsupported, return explicit `unsupported-storage` rather than pretending `claude_code`. | Replace test: `cargo test -p oulipoly-runtime --test age244_s7b_export_replace_dispatch opencode_replace_round_trip_or_explicit_unsupported -- --nocapture`. Flow: canonical fixture -> OpenCode native store -> export again; or assert stable unsupported error until implemented. |
| Terminal classification | Claude/Codex recognizers map provider evidence to terminal signals; generic helpers map exit 0/nonzero/signal/spawn/prolonged-silence. | Remediated for runner-owned OpenCode JSON error fixtures. OpenCode has a recognizer for JSONL `error` events carrying 429/rate-limit or persistent quota evidence. | Live OpenCode error emission semantics still require isolated runtime confirmation. | Keep the OpenCode recognizer scoped to structured JSON `error` events and avoid generic substring matching outside that surface. | Terminal test: `cargo test -p oulipoly-runtime --test age242_terminal_classify_characterization opencode_json_error_429_is_rate_limited -- --nocapture`. Flow: evidence stdout has `{"type":"error","sessionID":"ses_fixture","error":{"data":{"message":"Rate limit exceeded","statusCode":429}}}` with nonzero exit; assert `TerminalSignalKind::RateLimited`. |
| Discovery models/accounts | Runtime has generic discovery with an OpenCode strategy `opencode models`; setup detection also recognizes OpenCode auth profiles from `~/.local/share/opencode/auth.json`. Claude/Codex have analogous setup/discovery. | Partial. OpenCode is recognized by setup/discovery, but current `gpt-*` model TOMLs are static and use wrapper commands `opencode1` through `opencode5`, not discovered accounts. | Discovery does not populate the five configured account wrappers or verify that each wrapper maps to the intended account/auth file. No external `discovery.models/accounts`. | Add an opencode account discovery bridge if wrappers are first-class accounts. Parse `opencode auth list` or auth JSON, and map discovered accounts to generated provider entries. Keep static TOMLs if discovery remains advisory. | Discovery test: `cargo test -p oulipoly-runtime discovery::tests::opencode_models_output_parses_provider_slash_models -- --nocapture` plus setup test `cargo test -p oulipoly-setup opencode_profiles_from_auth_json -- --nocapture`. Flow: fixture `auth.json` and fake `opencode models` output. |
| Settings | External contract defines settings CRUD/validate/migrate. App has relocated provider settings/accounts UI/source guards, not a per-provider OpenCode settings API. Claude/Codex are config-driven. | Missing as contract capability; config-driven provider entries exist. | No OpenCode provider settings validation/migration beyond TOML parse. | If adopting external provider contract, implement settings schema for OpenCode account roots, auth source, session storage root, capture mode, and policy mode. Otherwise document settings as host-owned `providers.toml`. | Settings contract test: `cargo test -p oulipoly-runtime --test provider_settings_host opencode_settings_validate_rejects_false_locators -- --nocapture`. Flow: validate an opencode settings object with `/bin/false` locators and missing capture; assert diagnostics. |
| Setup detect/install/sync | Setup detection knows `opencode`, checks `~/.opencode`, checks auth at `~/.local/share/opencode/auth.json`, enumerates auth providers, and sync paths include `.opencode/config.json` for MCP. | Partial. OpenCode setup detection exists, but not account pool generation for `opencode1..5`, session storage scripts, capture config, quota mapping, or migration from Codex auth. | Setup can find OpenCode but cannot produce a fully working provider-contract config for async wake/resume. | Extend setup plan/sync to generate or validate opencode account entries, capture config, quota source, session scripts, and policy restrictions. | Setup test: `cargo test -p oulipoly-setup opencode_setup_plan_generates_capture_resume_storage -- --nocapture`. Flow: temp home with OpenCode auth JSON; assert proposed provider entries include `resume`, `session_capture`, non-false storage scripts, and quota source. |
| Migration plan/apply | Contract has provider migration plan/apply. Runner also has DB/config migration surfaces and Claude/Codex session migration mechanics. | Missing for OpenCode provider data. | No plan to migrate OpenCode sessions or account configs into the provider contract shape. No import from `opencode export` into canonical session store. | Add migration only after OpenCode export/replace/storage format is defined. Minimum: config migration from stubbed storage to real scripts; later: session canonical migration/import. | Migration test: `cargo test -p oulipoly-runtime --test migration_service_parity opencode_config_migration_replaces_false_storage -- --nocapture`. Flow: fixture legacy opencode providers with `/bin/false`; migration plan reports required edits and apply produces valid capture/storage config in a temp config root. |

## Proposed OpenCode Config Shape After Code Support

This is now valid under the remediated `SessionCapture` schema because `json_args` exists and the last-message sidecar is optional.

```toml
[opencode.session_capture]
kind = "stdout_json_event"
json_args = ["--format", "json"]
event_type = "step_start"
event_id_path = "sessionID"
restore_stdout = "raw_jsonl"

[opencode.resume]
kind = "flag"
flag = "--session"

[opencode.session_storage]
kind = "script"
storage_type = "opencode_session"
cwd_script = "opencode-cwd ~/.local/share/opencode"
transcript_script = "opencode-locate-transcript ~/.local/share/opencode"
```

If introducing `opencode_session` is too much for the first pass, leave export/replace explicitly unsupported and still implement locate/read-turn scripts for wake/routing. Do not label OpenCode native storage as `claude_code` unless fixtures prove the JSONL format is identical.

## Prioritized Worklist

P0, required for async-bash wake end-to-end on OpenCode:

1. Add `stdout_json_event` capture support for opencode's argument shape: support multi-token JSON mode such as `--format json` and make last-message sidecar optional or replace it with a generic stdout restoration mode.
2. Configure all five opencode accounts with `session_capture` using `event_type = "step_start"` and `event_id_path = "sessionID"`.
3. Fix resume/wake identifier validation so `agents resume --session-id ses_...` works, or change wake/manual resume to pass chain UUID while preserving provider `ses_...` for the actual opencode `--session` argv.
4. Add fake-provider integration coverage for `session_capture -> DB active provider session -> resume argv --session ses_... -> wake respawn` without using production DBs or real opencode.
5. Treat the 120s direct-invocation hang as a release risk: deterministic capture should be tested with fake providers first, then with an isolated OpenCode sandbox and hard timeout. Consider streaming early session ID capture because OpenCode emits `sessionID` before final process exit.

P1, required for reliable routing and quota behavior:

1. Verify the opencode account-to-auth-file mapping. Current mapping is `opencode -> ~/.codex/auth.json`, `opencode2 -> ~/.codex5/auth.json`, `opencode3 -> ~/.codex2/auth.json`, `opencode4 -> ~/.codex3/auth.json`, `opencode5 -> ~/.codex4/auth.json`.
2. Decide whether quota should read Codex auth files or native OpenCode auth at `~/.local/share/opencode/auth.json`.
3. Add opencode turn scripts in `sessions.toml` so balancer/session read paths can count assistant turns.
4. Add OpenCode terminal classification for JSONL `error` events, especially 429/rate-limit.
5. Add `resume_acceptance` patterns if OpenCode emits deterministic session-missing or session-mismatch output.

P2, provider-contract completeness and maintainability:

1. Replace `/bin/false` storage scripts with real `opencode-cwd` and `opencode-locate-transcript` scripts.
2. Define OpenCode canonical export/import semantics or explicitly keep export/replace unsupported.
3. Add OpenCode policy support if the safety policy and tool restrictions must match Claude/Codex.
4. Extend setup/discovery to generate or validate complete opencode provider account entries.
5. Implement external provider `describe/schema/settings/setup/migration/rotation` only if OpenCode is moving from host-owned TOML wiring to a provider binary/crate.

## Code Changes Needed

Code changes were needed for the P0 path unless a brittle config-only workaround was accepted. The remediated implementation adds the P0 capture/resume surfaces described below.

Required code changes:

| Area | Why code is needed |
|---|---|
| `SessionCapture` config and launch args | Remediated `stdout_json_event` accepts multi-token `json_args` and optional sidecar, so OpenCode can use `--format json` without a documented last-message sidecar. |
| Resume/wake identifier validation | Remediated resume validation accepts non-empty input and relies on `StateDb::resolve_resume` to validate chain UUIDs or active provider session IDs such as `ses_...`. |

Likely code changes:

| Area | Why code may be needed |
|---|---|
| Terminal recognizer | Needed if OpenCode quota/rate-limit should drive typed retry/migration instead of generic nonzero exit. |
| Policy | Needed if opencode should receive the child-agent safety policy/tool restrictions with first-class semantics. |
| Session export/replace | Needed if OpenCode native storage is not Claude/Codex-compatible. |
| Setup/discovery/settings/migration | Needed if opencode should be fully provider-contract managed rather than static TOML. |

Config/script-only work after P0 code exists:

| Area | Required edit |
|---|---|
| `providers.toml` | Add `[opencode*.session_capture]`; replace `/bin/false` session storage scripts; correct `storage_type`; verify quota/auth refresh. |
| `sessions.toml` | Add `[opencode*] turn_script = "opencode-turns <root/account>"`. |
| model TOMLs | No change needed for the five-account pool unless account names or variants change. |

## Proof plan

This proof plan scopes the P0/P1 runtime claims to runner-owned behavior under fake-provider and isolated-XDG evidence. It does not claim that a live OpenCode binary emits every fixture shape or that the current account/auth mapping is production-correct; those remain external verification items.

| Runtime claim | Proof method | Evidence-class match |
|---|---|---|
| P0 session capture appends OpenCode JSON mode as `--format json` and captures `step_start.sessionID` into the runner's provider session ID. | `cargo test -p oulipoly-runtime --test age_164_c5_resume_capture opencode_launch_argv_uses_format_json_and_captures_session -- --nocapture`; `cargo test -p oulipoly-runtime --test age_164_c5_resume_capture opencode_stdout_json_event_step_start_session_id -- --nocapture`; `cargo test -p oulipoly-config parses_opencode_session_capture_json_args_for_all_accounts -- --nocapture`. | Fake-provider stdout and isolated temp config exercise the runner-owned argv formatter, stdout JSON event parser, capture result mapping, and TOML schema acceptance without touching production DBs or a real OpenCode process. This is the right P0 class for runner capture behavior; live OpenCode stream behavior remains bounded to docs/source evidence. |
| P0 non-UUID OpenCode resume input resolves through `resolve_resume`, composes `--session ses_...`, and does not fail the wake path's public validator. | `cargo test -p oulipoly-state resolve_resume_accepts_opencode_provider_session_id -- --nocapture`; `cargo test -p oulipoly-runtime --test age_164_c5_resume_capture opencode_resume_flag_composes_session -- --nocapture`; `cargo test -p oulipoly-agent-runner --test pr_f_resume_integration opencode_resume_accepts_ses_provider_session_id -- --nocapture`; `cargo test -p oulipoly-agent-runner --test wu_d_proactive_wake_integration opencode_notify_idle_wakes_resume_with_ses_session -- --nocapture`. | Temp DB/config roots and fake providers exercise the production runner resume lookup, Tauri resume entrypoint, wake respawn validation, and provider argv composition. This matches the P0 claim because the validation and DB resolution are runner-owned; no production state DB is used. |
| P1 OpenCode JSON `error` events map 429/rate-limit to `RateLimited` and persistent quota text to `QuotaExhaustedInband`. | `cargo test -p oulipoly-runtime --test age242_terminal_classify_characterization opencode_json_error_429_is_rate_limited -- --nocapture`; `cargo test -p oulipoly-runtime --test age242_terminal_classify_characterization opencode_json_error_persistent_quota_is_quota_exhausted -- --nocapture`. | Fake provider processes emit structured JSON error lines on stdout/stderr and exercise the real provider recognizer dispatch plus terminal-signal classification. This proves runner classification for documented fixture events, not live OpenCode emission semantics. |
| P1 `scripts/opencode-turns` ingests OpenCode session turns through the public `opencode export <sessionID>` interface and persists normalized assistant turn counts. | `cargo test -p oulipoly-runtime --test age243_s7a_session_dispatch opencode_read_turns_ingests_normalized_jsonl -- --nocapture`. | The test wires `OPENCODE_BIN` to a fake public export CLI, runs the bundled adapter script in an isolated temp tree, and asserts `StateDb::count_session_turns("opencode", "ses_fixture")` sees the expected assistant count. This matches runner adapter/ingest behavior; the only residual private-layout touch is session-id directory enumeration when no IDs are supplied, declared in `planning/oc-gate/contracts/oc.contract.md`. |
| P1 five-account OpenCode routing skips exhausted quota-script accounts and selects a healthy account from the configured pool. | `cargo test -p oulipoly-runtime --test age35_routing_characterization opencode_five_account_quota_scripts_route_healthy_accounts -- --nocapture`. | Five fake quota scripts and an isolated state DB exercise the generic production routing service over an OpenCode-shaped provider pool. This is the right class for runner routing selection; it does not prove real OpenCode auth-file mapping or native quota-source correctness. |

## Safe Test Environment Pattern

All proof commands should run with fake provider binaries and isolated state/config roots. Do not run real `agents` or real `opencode` against production state.

Recommended harness flow:

```bash
tmp=$(mktemp -d)
mkdir -p "$tmp/bin" "$tmp/config/oulipoly-agent-runner/models" "$tmp/data"
export HOME="$tmp/home"
export XDG_CONFIG_HOME="$tmp/config"
export XDG_DATA_HOME="$tmp/data"
export PATH="$tmp/bin:$PATH"
export OULIPOLY_STATE_DB="$tmp/state/state.db"
```

Fake `opencode1` behavior for capture tests:

```text
Assert argv contains: run --dangerously-skip-permissions -m openai/gpt-5.5 --variant high --format json
Print JSONL: {"type":"step_start","timestamp":1767036059338,"sessionID":"ses_fixture","part":{"sessionID":"ses_fixture","type":"step-start"}}
Print JSONL: {"type":"text","timestamp":1767036059444,"sessionID":"ses_fixture","part":{"type":"text","text":"ok"}}
Print JSONL: {"type":"step_finish","timestamp":1767036059555,"sessionID":"ses_fixture","part":{"type":"step-finish","reason":"stop"}}
Exit 0
```

The minimum acceptance flow for P0 is:

1. Balanced launch captures `ses_fixture` by `stdout_json_event`.
2. State DB stores provider session ID `ses_fixture` and capture method `stdout_json_event`.
3. Manual resume with `ses_fixture` or chain UUID resolves to active provider `opencode` and active session `ses_fixture`.
4. Runtime resume argv contains `--session ses_fixture`.
5. Mailbox pending message for `ses_fixture` triggers detached wake.
6. Wake child validates its claim and does not exit early due to invalid UUID.
7. Wake child resumes through fake opencode and marks pending mailbox rows delivered.

## 15-Line Executive Summary

1. OpenCode basic launch is wired through `opencode1..5` and all `gpt-*` model pools include the five accounts.
2. OpenCode resume argv is wired: `kind = "flag"`, `flag = "--session"` composes `--session <id>`.
3. OpenCode session capture code support is present; deployment still needs account config and live sandbox confirmation.
4. The right JSON capture event is `step_start` with top-level `sessionID`, producing non-UUID `ses_...` IDs.
5. `stdout_json_event` capture can express `opencode run --format json` through multi-token `json_args`.
6. The last-message sidecar is optional, so OpenCode does not need a sidecar equivalent for session capture.
7. Resume/wake now accepts non-UUID `ses_...` inputs by letting `StateDb::resolve_resume` perform DB-backed validation instead of UUID-only prevalidation.
8. Async-bash wake is covered by fake-provider tests for `ses_...` resume; live OpenCode still needs isolated sandbox confirmation because the direct `--format json` probe previously hung.
9. Session storage is stubbed with `/bin/false` and incorrectly labels OpenCode as `claude_code`; locate/export/replace are not working.
10. `sessions.toml` has no opencode turn scripts, so `session.read_turns`, turn-count routing, and zero-turn classification are incomplete.
11. Quota is partially wired through explicit `chatgpt-usage` scripts, but the Codex auth-file mapping and `/bin/false` auth refresh need verification.
12. Five-account load balancing should work generically once quota/capture/session inputs are valid; no pool-selection code change is obvious.
13. Terminal classification has an OpenCode JSON `error` recognizer for 429/rate-limit and persistent quota fixtures.
14. Setup/discovery recognize OpenCode, but they do not generate a complete working provider contract config for the five opencode accounts.
15. P0 code support is capture arg support plus non-UUID resume/wake support; storage, account config, live capture verification, policy, export/replace, setup, and migration follow.
