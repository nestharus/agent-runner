# Phase 4 R4 Risk Gate — Scope (WU-14-02 / claude-path-hash)

- Round: R4 (post-amendment re-run)
- Inputs: `proposals/15-claude-path-hash.md`,
  `tickets/phase-14:plans/tickets/phase-14/WU-14-02.md`,
  `research/15-claude-path-hash-problem-map.md`,
  `tmp/scratch/wu-14-02/questions/phase-3-r3-ticket-scope-contradiction.answer.md`
- Mandate: evaluate the proposal against the **amended** ticket
  Anti-scope (literal Anti-scope + the root-approved two-locus
  expansion in `src-tauri/tests/session_migration_rca/mod.rs`).

## 1. Amended Anti-scope baseline

The R3 scope MEDIUM was driven by a literal Anti-scope text that
forbade *any* edit inside `tests/session_migration_rca/`. The root
resolved that contradiction at
`tmp/scratch/wu-14-02/questions/phase-3-r3-ticket-scope-contradiction.answer.md`
by approving Option A: a narrow two-locus expansion that permits
exactly two updates inside `src-tauri/tests/session_migration_rca/mod.rs`
and nothing else:

1. The `claude_project_dir_name` Rust helper at `mod.rs:129-130`.
2. The fake-Claude Bash lookup script at `mod.rs:109-115`
   (`project="${PWD//\//-}"`).

Everything else in `tests/session_migration_rca/` — including
`rc1_cwd_project_dir_mismatch.rs` (the WU-14-01 RC-1 test body),
`MigrationFixture`, JSONL seeders, child-Claude launch wiring, and
all assertion semantics — remains off-limits. The other named
adjacent slash-only helpers in `tests/fixtures/initiative_06*.rs`,
`tests/initiative_05_migration.rs`, and
`tests/pr_f_resume_integration.rs` also remain out of scope.

This is the baseline the R4 scope gate evaluates against.

## 2. Proposal Anti-scope vs. amended ticket Anti-scope

### 2.1 The two-locus expansion is named with file:line refs

`proposals/15-claude-path-hash.md` §2 names both loci:

> the only updates inside this directory are the two encoder mirrors
> that this WU is changing in production:
> `tests/session_migration_rca/mod.rs::claude_project_dir_name` gets
> a one-function rewrite to apply the same encoder rule the
> production code now applies, and the fake-Claude Bash
> `${PWD//\//-}` snippet at
> `tests/session_migration_rca/mod.rs:109-115` is rewritten to apply
> the same rule.

§3 (Design) reiterates and bounds both updates:

- Helper: "replace `/` and `\` with `-`, then filter to ASCII
  alphanumeric plus `-`, replacing every other character with `-`."
- Bash snippet at `mod.rs:109-115`: factored through a per-test Bash
  helper running
  `printf '%s' "$1" | sed -e 's#[/\\]#-#g' -e 's/[^A-Za-z0-9-]/-/g'`,
  called with `$PWD`.

Coverage check vs. the amendment:

- Locus 1 (`claude_project_dir_name`, `mod.rs:129-130`) — named by
  function name and file. The amendment cites lines 129-130; the
  proposal does not include the literal `:129-130` suffix in §2 but
  the function name is unique inside the file and matches the
  amendment exactly. No drift.
- Locus 2 (fake-Claude Bash snippet, `mod.rs:109-115`) — named with
  the literal line range `109-115` in both §2 and §3. Exact match.

### 2.2 RC-1 test body explicitly carried forward

Proposal §2 carries forward the WU-14-01 RC-1 contract:

> Do NOT alter the contract or test logic of the WU-14-01 RC-1
> reproduction (`src-tauri/tests/session_migration_rca/`). The
> test's assertions, fixtures, and assertion semantics stay
> unchanged.

