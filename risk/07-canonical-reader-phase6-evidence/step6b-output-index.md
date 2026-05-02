# Phase 6 Step 6b Output Index

## Test Files

- `src-tauri/tests/initiative_06_import_replace.rs`
  - mtime: `2026-05-01 19:49:21.045746533 -0700`
  - Modified:
    - `t1_valid_replace_claude_stdin_emits_receipt_and_provider_native_transcript`
    - `t2_codex_replace_writes_codex_rollout_jsonl`
    - `t4_preimage_match_succeeds_with_current_canonical_export_hash`
  - Change: removed deferred `#[ignore]` markers and TODO comments so RC-1 and RC-3 coverage is active.

- `src-tauri/tests/initiative_07_canonical_reader_unification.rs`
  - mtime: `2026-05-01 19:52:11.853679684 -0700`
  - Added:
    - `t_rc2_claude_content_array_with_tool_use_chunk_accepts_export_preimage`
    - `t_rc4_claude_compaction_summary_accepts_export_preimage`
    - `t_rc5_claude_out_of_order_timestamps_reject_without_mutation`
    - `t_rc6_codex_without_session_meta_rejects_without_mutation`

## Risk / Level / Source / Observable / Residual

- `t1_valid_replace_claude_stdin_emits_receipt_and_provider_native_transcript`
  - Risk: valid Claude stdin replacement may write canonical bytes instead of provider-native bytes.
  - Level: CLI integration.
  - Source: contract §7 T-valid-replace; proposal §9.1 Valid stdin replace; A1, A3, A5.
  - Observable: exit 0; receipt fields are populated; transcript is Claude-native; export semantics match imported canonical records.
  - Residual: does not exhaust every Claude content variant.

- `t2_codex_replace_writes_codex_rollout_jsonl`
  - Risk: Codex rendering may be accidentally treated as unsupported or written in Claude shape.
  - Level: CLI integration.
  - Source: contract §7 T-codex-replace; proposal §9.1 Postimage round-trip; A3, A5.
  - Observable: exit 0; receipt storage_type is codex_session; transcript contains Codex rollout records.
  - Residual: does not cover Codex compaction records.

- `t4_preimage_match_succeeds_with_current_canonical_export_hash`
  - Risk: preimage protection may compare the wrong hash domain or run outside the lock.
  - Level: CLI integration.
  - Source: contract §7 T-preimage-match; proposal §5 and §6 hash details; A4.
  - Observable: current canonical export hash succeeds when supplied through `--preimage-sha256`.
  - Residual: does not prove TOCTOU protection against non-cooperating external writers.

- `t_rc2_claude_content_array_with_tool_use_chunk_accepts_export_preimage`
  - Risk: RC-2 Claude array content containing non-text chunks may hash differently between export and import-replace.
  - Level: CLI integration.
  - Source: `research/07-canonical-reader-divergence-rca.md` RC-2; `proposals/07-canonical-reader-unification.md` §4.
  - Observable: export-derived preimage hash is accepted; receipt `preimage_sha256` equals that oracle.
  - Residual: covers one representative `tool_use` chunk, not every Claude structured content kind.

- `t_rc4_claude_compaction_summary_accepts_export_preimage`
  - Risk: RC-4 Claude compaction summaries may leave pre-summary turns in import-replace's preimage hash.
  - Level: CLI integration.
  - Source: `research/07-canonical-reader-divergence-rca.md` RC-4; `proposals/07-canonical-reader-unification.md` §4.
  - Observable: export-derived preimage hash is accepted; receipt `preimage_sha256` equals that oracle.
  - Residual: covers one latest compaction boundary, not multiple summary markers.

- `t_rc5_claude_out_of_order_timestamps_reject_without_mutation`
  - Risk: RC-5 out-of-order Claude timestamps may be accepted by import-replace after export rejects them.
  - Level: CLI integration.
  - Source: `research/07-canonical-reader-divergence-rca.md` RC-5; `proposals/07-canonical-reader-unification.md` §4; TA-07-F03.
  - Observable: export and import-replace both exit non-zero; transcript bytes, `session_turns`, and pending journals are unchanged.
  - Residual: asserts one decreasing timestamp pair, not every timestamp normalization edge.

- `t_rc6_codex_without_session_meta_rejects_without_mutation`
  - Risk: RC-6 Codex transcripts missing `session_meta` may be accepted by import-replace after export rejects them.
  - Level: CLI integration.
  - Source: `research/07-canonical-reader-divergence-rca.md` RC-6; `proposals/07-canonical-reader-unification.md` §4; TA-07-F03.
  - Observable: export and import-replace both exit non-zero; transcript bytes, `session_turns`, and pending journals are unchanged.
  - Residual: covers absent `session_meta`, not mismatched `session_meta` id.

## Verification

- `cargo fmt --manifest-path src-tauri/Cargo.toml --check` passed.
- `cargo build --tests` passed from `src-tauri/`.
- `cargo test --test initiative_07_canonical_reader_unification` failed pre-fix as expected:
  - RC-2 and RC-4 fail with `preimage-mismatch`.
  - RC-5 and RC-6 fail because pre-fix import-replace exits 0 instead of rejecting before mutation.
