# opencode P0+P1 Phase-6a Code-Quality Contract

Scope inputs read:

- `/home/nes/ai/conventions/code-quality.md`
- `planning/oc-gate/gates/diff.patch`
- `planning/oc-gate/gates/touched-surfaces.md`
- Touched production Rust files listed in `planning/oc-gate/gates/touched-surfaces.md`

## Component declared roles

Component scoring mode: cohesion should be scored per touched file or focused touched sub-surface. This WU is not one cohesive all-purpose component; it spans provider config, session capture, terminal recognition, resume lookup, and Tauri resume validation.

If an auditor is forced to score the entire touched production-Rust set as one component, the honest changed-function union is:

`accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator`

This is the honest post-split union for changed production Rust bodies after OpenCode JSON stream candidate-line filtering was extracted into a helper.

Focused sub-surfaces:

- OpenCode terminal recognition: `orchestration`, `parser`, `filter`, `formatter`, `validator`, `mapper`, `accessor`, `predicate`
- Session capture JSON args, planning, streaming observation, and capture results: `formatter`, `validator`, `mapper`, `orchestration`
- Provider identity dispatch and facade exports: `mapper`, `orchestration`, `accessor`
- Resume input, resume acceptance, and non-UUID resume resolution: `validator`, `predicate`, `orchestration`
- Config parsing surfaces touched only by tests or schema fields keep their file-local role declarations rather than expanding the WU component role set.

## Per-file declared roles

