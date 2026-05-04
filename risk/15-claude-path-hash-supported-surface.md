# WU-14-02 — Supported-Surface Risk Gate (Phase 4)

Inputs reviewed:

- `proposals/15-claude-path-hash.md`
- `research/15-claude-path-hash-problem-map.md`
- `research/15-claude-path-hash-rca.md`
- `research/14-problem-map.md` (broader resume flow context)
- `tickets/phase-14:plans/tickets/phase-14/WU-14-02.md`

## 1. Termination evaluation

The two termination signals in the supported-surface termination rule are
evaluated independently of the LOW/MEDIUM/HIGH verdict.

### 1a. Invalidated assumption that breaks the current problem framing

**Does not fire.**

Each load-bearing assumption in the proposal's assumption register is
either supported by RCA evidence or congruent with it:

- **A1 — anthropics/claude-code#19972 encoder rule.** The RCA reproduces
  the encoder verbatim (`research/15-claude-path-hash-rca.md:31`–`:37`),
  records the reverse-engineering provenance (japsoon 2026-01-21,
  corroborated by sindrijo 2026-02-12 with collision examples,
  `research/15-claude-path-hash-rca.md:39`–`:40`), and contains no
  observed counter-evidence. The proposal's invalidator framing — "a
  real-Claude probe showing a different encoding rule"
  (`proposals/15-claude-path-hash.md:243`) — is consistent with the RCA.
- **A2 — `std::fs::canonicalize` matches Claude's symlink resolution.**
  The real-Claude symlink probe in the RCA explicitly *supports* the
  canonicalize-before-hash plan rather than contradicting it: Probe A
  (literal symlink path) returned "No conversation found", and Probe B
  (resolved symlink target path) succeeded
  (`research/15-claude-path-hash-rca.md:45`–`:56`). The proposal's plan
  to canonicalize then hash is the direct corollary of that probe.
- **A3 — `to_string_lossy()` is acceptable encoder input.** Aligns with
  the ticket's Phase 2.5+ Notes on non-UTF8 bytes already encoding to
  `-` under the filter.
- **A4 — stderr warning is sufficient observability for canonicalize
  fallback.** Consistent with the existing
  `stderr: &mut dyn Write` already threaded through
  `migrate_chain_segment` (`proposals/15-claude-path-hash.md:128`–`:130`).

The orchestrator brief explicitly notes that the WU-14-01 D-010/D-011
revisit conditions are met by the RCA artifact — community
reverse-engineering plus corroboration plus a real-Claude symlink probe
— and that this gate must NOT terminate on "insufficient evidence for
Windows hashing" or "insufficient evidence for symlink behavior". No
RCA finding contradicts the canonicalize plan; the symlink probe
*requires* it.

### 1b. Non-positive value on the current supported surface

**Does not fire.**

The named current-state risk is concrete: migrated JSONL is currently
written under a directory name that the child `claude --resume` will
not search for three input shapes — paths containing filtered
characters (`_`, `.`, CJK, accented latin, etc.), Windows-shaped paths
(backslashes, drive punctuation), and symlinked workspaces. That risk
sits on the active supported surface (the migration writer at
`src-tauri/src/migration/mod.rs:188`) reached from the supported
`run_repl` and `run_resume` paths in `src-tauri/src/main.rs`
(`research/15-claude-path-hash-problem-map.md:64`–`:69`).

The proposal eliminates that risk by replacing the encoder with the
authoritative #19972 rule plus canonicalize-then-hash. Reduction is
deterministic and load-bearing for the migration → child-resume
handoff. No counter-vailing customer-visible regression is introduced
on legacy alphanumeric Unix paths: the proposal verifies that
`/home/nes/x → -home-nes-x` is byte-stable under the new rule
(`proposals/15-claude-path-hash.md:434`–`:435`), and the problem map
confirms via `rg` that the only production caller of
`claude_project_dir_for` is the migration writer
(`research/15-claude-path-hash-problem-map.md:24`).

Net value on the current supported surface is positive.

## 2. Verdict

The proposal targets the actual supported-surface risk, the assumption
register covers the load-bearing claims, and the adjacent-paths
analysis is correct. Reasoning by gate criterion:

