# 06-locate Multi-Concern Review (Phase 8 RE-RUN against tip 2605b37)

## Verdict

**SINGLE_CONCERN**

The branch ships exactly one user-visible behavior change — a new
`agents session locate <session-id>` CLI surface plus the reusable
`SessionMetadata` library API declared in `proposals/06-locate.md`
§1 — together with the implementation, tests, README per §10, and
planning/audit history that produced it. Nothing on the branch
constitutes a second, independently shippable feature.

## Diff snapshot

- 14 commits, 22 files, +5044/-21 lines.
- Source: `src-tauri/src/session_metadata/mod.rs` (new, 447 lines),
  `src-tauri/src/main.rs` (+129; adds `Session` enum variant and
  `run_session_locate`), `src-tauri/src/lib.rs` (+1 module decl),
  `src-tauri/src/trace/mod.rs` (21-line move of `TranscriptState`).
- Tests: `tests/initiative_06_locate.rs`,
  `tests/session_metadata_component.rs`,
  `tests/fixtures/initiative_06.rs`, `tests/fixtures/mod.rs`
  (1542 lines).
- Docs: `README.md` (+42/-3) — adds Subcommands header line,
  "Locating a Session" section, and SQL note pointing at the new
  command.
- Planning/audit: 13 files under `proposals/`, `research/`,
  `risk/`, `initiatives/` (+2883 lines).

## Concern-by-concern

### 1. `TranscriptState` moved out of `trace`

Not a separable prep PR. The move is 21 lines: the enum and its
`as_str` helper relocate from `src-tauri/src/trace/mod.rs` into
the new `session_metadata` module, and trace re-imports it via
`use crate::session_metadata::TranscriptState`. Both modules need
the type and the new module is the natural owner per
proposal §6. Landing the move ahead of the feature would be a
no-op refactor that gains nothing — there is no second consumer
on `main` today. Bundling matches the §1 "what changes" list,
which calls out factoring transcript-state logic alongside the
feature.

### 2. Planning / audit artifacts (proposals, research, risk, initiatives)

Co-scoped with the feature. The 13 documents are the Phase 2.5–8
artifacts that produced this exact code: the proposal cites
`research/06-locate-problem-map.md`, the contract cites
`research/06-locate-hookpoints.md`, the README cites
`proposals/06-locate.md` §10, and risk records cite the audit
trail. Splitting them off would orphan citations or land code
without its rationale. They are non-executable, additive, and
scoped under `06-locate*` filenames, so they cannot collide with
sibling Initiative 06 PRs. Bundling them with the feature PR is
the design intent of the workflow.

### 3. `session` parent + `locate` first child

No empty-parent risk. `src-tauri/src/main.rs` adds the `Session`
variant and its `SessionSubcommands::Locate` child in the same
commit (b88097e). Clap's derive forbids an empty `#[command(subcommand)]`
at runtime, but more importantly the branch never lands `session`
without a child. Sibling subcommands (`export`, `import-replace`,
`pause-handshake`, `schema-probe`) are explicitly anti-scoped in
proposal §1.

### 4. README updates (new in this re-run)

Correctly bundled. The added "Locating a Session" section and the
Subcommands listing document the exact CLI surface this PR
introduces; the SQL-fallback paragraph reframes existing prose
around the new command. Landing the surface without docs would
create an undocumented public command; landing docs separately
would describe nonexistent behavior. Proposal §10 makes README an
explicit deliverable of this PR. The Rev 1 R1-F09 framing of
`mutable` as a read-time eligibility hint is honored verbatim in
the new README copy.

## Why not MULTI_CONCERN_ACCEPTABLE

That verdict would acknowledge multiple concerns and justify
keeping them together. Here there is genuinely one concern: a
single new command + its supporting library API + colocated
artifacts. The TranscriptState relocation is a sub-step of the
feature, not a parallel concern.

## Recommended action

Proceed to the next gate. No split, no carve-out.