| File | Declared roles | Production-change note |
|---|---|---|
| `crates/oulipoly-config/src/model.rs` | `parser`, `validator`, `mapper`, `formatter`, `accessor`, `predicate` | Existing file-local role set; changed production role is `validator` for `SessionCapture::validate`. |
| `crates/oulipoly-config/src/providers.rs` | `parser`, `validator`, `mapper`, `formatter`, `accessor`, `predicate`, `filter`, `orchestration` | Whole-file provider config loader has all eight roles; this WU only adds tests, no changed production function inventory entry. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration` | Existing facade role; this WU only updates test fixtures, no changed production function inventory entry. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs` | `mapper`, `predicate`, `accessor`, `formatter`, `orchestration` | Existing file-local roles; changed production functions are capture result mapping and stdout restore selection. |
| `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs` | `orchestration` | Existing file-local role; changed production function passes capture plan through the supervisor sequence. |
| `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs` | `parser`, `mapper`, `predicate`, `accessor`, `filter`, `orchestration` | Existing provider identity surface plus OpenCode recognizer dispatch; `orchestration` covers the changed delegating recognizer dispatch. |
| `crates/oulipoly-runtime/src/executor/cli/result.rs` | `mapper`, `orchestration` | Existing file-local roles; changed production function maps streamed session IDs into raw results. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/args.rs` | `formatter` | Existing file-local role; changed production function formats `json_args` plus optional last-message argv. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs` | `mapper`, `orchestration`, `validator` | Existing file-local roles; changed production functions validate/map stdout JSON capture config. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `mapper`, `orchestration`, `predicate` | Existing file-local roles; changed and added production functions sequence supervision and streamed ID observation. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `mapper` | Existing file-local role; changed production function initializes the new streamed ID field. |
| `crates/oulipoly-runtime/src/executor/mod.rs` | `accessor`, `mapper`, `orchestration` | Existing facade role set; this WU only adds the OpenCode recognizer re-export, no changed production function inventory entry. |
| `crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs` | `predicate` | Existing provider-specific phrase surface; OpenCode guessed phrases are intentionally disabled until live wording is verified. |
| `crates/oulipoly-runtime/src/executor/providers/mod.rs` | `accessor` | Module exposure surface only; no production functions. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `orchestration`, `parser`, `filter`, `formatter`, `validator`, `mapper`, `accessor`, `predicate` | New provider recognizer; OpenCode JSON stream decoding, line filtering, event validation, terminal mapping, and evidence formatting are split into single-classification helpers. |
| `crates/oulipoly-runtime/src/executor/terminal_signal.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `validator` | Existing shared DTO/helper roles; this WU only adds tests, no changed production function inventory entry. |
| `crates/oulipoly-state/src/db.rs` | `accessor`, `mapper`, `formatter`, `predicate`, `validator`, `parser`, `orchestration`, `filter` | Existing declared multi-role StateDb persistence adapter; changed production function is resume-resolution orchestration. |
| `src-tauri/src/run/resume/orchestration.rs` | `orchestration`, `validator`, `accessor`, `mapper`, `filter`, `predicate`, `formatter` | Existing resume orchestration roles; changed production function delegates to the relaxed validator. |
| `src-tauri/src/run/resume/validator.rs` | `validator` | Replaced UUID-only validation with non-empty resume input validation. |
| `scripts/opencode-turns` | `parser`, `accessor`, `mapper`, `formatter`, `filter`, `validator`, `orchestration` | Non-Rust adapter invokes public `opencode session list` for implicit discovery, invokes public `opencode export <sessionID>` for content, and maps exported session JSON to normalized turn JSONL. |

Touched non-Rust adapter surface: `scripts/opencode-turns`. It is intentionally excluded from the Rust A5 function inventory, but has an explicit A6 per-file role declaration above because A6 is language-neutral.

## Function inventory

Production functions added or meaningfully changed in the touched Rust are listed below. Each entry has exactly one A1 classification after the OpenCode JSON helper split.

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-config/src/model.rs::SessionCapture::validate` | `validator` | Accepts or rejects `SessionCapture` by kind, now allowing non-empty `json_args` or `json_flag` and optional last-message sidecar. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/args.rs::stdout_json_event_capture_args` | `formatter` | Formats capture configuration into provider argv tokens for JSON mode and optional last-message reconstruction. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs::build_stdout_json_event_capture_plan` | `mapper` | Maps a valid `SessionCapture` into `CapturePlan`, argv fragments, and temp-file ownership; validation is delegated. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs::stdout_json_event_json_args` | `validator` | Accepts non-empty `json_args` or legacy `json_flag` and returns the validated argv fragment, otherwise errors. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs::finalize_capture` | `mapper` | Maps capture plan variant plus stdout or streamed session ID into a `SessionCaptureResult`. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs::finalize_stdout_json_event_capture` | `mapper` | Maps a streamed session ID or parsed stdout JSON event into the stdout-json-event capture result. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs::maybe_restore_plain_stdout` | `orchestration` | Chooses sidecar restoration only when the capture plan owns a last-message path, otherwise uses stdout fallback. |
| `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs::execute_provider_with_arg_parts_and_supervisor_config` | `orchestration` | Sequences launch assembly, supervised execution with capture plan, return-channel cleanup, and raw result construction. |
| `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs::ProviderRecognizer::for_provider` | `mapper` | Maps provider name or executable basename into the bounded recognizer enum, now including OpenCode. |
| `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs::ProviderRecognizer::recognize` | `orchestration` | Delegates evidence recognition to the selected provider-specific recognizer, now including OpenCode. |
| `crates/oulipoly-runtime/src/executor/cli/result.rs::raw_result_from_supervised_output` | `mapper` | Maps `SupervisedOutput`, streamed session ID, capture result, terminal data, and returned artifacts into `RawResult`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs::run_provider_supervisor` | `orchestration` | Thinly delegates provider supervision with capture plan and maps supervisor errors for the executor. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs::execute_with_supervisor` | `orchestration` | Runs the existing child lifecycle and now observes streamed stdout session IDs during supervision. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs::observe_streamed_session_id` | `orchestration` | Guards already-observed state, delegates JSON session parsing, and records the first parsed streamed ID. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs::supervised_output_from_terminal` | `mapper` | Maps terminal status, optional live signal, stdout/stderr, and real status into `SupervisedOutput` with no streamed ID by default. |
| `crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs::output_reports_missing_session` | `predicate` | Answers whether provider output contains verified missing-session phrases; OpenCode guessed phrases are disabled. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::Recognizer::recognize` | `orchestration` | Sequences pre-quota status recognition, OpenCode JSON error recognition, post-quota status recognition, and unknown fallback. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::opencode_json_error_signal` | `orchestration` | Tries stdout stream recognition first and stderr stream recognition second without adding parsing logic. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::json_error_signal_from_stream` | `orchestration` | Delegates stream decoding, non-empty-line filtering, line classification, and evidence formatting to named helpers. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::stream_text` | `parser` | Decodes provider output bytes into lossy UTF-8 text. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::non_empty_stream_lines` | `filter` | Selects trimmed non-empty candidate lines from decoded stream text. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::json_error_line_evidence` | `formatter` | Formats a bounded terminal-signal evidence excerpt from a candidate JSON line. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::json_error_line_kind` | `orchestration` | Sequences JSON parsing, error-event validation, message normalization, and terminal-kind mapping through single-role helpers. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::parse_json_error_line` | `parser` | Parses one provider output line as JSON. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::json_error_event_error` | `validator` | Accepts only OpenCode `type = "error"` events and returns their `error` object. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::normalized_error_message` | `formatter` | Formats the provider error message into lowercase text for predicate matching. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::terminal_signal_kind_from_json_error` | `mapper` | Maps classified OpenCode error facts into the runner terminal-signal kind enum. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::error_status_code` | `accessor` | Retrieves the first supported status-code field path and delegates numeric conversion. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::error_message` | `accessor` | Retrieves the supported OpenCode error message field path and returns an owned string. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::value_at_paths` | `accessor` | Retrieves the first JSON value found at a list of pointer paths without changing meaning. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::number_or_numeric_string` | `parser` | Parses a numeric JSON value or numeric string into an integer status code. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::error_reports_rate_limit` | `predicate` | Answers whether status code or message text reports a rate limit. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::message_reports_rate_limit` | `predicate` | Answers whether a lowercased provider message reports rate limiting. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs::error_reports_persistent_quota` | `predicate` | Answers whether lowercased provider message text reports persistent quota exhaustion. |
| `crates/oulipoly-state/src/db.rs::StateDb::resolve_resume` | `orchestration` | Sequences wrong-ID-kind rejection, chain lookup, active segment lookup, model resolution, and result assembly; UUID validation was removed. |
| `src-tauri/src/run/resume/orchestration.rs::reject_invalid_resume_input` | `orchestration` | Delegates validation and routes failure to stderr plus exit code, now using the non-UUID validator. |
| `src-tauri/src/run/resume/validator.rs::validate_resume_input` | `validator` | Accepts any non-empty resume input and rejects only blank input. |