- **Targets the actual supported surface.** The single production
  caller is the migration writer at
  `src-tauri/src/migration/mod.rs:161`–`:188`
  (`research/15-claude-path-hash-problem-map.md:7`–`:8`,`:24`); the
  proposal's design points exactly there
  (`proposals/15-claude-path-hash.md:67`–`:75`). The
  encoder rewrite is one function (`claude_project_dir_for`), not a
  cross-module refactor.

- **Adjacent surfaces correctly identified as no-change.** Both supported
  resume entry points (`run_repl` near `main.rs:1622` and `run_resume`
  near `main.rs:1847`) feed cwd into the same migration writer; nothing
  about argv composition, provider selection, or
  `MigratedSegment.target_jsonl_path` contract changes
  (`proposals/15-claude-path-hash.md:188`–`:212`). The problem map
  confirms this enumeration
  (`research/15-claude-path-hash-problem-map.md:64`–`:77`).

- **Load-bearing test mirror correctly carried into the Code Boundary.**
  The problem map's section 3.1 establishes that
  `tests/session_migration_rca/mod.rs` is a *load-bearing* test
  dependency — its `claude_project_dir_name` helper (`mod.rs:129`–`:130`)
  and the fake-Claude `${PWD//\//-}` Bash snippet (`mod.rs:109`–`:115`)
  are encoder mirrors that diverge once the production encoder changes,
  because the WU-14-01 fixture's tempdir paths contain `.`
  (`research/15-claude-path-hash-problem-map.md:53`–`:59`). The proposal
  picks up exactly those two loci and bounds the rewrite to keep AC-4
  and the WU-14-01 RC-1 contract green
  (`proposals/15-claude-path-hash.md:36`–`:41`,`:142`–`:155`).
  Other slash-only fixture helpers are correctly held out of scope on
  the basis that their assertions don't depend on the new rule's
  output.

- **Migration / rollback are sound.** Code-only change, next-release
  deploy, revert-one-commit rollback, no DB or file rewrite. Rollback
  explicitly restores the old placement risk for *future* migrations
  only — a correct framing that doesn't pretend the change is
  retroactive (`proposals/15-claude-path-hash.md:215`–`:226`).

- **Observability is adequate.** The fallback warning is specified
  exactly, including format string, channel (existing
  `stderr: &mut dyn Write`), and trigger condition. Tests are
  capturable through the existing `Vec<u8>` stderr sink pattern
  (`proposals/15-claude-path-hash.md:128`–`:139`,`:344`–`:351`).

- **Assumption register covers the load-bearing claims.** A1 (encoder
  rule), A2 (symlink canonicalization), A3 (string-input shape), and
  A4 (stderr observability) each have an evidence line and an
  invalidator line. No load-bearing claim is missing — the
  decode-side adjacency is explicitly noted as out-of-band for the
  writer path (`proposals/15-claude-path-hash.md:206`–`:208`,
  `research/15-claude-path-hash-problem-map.md:49`–`:50`).

- **Adjacent-paths blast radius is bounded and addressed.** The two
  blast-radius watchpoints — inline migration tests and the WU-14-01
  RCA harness — are both addressed. Inline tests are extended (the
  existing `claude_project_dir_for_encodes_absolute_unix_path` is
  preserved and strengthened to exercise filtered characters). The
  WU-14-01 harness gets the two scoped encoder-mirror updates
  spelled out at file-and-line precision. Legacy alphanumeric Unix
  paths are byte-stable.

A residual is correctly named (RC-2 has no live Windows Claude
probe) and routed to `risk/15-test-residuals.md` rather than blocking
this gate (`proposals/15-claude-path-hash.md:447`–`:453`). That is
the right disposition under the gate's "do not terminate on
insufficient Windows / symlink evidence" rule.

## 3. Net-value summary

The proposal reduces a real current-state risk on the named
supported surface — migrated JSONL landing at a directory the child
`claude --resume` will not search for filtered-char, Windows-shape,
or symlinked-workspace inputs — by replacing the slash-only encoder
with the authoritative #19972 rule plus canonicalize-then-hash. The
reduction is worth the blast radius: the rewrite is one production
function plus two scoped test-mirror updates, legacy alphanumeric
Unix paths are byte-stable, migration is next-release-deploy,
rollback is revert-one-commit, and the observability story (existing
`[migrate]` line plus a precisely-shaped fallback warning) is
adequate without new dependencies.

Verdict: LOW
