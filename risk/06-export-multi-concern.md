# 06-export Multi-Concern Review (Phase 8, against tip fc59558)

## Verdict

**SINGLE_CONCERN**

The branch ships exactly one user-visible behavior change — a new
`agents session export <session-id> [--format canonical-jsonl]` CLI
surface plus the reusable `read_canonical_transcript` library API
declared in `proposals/06-export.md` §1, §2, and §6 — together with
the implementation, byte-faithful JSONL scanner, Claude/Codex
parsers, T1-T9 tests, and planning/audit artifacts that produced
it. Nothing on the branch constitutes a second, independently
shippable feature.

## Diff snapshot

- 10 commits, 18 files, +3544/-0 lines.
- Source: `src-tauri/src/session_export/mod.rs` (new, 419 lines —
  `CanonicalRecord` / `ContentChunk` / `RecordSource` /
  `SessionStorageType` / `ExportSessionMetadata` / `ExportError`,
  `read_canonical_transcript`, byte-faithful `scan_jsonl`, Claude
  and Codex parsers, timestamp validator),
  `src-tauri/src/main.rs` (+235; adds `Subcommands::Session`
  variant, `SessionSubcommands::Export`, `run_session_export`,
  resolver glue `resolve_export_session_metadata`, error mapping
  helpers, stderr JSON emitter), `src-tauri/src/lib.rs` (+1 module
  decl), `src-tauri/Cargo.toml` (+1 line `sha2 = "0.10"`),
  `src-tauri/Cargo.lock` (+1 line for the new direct dep).
- Tests: `src-tauri/tests/initiative_06_export.rs` (283 lines,
  T1-T9 with explicit risk/level/source/observable/residual
  annotations), `src-tauri/tests/fixtures/initiative_06_export.rs`
  (608 lines), `src-tauri/tests/fixtures/mod.rs` (1 line).
- Planning/audit: 10 files under `proposals/`, `research/`,
  `risk/` (+2200 lines covering Phase 2.5 problem map, Phase 3
  proposal Rev 2, Phase 4 audits, Phase 5 hookpoints, Phase 6
  Step 6a contract, and Phase 6 process-tree audit).
- No README delta on this branch (note: §10 of the proposal
  expects README updates; this is a justification / supported-
  surface issue, not a separable concern — see §"Why not split"
  below).

## Concern-by-concern

### 1. `session_export` module + `read_canonical_transcript` API

Single inseparable feature. The CLI in §2 and the public Rust API
in §6 are explicitly twin deliverables of the same proposal: the
harness consumes the CLI; `06-import-replace` later consumes the
Rust API for round-trip parsing. The byte-faithful `scan_jsonl`
helper (`session_export/mod.rs:260-308`) exists only to satisfy
the `source.line/byte_start/byte_end/sha256` requirement that the
CLI promises in §3 / D1. Splitting the library API from the CLI
would land a public `pub fn read_canonical_transcript` with no
caller, and splitting the CLI from the API would leave the binary
without the parser. They must ship together.

### 2. `Session` clap parent + `Export` first (and only) child

No empty-parent risk. `src-tauri/src/main.rs` adds the
`Subcommands::Session { command: SessionSubcommands }` variant,
the `SessionSubcommands::Export { session_id, format }` child,
and the `Subcommands::Session` arm of `run` in the same commit
(b69c6c7); the dispatch routes `Export` to `run_session_export`.
06-export does not stack on 06-locate (per contract §11 the v1
choice is option (c): branch off `main` and define a minimal
local input type), so `Session` is being introduced fresh here
with `Export` as its only child. This matches proposal §2 verbatim
and is anti-scope-clean: no `Locate`, `SchemaProbe`,
`PauseHandshake`, or `ImportReplace` siblings appear, even as
hidden stubs. Bundling the parent with its first child is the
canonical pattern (06-locate's review used the same reasoning for
its `Session`+`Locate` introduction).

### 3. `sha2 = "0.10"` direct dependency

Required by the same feature. Proposal §3 / D1 mandates SHA-256
of exact source bytes on every emitted record; `Cargo.lock`
already had `sha2` transitively (proposal A8) but `Cargo.toml`
had no direct entry. The +1 line in `Cargo.toml` is the minimum
edit needed for the parser to call `Sha256::digest`. CodeRabbit
Pass 1 R2-F05 explicitly chose this over a handwritten
implementation, recorded in the audit history. Not a separable
"add a dep" PR — the dep has no other consumer and ships with
its only call site in `session_export/mod.rs:416-419`.

