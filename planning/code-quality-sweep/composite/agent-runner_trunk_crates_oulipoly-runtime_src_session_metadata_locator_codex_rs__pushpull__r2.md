# Push/Pull Coupling Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/trunk`
- `repo_root=/home/nes/projects/agent-runner/trunk`
- `diff_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/diffs/agent-runner_trunk_crates_oulipoly-runtime_src_session_metadata_locator_codex_rs.diff`
- `touched_surfaces_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/touched/agent-runner_trunk_crates_oulipoly-runtime_src_session_metadata_locator_codex_rs.md`
- `output_path=/home/nes/projects/agent-runner/planning/code-quality-sweep/composite/agent-runner_trunk_crates_oulipoly-runtime_src_session_metadata_locator_codex_rs__pushpull__r2.md`
- Touched production file: `crates/oulipoly-runtime/src/session_metadata/locator/codex.rs`

## References Read

- `~/ai/conventions/code-quality.md` lines 21-27, 116-141, 153-159, 301-320. Verified A1 Push-vs-pull system coupling, touched-file ownership, `uncontrolled-source coupler`, and numerical/failure-mode context are present.
- `~/ai/conventions/agent-questions-and-session-graph.md` lines 230-242. Verified the Pull-vs-Push Policy disambiguator exists and is session-graph context transfer, not this system-coupling audit.
- `crates/oulipoly-runtime/src/session_metadata/locator/codex.rs` lines 1-165.
- `crates/oulipoly-runtime/src/session_metadata/locator.rs` lines 1-17, 39-50, 220-236.
- `crates/oulipoly-runtime/src/session_metadata/locator/content_fallback.rs` lines 36-46, 166-172, 223-241.
- `crates/oulipoly-config/src/model/session_storage.rs` lines 28-44, 53-70, 74-139.
- `README.md` lines 330-356 and 469-479.
- `scripts/README.md` lines 111-147.

## Pull Sites Inspected

| ID | Puller | Source | Pull mechanism | Ownership/interface evidence | Verdict | Evidence |
|---|---|---|---|---|---|---|
| PP-001 | `CodexStorageLocator::locate_jsonl` / `require_codex_sessions_dir` | Runner-owned `SessionStorage::Codex { sessions_dir }` config | Reads `request.storage`, matches `SessionStorage::Codex`, and returns the configured `sessions_dir` | LOW source-control proof: `SessionStorage::Codex` schema is in the same repo and is documented in runner config docs | LOW | `codex.rs:20`, `codex.rs:43-50`; `session_storage.rs:28-44`; `README.md:342-356` |
| PP-002 | `canonical_codex_sessions_dir`, `read_codex_directory`, `collect_codex_rollout_matches_at_depth` | Codex sessions filesystem tree below the configured root | Canonicalizes the root and recursively `read_dir`s private provider storage up to depth `<= 4` | No proof that the runner controls Codex's private file layout; docs identify a root but do not prove Codex pushes a stable directory interface | HIGH | `codex.rs:21`, `codex.rs:54-63`, `codex.rs:78-99`, `codex.rs:120-138`; `README.md:342-356` |
| PP-003 | `is_codex_rollout_match` / `rollout_filename_matches_session` | Codex rollout filename convention | Pulls filename shape by requiring `rollout-` prefix, `.jsonl` suffix, and `session_id` substring | Stable common-interface proof absent: repository docs/script docs describe bundled adapter assumptions, but no producer-owned contract that Codex will keep this private naming convention stable | HIGH | `codex.rs:148-160`; `README.md:479`; `scripts/README.md:143-147` |
| PP-004 | `codex_filename_or_content_matches` via `locate_codex_by_content` | Codex JSONL content shape | Falls back from filename lookup to content scan; callee parses `type == "session_meta"` and `payload.id` | Stable common-interface proof absent: the parsed generated artifact shape is Codex private JSONL content, not an inline canonical `~/ai` schema or producer-pushed common interface | HIGH | `codex.rs:32-40`; `content_fallback.rs:36-46`, `content_fallback.rs:223-241`; `scripts/README.md:146` |
| PP-005 | `single_jsonl_match`, `validated_rollout_path`, `located` | Runner locator result interface and existing JSONL path validation | Validates a selected path and maps it into `LocatedTranscript` with `SessionStorageType::CodexSession` | LOW source-control/common-interface proof: these are runner-owned locator abstractions in the same controlled boundary | LOW | `codex.rs:23-28`, `codex.rs:163-164`; `locator.rs:39-63`, `locator.rs:220-236` |

## Uncontrolled-Source Coupler Findings

| ID | Puller | Source | Implicit contract evidence | Missing proof | Decoupling direction | Failure mode |
|---|---|---|---|---|---|---|
| PP-002 | `crates/oulipoly-runtime/src/session_metadata/locator/codex.rs` directory scan helpers | Private Codex sessions filesystem layout under `sessions_dir` | `canonicalize` plus recursive `std::fs::read_dir` and depth cap in `codex.rs:54-63`, `codex.rs:78-99`, `codex.rs:120-138` | both | Codex/provider-specific adapter should push resolved transcript paths or a declared storage index into a runner-owned `TranscriptLocator`/session-location common interface; runtime consumers should pull only from that interface, not Codex private file layout. | uncontrolled-source coupler |
| PP-003 | `crates/oulipoly-runtime/src/session_metadata/locator/codex.rs::rollout_filename_matches_session` | Incidental Codex rollout filename convention | `name.starts_with("rollout-") && name.ends_with(".jsonl") && name.contains(session_id)` in `codex.rs:159-160` | stable common-interface proof absent | Codex/provider-specific adapter should push filename-resolution results into the common transcript-location interface; the runner should pull the resolved path from that interface instead of embedding private naming rules. | uncontrolled-source coupler |
| PP-004 | `crates/oulipoly-runtime/src/session_metadata/locator/codex.rs::codex_filename_or_content_matches` | Unstable/private Codex JSONL generated output shape | Fallback call in `codex.rs:40`; callee expects `type == "session_meta"` and `payload.id` in `content_fallback.rs:223-241` | stable common-interface proof absent | Codex/provider-specific adapter should push session-id-to-transcript mapping into a declared contract/schema or `TranscriptLocator` interface; the runner should pull from that common interface rather than parsing private provider JSONL content. | uncontrolled-source coupler |

## Residual Ambiguity / Stop-Condition Notes

- No test code in the touched file; no exclusions applied.
- `README.md` and `scripts/README.md` document bundled adapter behavior for `codex-locate-transcript`, but they are runner-side reference documentation. They do not prove that the external Codex producer controls or agrees to the private filesystem, filename, or JSONL record shape as a stable common interface.
- No Phase 6 `contract_path` or `proposal_path` was supplied for this ad-hoc audit; Phase 6 blocking rules were not invoked.

Verdict: HIGH
