# Phase 8 Justification Review — 06-export

**Verdict:** `LOW_CONCERN`
**Reviewer:** claude-opus
**Scope:** `git diff main..06-export` (10 commits, 18 files, +3544 lines).
**Inputs:** `research/06-export-contract.md`; `proposals/06-export.md` (Rev 2);
`research/06-export-problem-map.md`; `risk/06-export-audit-history.md`;
`risk/06-export-process-tree-audit.md` (PASS-WITH-ADVISORY).

## Summary

The diff implements `agents session export <session-id>
[--format canonical-jsonl]` exactly as pinned by the Rev 2 proposal and
the Phase 6 contract. Every product file (`src-tauri/Cargo.toml`,
`src-tauri/src/lib.rs`, `src-tauri/src/main.rs`,
`src-tauri/src/session_export/mod.rs`) traces to a numbered §1-§9 contract
clause; every test in `src-tauri/tests/initiative_06_export.rs` cites a
T1-T9 risk row from contract §8. No behavior change leaks into adjacent
surfaces (`trace`, `repl`, `resume`, `migrate-db`, `migrate-config`); no
unrelated refactor, no drive-by cleanup, no speculative abstraction.
Two minor findings are recorded; both are LOW and foldable into the next
fix pass.

## Diff scope

```
proposals/06-export.md                            +461   pipeline phase artifact
research/06-export-contract.md                    +213   pipeline phase artifact
research/06-export-hookpoints.md                  +396   pipeline phase artifact
research/06-export-problem-map.md                 +150   pipeline phase artifact
risk/06-export-audit-history.md                    +39   pipeline phase artifact
risk/06-export-audit.md                           +115   pipeline phase artifact
risk/06-export-process-tree-audit.md               +81   pipeline phase artifact
risk/06-export-scope.md                           +206   pipeline phase artifact
risk/06-export-shortcut.md                        +128   pipeline phase artifact
risk/06-export-supported-surface.md               +206   pipeline phase artifact
src-tauri/Cargo.lock                                +1   sha2 dep (transitive entry)
src-tauri/Cargo.toml                                +1   sha2 = "0.10" (D1 hash)
src-tauri/src/lib.rs                                +1   pub mod session_export
src-tauri/src/main.rs                             +235   Session/Export clap, dispatcher, error mapping
src-tauri/src/session_export/mod.rs               +419   canonical reader, parsers, scanner
src-tauri/tests/fixtures/initiative_06_export.rs  +608   T1-T9 fixture helpers
src-tauri/tests/fixtures/mod.rs                     +1   fixture mod export
src-tauri/tests/initiative_06_export.rs           +283   T1-T9 risk-annotated tests
```

The 10 pipeline-artifact files (`research/`, `proposals/`, `risk/`) are
required by `~/ai/workflows/implementation-pipeline.md` Phases 2.5-6. They
are not "code" for justification purposes; they are the audit trail. Each
exists because the workflow demanded it (problem map → proposal →
scope/shortcut/audit/supported-surface → contract → hookpoints →
process-tree audit → audit history). I did not re-evaluate their content
in this gate.

## Per-change justification — product code