### 4. `ExportSessionMetadata` local type vs. consuming locate's `SessionMetadata`

This is the explicitly-documented "option (c)" choice from the
contract (§11) and the proposal's A1 invalidator: 06-export
branches off `main`, not off 06-locate, so it cannot import
`session_metadata::SessionMetadata`. The contract identifies
unifying the type as a "follow-up PR after 06-locate merges,"
which makes that unification a future PR, not this PR's concern.
Bundling it here would force this branch to either stack on
06-locate (a sequencing change outside the proposal) or
preemptively unify a type that 06-locate has not yet merged.
Leaving the local type stays inside scope.

### 5. T1-T9 tests + fixture support

Co-scoped with the feature. The 283-line `initiative_06_export.rs`
hits exactly the nine intent-first risks named in the contract §8
and proposal §9 (T1-T9: resolver pass-through, Codex shape, source
preimage, ordering, unsupported records, malformed exit 15,
read-only behavior, compaction, resolver-error mapping). The
608-line fixture file exists only to support these tests. Phase 6
firstness is documented in the process-tree audit
(`risk/06-export-process-tree-audit.md`) — Step 6b authored the
tests before Step 6c authored the product code, with separate
agent invocations and `step6c-reads.md` predating product-code
mtimes. Splitting tests from product would invert that firstness
order or land tests against nonexistent code.

### 6. Planning / audit artifacts (proposals, research, risk)

Co-scoped with the feature. The 10 documents are the Phase 2.5–8
artifacts that produced this exact code: the proposal cites
`research/06-export-problem-map.md`, the contract cites
`research/06-export-hookpoints.md`, the audit history records the
R1-F01 → Rev 2 close and the two CodeRabbit passes, and the
process-tree audit records the firstness chain. Splitting them
off would orphan citations or land code without its rationale.
They are non-executable, additive, and scoped under `06-export*`
filenames so they cannot collide with sibling Initiative 06
artifacts. Bundling them with the feature PR is the design
intent of the workflow.

## Why not MULTI_CONCERN_RECOMMEND_SPLIT

The candidate splits all collapse:

- **Library API vs. CLI**: same proposal section, same module,
  no caller for either half alone (concern 1).
- **`sha2` dep vs. parser**: `sha2` is dead code without the
  parser; the parser is dead code without `sha2` (concern 3).
- **`Session` clap parent vs. `Export` child**: clap's derive
  forbids an empty `#[command(subcommand)]` at runtime, and the
  proposal anti-scope (§7) explicitly forbids empty-stub
  siblings. Landing the parent alone means landing a parent that
  cannot be invoked (concern 2).
- **Tests vs. product**: would invert the firstness chain that
  the Phase 6 process-tree audit just verified (concern 5).
- **Docs vs. code**: would orphan citations (concern 6).

## Why not MULTI_CONCERN_ACCEPTABLE

That verdict would acknowledge multiple concerns and justify
keeping them together. Here there is genuinely one concern: a
single new command + its supporting library API + its required
dependency + colocated tests and artifacts. Each piece either
has zero consumers without the others or fails to build/run
without the others. There is no second concern to acknowledge.

## Out-of-scope items observed (forwarded to other gates)

These are not multi-concern findings, but they exist in the diff
and belong to other gates. Recording them here so they are not
lost:

- **No README changes** despite proposal §10 requiring
  documentation of the new subcommand, JSONL schema, source-hash
  semantics, exit codes, compaction behavior, and partial-stdout
  guarantee. This is a Supported-Surface or Justification finding,
  not a decomposition signal — adding the README updates would
  enlarge the same single concern, not split it. Forward to those
  gates.
- **`canonical_chunk_type` shape divergence**: `ContentChunk` is
  defined as `{ r#type, text }` (`session_export/mod.rs:20-24`),
  not the typed `Text { text } | ToolCall { id, name, input } |
  ToolResult { tool_call_id, text, is_error }` enum that
  proposal §6 specifies. This is a justification / supported-
  surface contract question (does the diff still meet D2?), not a
  multi-concern split signal.

## Recommended action

Proceed to the next gate. No split, no carve-out.