This covers `rc1_cwd_project_dir_mismatch.rs` body, the
`MigrationFixture` constructor (`mod.rs:23-39`), the JSONL seeder,
and the child-Claude launch wiring — every part of the harness
*except* the two approved loci. The amendment's "Forward-compatible
note" in the answer artifact specifically warns Phase 8 to flag
"touching the rc1 test body itself" as a real Anti-scope violation;
this proposal does not propose any such touch.

The WU-14-02 RC-1 cross-reference at proposal §6
("`src-tauri/tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs`")
is the new WU-14-02 RCA harness, not the WU-14-01 RC-1 file. No
naming collision; both are in their respective test boundaries.

### 2.3 Other adjacent slash-only helpers stay out of scope

Proposal §2 names the out-of-scope helpers explicitly with line
ranges:

> Out-of-scope helpers are `tests/fixtures/initiative_06.rs:886-888`,
> `tests/fixtures/initiative_06_import_replace.rs:995-997`,
> `tests/fixtures/initiative_06_export.rs:605-607`,
> `tests/initiative_05_migration.rs:636-638` and its assertions, and
> `tests/pr_f_resume_integration.rs:949-959`.

This matches the problem map §3.1 enumeration verbatim.

### 2.4 No other scope inside `tests/session_migration_rca/`

Scanning the proposal end-to-end for any edit to
`tests/session_migration_rca/` beyond the two approved loci:

- §3 (Design) — only mentions the two loci.
- §6 (Test-Intent Track) AC-5 — explicitly states the WU-14-01
  harness "stays green AFTER its encoder mirrors
  (`mod.rs`'s `claude_project_dir_name` and the fake-Claude
  `${PWD//\//-}` snippet) are updated to the new rule" — same two
  loci, no broader edit.
- §6 RC-1 fixture line cites WU-14-02's
  `claude_path_hash_rca/rc1_non_alnum_encoding.rs`, not the
  WU-14-01 directory.
- No new test files are proposed inside `tests/session_migration_rca/`.
- No signature changes to the fake-Claude script beyond rewriting
  the `${PWD//\//-}` substitution into the helper-driven encoder.
- §3 explicitly says: "No symlink canonicalization is added to the
  fake-Claude Bash. The WU-14-01 RC-1 test does not exercise
  symlinks; this helper update is character-filter-only." — this
  prevents scope drift via incidental fake-Claude enhancement.

## 3. Code Boundary coverage

The ticket's in-scope Code Boundary items, mapped to the proposal:

| Code Boundary item | Proposal coverage |
|---|---|
| `src-tauri/src/migration/mod.rs::claude_project_dir_for` rewrite | §3 Design: encoder pseudocode (canonicalize-then-string-rule), production call site at `mod.rs:161`. |
| `src-tauri/src/migration/mod.rs::tests::claude_project_dir_for_*` updates | §6 AC-4: existing inline tests updated to assert the full rule, including `claude_project_dir_for_encodes_absolute_unix_path`. |
| `src-tauri/src/migration/mod.rs::MigrationError::SpawnCwdUnsupported` trigger | §3 "SpawnCwdUnsupported posture": variant kept, trigger narrowed to empty cwd, `is_absolute()` rejection removed. |
| `DECISIONS.md` updates (D-010, D-011, D-NN) | §6 AC-7: marked resolved with dated lines; Phase 2.5 human-gate skip recorded as new D-NN. |
| `risk/14-test-residuals.md` updates | §6 AC-8: Windows hashing and symlink/canonicalization residuals marked resolved with pointers to harnesses + PR. |

The Test Boundary items (the three new
`tests/claude_path_hash_rca/rc{1,2,3}_*.rs` harnesses) are addressed
in §6 RC-1, RC-2, RC-3, with each flipping RED → GREEN under the
post-fix encoder. The two approved encoder mirrors in
`tests/session_migration_rca/mod.rs` are the only WU-14-01-area
edits.

## 4. AC coverage

Every AC has a concrete plan in §6:

- AC-1 (RC-1 non-alnum filtering): full filter rule + RCA harness flip.
- AC-2 (RC-2 Windows-shape acceptance): `is_absolute()` lifted;
  `SpawnCwdUnsupported` triggered only on empty cwd; RCA harness flip.
- AC-3 (RC-3 symlink canonicalization): `std::fs::canonicalize`
  before hashing; warning + literal fallback on failure.
- AC-4 (inline migration tests stay green): inline tests updated to
  the full rule; `claude_project_dir_for_encodes_absolute_unix_path`
  is strengthened, not removed.
- AC-5 (prior RCA harnesses stay green): WU-14-01
  `session_migration_rca/` stays green AFTER the two scoped encoder
  mirrors are updated; named adjacent harnesses
  (`routing_fanout_rca/`, `empty_bodies_ref_rca/`,
  `release_yml_contract.rs`, `session_lock_cross_platform.rs`) stay
  green with no fixture edits.
- AC-6 (cargo + frontend gates): `cargo fmt`, `cargo clippy -D
  warnings`, `cargo test --no-fail-fast`, plus frontend gates.
- AC-7 (`DECISIONS.md` updates): D-010 and D-011 marked resolved
  with dates; new D-NN appended for Phase 2.5 human-gate skip.
- AC-8 (`risk/14-test-residuals.md` cleanup): residuals marked
  resolved without erasing historical context.

## 5. Scope-creep signals beyond the approved expansion

Searched explicitly for the disallowed signals from the rubric:

- Changes to `rc1_cwd_project_dir_mismatch.rs`: **none proposed**.
  §2's RC-1 carry-forward language explicitly preserves the test
  body; §3 confines edits to the helper + Bash snippet in `mod.rs`.
- New test files in `tests/session_migration_rca/`: **none proposed**.
- Signature changes to the fake-Claude script beyond rewriting the
  `${PWD//\//-}` snippet: **none proposed**. §3 says the helper
  approach is "character-filter-only" and explicitly excludes
  symlink canonicalization.
- Edits to other adjacent slash-only helpers
  (`initiative_06*`, `initiative_05_migration.rs`,
  `pr_f_resume_integration.rs`): **explicitly forbidden** in §2
  with line-precise refs.
- Platform-specific code (`#[cfg(target_os)]`): forbidden in §2;
  §3's encoder is platform-neutral string processing.
- Unrelated `MigrationError` changes: forbidden in §2; §3 narrows
  only the `SpawnCwdUnsupported` trigger.
- Backwards-compat / feature flag for the old encoder: forbidden in
  §2 per `~/ai/conventions/no-backwards-compatibility.md`.
- Bulk rewrite of already-migrated JSONL: forbidden in §2; §4
  Migration path confirms only future migrations get the corrected
  placement.

The Net-Value statement (§7) explicitly characterizes the two-locus
expansion as "mechanical: both edits mirror the production encoder
rule, preserve the WU-14-01 test contract, and do not change this
WU's net-value case" — language consistent with the amendment's
intent.

## 6. Minor observation (not a verdict driver)

In §2 the proposal references the Rust helper by function name
(`tests/session_migration_rca/mod.rs::claude_project_dir_name`)
without the literal `:129-130` suffix. The amendment used
`mod.rs:129-130`. Because the function name is unique in the file
and §3 reinforces the locus, this is unambiguous. Phase 8 audit
should still spot-check that the only Rust edit inside that file
sits at lines 129-130; no further Phase 4 action is warranted.

## 7. R3 → R4 transition

The R3 scope verdict was MEDIUM with the explicit caveat that "the
merits are right but the literal Anti-scope text is exceeded." The
root's amendment (Option A) updates the Anti-scope to permit those
merits. With the amended baseline, the deviation that drove R3
MEDIUM no longer exists. The R3 scope MEDIUM retires on this
re-run.

## Verdict: LOW