| File | Change | Required by | Verdict |
|---|---|---|---|
| `src-tauri/Cargo.toml:13`, `Cargo.lock:2252` | `sha2 = "0.10"` direct dep. | Proposal §3 D1 (`source.sha256`); A8; §12 residual; CodeRabbit Pass 1 R2-F05 (replaced handwritten SHA-256). | ✅ |
| `src-tauri/src/lib.rs:8` | `pub mod session_export;` | Proposal §6 ("Expose it from `src-tauri/src/lib.rs`"). | ✅ |
| `src-tauri/src/main.rs:8-10` | Import `ExportError`, `ExportSessionMetadata`, `SessionStorageType`, `read_canonical_transcript`. | §4, §5 (CLI consumes the §6 reader API). | ✅ |
| `src-tauri/src/main.rs:106-109` | `Subcommands::Session { command }`. | §2 ("Extend the `SessionSubcommands` enum"); contract §1 clap shape. | ✅ |
| `src-tauri/src/main.rs:176-184` | `enum SessionSubcommands { Export { session_id, format } }`. | §2 clap shape; contract §1. | ✅ |
| `src-tauri/src/main.rs:323-327` | Dispatch arm to `run_session_export`. | §2 + §4 step 1. | ✅ |
| `src-tauri/src/main.rs:504-547` | `run_session_export` (format guard, UUID parse, resolve, read, emit). | §4 steps 1-2 + step 10 (build complete `Vec` then write); §3 (compact JSONL); §5 (exit codes). | ✅ |
| `src-tauri/src/main.rs:549-650` | `resolve_export_session_metadata` (StateDb open, configs, resolve_resume, provider→storage, locate_transcript). | §4 steps 3-7; §8 STATE_DIR carve-out (Rev 2 R1-F01 closure); ResumeError→ExportError mapping per §5. | ✅ |
| `src-tauri/src/main.rs:651-714` | `export_error_exit_code` / `_code` / `_message` / `emit_export_error` / `emit_export_json_error`. | §5 exit-code table; §3 stderr `{"error":{"code","message"}}`. | ✅ |
| `src-tauri/src/session_export/mod.rs:8-86` | `CanonicalRecord`, `ContentChunk`, `RecordSource`, `SessionStorageType`, `ExportSessionMetadata`, `ExportError`. | §6 public types; contract §2; contract §11(c) "minimal local equivalent". | ✅ (one minor — F1 below) |
| `src-tauri/src/session_export/mod.rs:88-99` | `read_canonical_transcript` dispatcher. | §6 D7 reader API; §4 step 7. | ✅ |
| `src-tauri/src/session_export/mod.rs:101-164` | `parse_claude_code_jsonl` (Claude turns + `isCompactSummary` boundary). | §4 step 8 D4 Claude compaction; §3 schema; contract §6. | ✅ |
| `src-tauri/src/session_export/mod.rs:166-236` | `parse_codex_rollout_jsonl` (`session_meta` match + `response_item` walk). | §4 step 8 D4 Codex (no compaction marker); §3 schema; contract §6. | ✅ |
| `src-tauri/src/session_export/mod.rs:238-308` | `SourceLine` + `scan_jsonl` byte-tracking scanner. | §3 D1 implementation cost note ("byte-preserving JSONL scanner instead of `BufRead::lines()`"). | ✅ |
| `src-tauri/src/session_export/mod.rs:310-355` | `required_string` / `required_timestamp` / `validate_timestamp_order`. | §3 (timestamps RFC3339 mandatory); §4 step 9 D5 (regression → exit 15). | ✅ |
| `src-tauri/src/session_export/mod.rs:357-414` | `extract_claude_content` / `extract_content_chunks` / `canonical_chunk_type` / `text_chunk`. | §3 D2 chunk variants; covers Claude `message.content[]` shape and Codex `input_text`/`output_text` native types. | ✅ |
| `src-tauri/src/session_export/mod.rs:416-419` | `sha256_hex`. | §3 D1 lowercase-hex contract; CodeRabbit R2-F05/R3-F01 history (kept simple after replacing handwritten impl). | ✅ |

## Per-change justification — tests + fixtures

Every `#[test]` in `src-tauri/tests/initiative_06_export.rs` carries the
required Phase 6 risk header (Risk / Level / Source / Observable / Residual)
and maps 1-to-1 onto contract §8:

| Test | Contract row | Verdict |
|---|---|---|
| `export_claude_session_emits_canonical_jsonl_records` | T1 particular-integration | ✅ |
| `export_codex_session_emits_canonical_jsonl_records` | T2 particular-integration | ✅ |
| `canonical_reader_source_metadata_matches_jsonl_preimage` | T3 component | ✅ |
| `canonical_reader_preserves_provider_jsonl_order` | T4 component | ✅ |
| `canonical_reader_emits_unsupported_record_placeholders` | T5 component | ✅ |
| `export_malformed_transcript_exits_15_without_partial_stdout` | T6 particular-integration | ✅ |
| `export_does_not_mutate_state_rows_transcript_or_config` | T7 particular-integration | ✅ |
| `canonical_reader_emits_live_transcript_from_latest_compaction_boundary` | T8 component | ✅ |
| `export_unknown_well_formed_uuid_exits_session_not_found` / `…_ambiguous_session_exits_…` / `…_unsupported_storage_exits_…` | T9 particular-integration (3 sub-cases) | ✅ |

`tests/fixtures/initiative_06_export.rs` and `tests/fixtures/mod.rs` exist
solely to host the per-test fixture builders (`cli_*_fixture`,
`component_*_fixture`, `assert_*`, `parse_*_jsonl`). All helpers are
called by the T1-T9 tests. I checked `seed_provider_quota_exhausted` — used
by `cli_read_only_fixture` to widen T7's no-mutation observable to include
quota rows; this is a justified scope expansion of the named risk, not
drift.

## Drift / drive-by cleanup

None observed.

- `src-tauri/src/main.rs` adds only the new `Session` / `Export` clap
  variants, the dispatcher arm, and the `run_session_export` /
  `resolve_export_session_metadata` / error-emission helpers. No edits to
  existing handlers (`run_trace_command`, `repl`, `resume`, `migrate_db`,
  `migrate_config`).
- `src-tauri/src/lib.rs` adds one line. No reordering, no rename.
- `src-tauri/Cargo.toml` adds one direct dependency line in alphabetical
  order; `Cargo.lock` shows the corresponding manifest entry only. No
  dep upgrades, no feature flag changes, no other manifest churn.
- No edits to `src-tauri/src/sessions/mod.rs`,
  `src-tauri/src/state/db.rs`, `src-tauri/src/trace/`,
  `src-tauri/src/migration/`, `src-tauri/src/config/*`, or any GUI/Tauri
  code. Adjacent surfaces declared in §11 supported-surface track stay
  byte-identical.
- The Rev 2 §8 `STATE_DIR` mkdir clause is reused via the existing
  `agent_runner_lib::sessions::locate_transcript` helper; export does not
  duplicate or reimplement that side-effecting code path.

## Speculative abstractions

None.

- The new `session_export` module exports exactly the types named in
  contract §2: `CanonicalRecord`, `ContentChunk`, `RecordSource`,
  `SessionStorageType`, `ExportSessionMetadata`, `ExportError`, plus the
  `read_canonical_transcript` entry point and two `pub` parser fns
  exercised by component tests. No traits, no generics, no plugin
  registry, no async runtime.
- `ContentChunk` is intentionally a flat `{ type, text }` shape rather
  than the discriminated `Text | ToolCall | ToolResult` enum that
  proposal §6 sketched. The contract §2 narrowed the v1 surface to this
  flat shape ("future fields optional"); choosing the simpler shape is
  the opposite of speculative.
- `read_canonical_transcript` returns `Vec<CanonicalRecord>` (D7),
  matching the contract's "buffer-and-validate" choice. No iterator,
  streaming wrapper, or trait-object indirection was introduced.

## Behavior changes not required for the stated purpose

None.

- No alternate output formats (proposal §7 anti-scope); only
  `canonical-jsonl` is parsed.
- No fallback to `session_turns` for content/ordering/source metadata
  (D3, anti-scope).
- No telemetry, invocation rows, cursor writes, scan triggers, or
  provider launch.
- No GUI surface or Tauri command exposure.
- No backwards-compat shim between `SessionStorageType::Other` and any
  parser (rejected at exit `12` per §4 step 6 / contract §5).

## Cleanup that should ship separately

None observed in the diff.

## Findings (LOW; foldable into next fix pass)