Removed production function, not a current-body A5 inventory item: `crates/oulipoly-state/src/db.rs::StateDb::validate_resume_input_uuid`.

No production function inventory entries for these touched Rust files because the diff only changed tests, comments, exports, module declarations, or data fields: `crates/oulipoly-config/src/providers.rs`, `crates/oulipoly-runtime/src/executor/cli.rs`, `crates/oulipoly-runtime/src/executor/mod.rs`, `crates/oulipoly-runtime/src/executor/providers/mod.rs`, `crates/oulipoly-runtime/src/executor/terminal_signal.rs`.

## Adapter declarations

```yaml
adapter_declarations:
  - component: crates/oulipoly-runtime/src/executor/providers/opencode.rs
    role: adapter
    Translates:
      - opencode-format-json-event-stream-contract
      - runner-terminal-signal-contract
  - component: crates/oulipoly-runtime/src/executor/cli/session_capture
    role: adapter
    Translates:
      - provider-stdout-json-event-session-contract
      - runtime-session-capture-plan-result-contract
  - component: crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs
    role: adapter
    Translates:
      - provider-output-resume-mismatch-phrase-contract
  - component: scripts/opencode-turns
    role: adapter
    Translates:
      - opencode-session-list-output-contract
      - opencode-export-session-json-contract
      - runner-normalized-session-turn-jsonl-contract
```

`opencode-format-json-event-stream-contract` covers the OpenCode `--format json` stream fields used here: `step_start.sessionID` for session capture context and `error` events carrying `statusCode`, `status_code`, `status`, or message text that reports `429`, rate limit, or quota exhaustion.

`provider-stdout-json-event-session-contract` covers provider stdout JSON events selected by configured `event_type` and `event_id_path`, with launch-time `json_args` or legacy `json_flag`, optional last-message sidecar argv, live streamed session observation, and final capture-result mapping.

`scripts/opencode-turns` is a non-Rust adapter surface. It translates public `opencode session list` output into candidate session IDs, translates the public `opencode export <sessionID>` JSON result into runner-normalized turn JSONL, and is intentionally excluded from the Rust A5 function inventory. It does not read OpenCode private storage or native message JSON files.

## Residual declarations

No residual declarations remain for `scripts/opencode-turns`; implicit session discovery and content export both use public OpenCode CLI interfaces.

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: crates/oulipoly-config/src/model.rs
    role: intrinsic-surface
    Domain: model_provider_session_config
    Owns:
      - session_capture_json_args_validation
      - stdout_json_event_capture_required_fields
  - component: crates/oulipoly-runtime/src/executor/cli/provider_identity.rs
    role: intrinsic-surface
    Domain: provider_identity
    Owns:
      - opencode_provider_prefix_recognition
      - opencode_command_executable_token_recognition
      - terminal_signal_recognizer_dispatch
  - component: crates/oulipoly-runtime/src/executor/cli/session_capture
    role: intrinsic-surface
    Domain: provider_stdout_json_session_capture
    Owns:
      - stdout_json_event_session_id_path_extraction
      - json_args_or_json_flag_capture_argv
      - optional_last_message_sidecar
  - component: crates/oulipoly-runtime/src/executor/providers/opencode.rs
    role: intrinsic-surface
    Domain: opencode_terminal_signal_json
    Owns:
      - opencode_error_event_status_paths
      - opencode_rate_limit_message_patterns
      - opencode_persistent_quota_message_patterns
  - component: crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs
    role: intrinsic-surface
    Domain: resume_missing_session_phrase_set
    Owns:
      - verified_resume_missing_session_phrases
      - opencode_phrase_verification_deferral
      - resume_session_mismatch_phrase_recognition
  - component: crates/oulipoly-state/src/db.rs
    role: intrinsic-surface
    Domain: state_db_resume_resolution
    Owns:
      - provider_session_id_resume_lookup
      - wrong_id_kind_invocation_guard
```

No intrinsic-surface declaration is made for `src-tauri/src/run/resume/validator.rs` or `src-tauri/src/run/resume/orchestration.rs`; those files consume the resume-input and state-resolution contracts rather than owning a provider or parsing surface.