### F1 (LOW, justification) — `ExportSessionMetadata.chain_id` is unused inside `session_export`

`src-tauri/src/session_export/mod.rs:57` declares `pub chain_id: String`
on `ExportSessionMetadata`, and `src-tauri/src/main.rs:646` populates it
from `resolved.chain_id`. No code in `src-tauri/src/session_export/`
reads the field (`grep chain_id src-tauri/src/session_export` matches
only the struct declaration; `parse_claude_code_jsonl` /
`parse_codex_rollout_jsonl` / `RecordSource` consume only `session_id`,
`provider_name`, `storage_type`, and `jsonl_path`). The contract §11
explicitly listed `chain_id` in option (c)'s "minimal local equivalent"
shape, so the field is contract-driven rather than drift, but in the
delivered diff it is a write-only property.

Fix-pass options (any one is acceptable; do not block):

- Remove the field from `ExportSessionMetadata` and from main.rs:646;
  the contract §11 follow-up that "unifies the type when 06-locate
  merges" is the natural place to reintroduce it if needed.
- Or, fold it into `RecordSource` so that exported canonical lines carry
  chain provenance (would require contract §3 + §6 + tests update; not
  done in this PR).

This is a single-line, single-field issue; severity LOW.

### F2 (LOW, justification) — Proposal §10 README updates not present in diff

`proposals/06-export.md` §10 lists six explicit README deliverables (synopsis
entry, JSONL output explanation, source-conventions paragraph,
compaction behavior, exit-code table, trace-vs-export divergence note).
The diff contains zero `README.md` changes (`git diff main..06-export
-- README.md` is empty; `grep -n 'session export\|canonical-jsonl'
README.md` returns no matches).

Justification posture: this is the same staging the 06-locate branch
used (README was a separate fix-pass commit `2605b37` after Step 6c, and
06-locate's Phase 8 justification re-run closed an analogous F1). The
omission is deferral, not drift, and matches the project's
documented split-commit pattern. The next CodeRabbit fix-pass commit
on this branch is the natural place to land §10 verbatim. Severity LOW.

(This finding is informational from the justification gate's
perspective. The supported-surface and test-audit gates own the
"docs-truthful" obligation; if either of those gates also flags this
absence, treat it as one finding routed to that gate, not two.)

## Audit-history coherence

`risk/06-export-audit-history.md` records: Round 1 audit MEDIUM (R1-F01
closed by Rev 2 §8 `STATE_DIR` mkdir clause); CodeRabbit Pass 1 (5
applied / 1 skipped with cited contract §4 reason for R2-F03);
CodeRabbit Pass 2 (`CONVERGED:ALL_CHURN`, 4 micro-optimizations
skipped). Every applied finding (R2-F01 fixture-derived offsets, R2-F02
session_id mismatch reasoning, R2-F04 fixture-derived hashes, R2-F05
sha2 direct dep, R2-F06 unused compaction seeding removal) is observable
in the diff. No applied finding leaked behavior outside its stated
scope.

## Process-tree audit interaction

`risk/06-export-process-tree-audit.md` is PASS-WITH-ADVISORY
(`P6-PTA-ADV-001`: Step 6b output-index provenance fields were
back-amended after Step 6c completion). The advisory is a paperwork-order
issue, not a justification issue: tests and Step 6c read-evidence still
predate the product-code commit (`b69c6c7 feat(06-export): Phase 6 Step
6c`). No justification finding follows from the advisory.

## Conclusion

Every change in `git diff main..06-export` traces to either a numbered
contract / proposal clause, a Phase 6 process artifact, or a CodeRabbit
fix-pass record. Drift, drive-by cleanup, and speculative abstractions
are absent. Two LOW findings (F1 unused `chain_id` field; F2 deferred
§10 README updates) can be folded into the next fix pass without
destabilizing the diff. Phase 8 Justification gate clears at
`LOW_CONCERN`.
