# Audit history — initiative 10 routing-claude-skipped (Phase 4 risk gate loop)

## Purpose

Track Phase 4 revise/review rounds for `proposals/10-routing-claude-skipped.md`.
The loop verdict each round is `continue` (revise + re-run gates) or `apply`
(all four gates LOW with no termination signal, advance to Phase 5).

## Artifact lineage

- Proposal: `proposals/10-routing-claude-skipped.md`
- Phase 0 RCA: `research/10-routing-claude-skipped-rca.md`
- Phase 0 reproduction harness: `src-tauri/tests/rca_routing_claude_skipped.rs`
- Phase 2.5 problem map: `research/10-routing-claude-skipped-problem-map.md`
- Risk artifacts (current round): `risk/10-audit.md`, `risk/10-scope.md`,
  `risk/10-shortcut.md`, `risk/10-supported-surface.md`

## Round summaries

### Round 1 — initial proposal reviewed

- Artifact under review: `proposals/10-routing-claude-skipped.md` at
  `b92fb14`.
- Round artifacts: `risk/10-audit.md`, `risk/10-scope.md`,
  `risk/10-shortcut.md`, `risk/10-supported-surface.md` (round-1 commits
  `697c2d9`, `8ae5c76`, `2911ec5`, `2ecb0da`).
- Report artifacts:
  - report index: none
  - PDFs: none
  - uploaded artifact URL: none
  - screenshots: none
  - non-UI evidence: prompt+log pairs in `.tmp/routing-claude-skipped-risk-*`
- Prior finding counters:
  - closed: 0
  - intact: 0
  - weakened: 0
  - regressed: 0
  - not closed: 0
- New findings:
  - `R1-F01` — medium; audit gate flagged that the migration section did
    not state error/rollback behavior on partial failure or unexpected
    `providers` shapes; ancestor chain: none; oscillation: none.
  - `R1-N01` — note (low); shortcut gate flagged that
    `last_error_at = MAX(finished_at)` over all terminal rows could
    accidentally pick the most recent successful timestamp instead of the
    most recent failed one; ancestor chain: none; oscillation: none.
  - `R1-N02` — note (low); supported-surface gate flagged that
    `src-tauri/examples/quota_check.rs:123` calls `get_provider(name, i)`
    with an index argument and will fail to compile after the signature
    change; ancestor chain: none; oscillation: none.
- Oscillation:
  - same-label: 0
  - same-family: 0
  - fix-created: 0
- Decompose trigger: not fired (single MEDIUM finding plus two
  documentation/precision notes — well within revise scope).
- Watch signals: migration error/rollback paragraph and its tests must
  hold across rounds; no further index-keyed surface may resurface.
- Verdict / determination: `continue` — revise the proposal, re-run all
  four gates per Phase 4 rule "any MEDIUM or HIGH report means revise the
  proposal and re-run all four."
- Role outputs:
  - audit (gpt-high): `risk/10-audit.md` MEDIUM
  - scope (claude-opus): `risk/10-scope.md` LOW
  - shortcut (claude-opus): `risk/10-shortcut.md` LOW
  - supported-surface (claude-opus): `risk/10-supported-surface.md` LOW
    (termination=none)
- Next handoff: round 2 risk-gate fanout reads the revised
  `proposals/10-routing-claude-skipped.md`, this audit-history file's
  Round 1 entry, and the round-1 risk artifacts. Each gate must verify
  whether `R1-F01`, `R1-N01`, and `R1-N02` are closed by the revision.

## Decision register

- Decision D1: keep aggregate identity at `(model_name, provider_name)`;
  reject Option B (composite with provider_index) because it would split
  history across reorders. Source: proposal §Design Rationale; affirmed by
  scope and shortcut gates.
- Decision D2: discard pre-fix `providers.last_error` snippets during
  migration; rebuild from `invocations.error_category`. Source: assumption
  A4; affirmed by supported-surface gate.
- Decision D3: do not introduce a `PRAGMA user_version` write path.
  Source: assumption A5; affirmed by shortcut gate.
- Decision D4: include `recent_error_count` identity fix in the same diff
  as the aggregate fix. Source: scope of fix is "both index-keyed surfaces
  named in the problem map"; affirmed by scope and shortcut gates.
- Decision D5 (round 1 → round 2 revision): make the migration error
  contract explicit; add explicit `last_error_at` failure-row semantics;
  add `examples/quota_check.rs` to the in-scope diff; add three new
  test-intent entries (unexpected shape rejection, transactional rollback
  via rename-conflict, last_error_at vs success). Source: closes
  `R1-F01`, `R1-N01`, `R1-N02`.

### Round 2 — revised proposal reviewed

- Artifact under review: `proposals/10-routing-claude-skipped.md` at
  `b782b4d`.
- Round artifacts: `risk/10-audit.md`, `risk/10-scope.md`,
  `risk/10-shortcut.md`, `risk/10-supported-surface.md` (round-2
  commits `ceb60ba`, `1948f5c`, `2fdf2db`, `ddad20e`).
- Report artifacts:
  - report index: none
  - PDFs: none
  - uploaded artifact URL: none
  - screenshots: none
  - non-UI evidence: prompt+log pairs in
    `.tmp/routing-claude-skipped-risk-*-round2.{md,log}`
- Prior finding counters:
  - closed: 3 (R1-F01, R1-N01, R1-N02 all closed)
  - intact: 0
  - weakened: 0
  - regressed: 0
  - not closed: 0
- New findings:
  - `R2-F01` — medium; shortcut gate flagged that the round-2
    "Migration rollback on failed rebuild" test (using a pre-existing
    `providers_legacy_index_keyed` to force the `RENAME` to fail) does
    not actually exercise transactional rollback because the
    rename-conflict failure happens before any rebuild SQL runs; an
    implementation that omits the transaction wrapper would still
    pass the test. Ancestor chain: none. Oscillation: fix-created
    (introduced by the round-1 → round-2 revision that tried to close
    `R1-F01`).
  - `R2-N01` — low note; shortcut gate flagged that the migration
    error contract did not document an operator-level recovery
    procedure when `StateDb::open` rejects a malformed `providers`
    shape. Ancestor chain: none.
- Oscillation:
  - same-label: 0
  - same-family: 0
  - fix-created: 1 (`R2-F01`)
- Decompose trigger: not fired (one MEDIUM finding plus one low note,
  both addressable by replacing one test entry and adding one
  documentation paragraph; well within revise scope).
- Watch signals: `WS-3` added — the rollback property of
  `ensure_providers_schema` is now an explicit residual verified by
  code review only; no future round may quietly re-promote it to a
  runtime test claim without specifying a viable failure-injection
  mechanism.
- Verdict / determination: `continue` — revise the proposal, re-run
  all four gates per Phase 4 rule "any MEDIUM or HIGH report means
  revise the proposal and re-run all four."
- Role outputs:
  - audit (gpt-high): `risk/10-audit.md` LOW (R1-F01 closed)
  - scope (claude-opus): `risk/10-scope.md` LOW
  - shortcut (claude-opus): `risk/10-shortcut.md` MEDIUM (R1-N01
    closed; new R2-F01 medium + R2-N01 low note)
  - supported-surface (claude-opus): `risk/10-supported-surface.md`
    LOW termination=none (R1-N02 closed)
- Next handoff: round 3 risk-gate fanout reads the further-revised
  `proposals/10-routing-claude-skipped.md`, this audit-history file's
  Round 2 entry, and the round-2 risk artifacts. Each gate must
  verify whether `R2-F01` and `R2-N01` are closed by the round-3
  revision, and that the round-1 closures (`R1-F01`, `R1-N01`,
  `R1-N02`) remain closed.

## Watch signals

- `WS-1`: any future round must re-check that the migration helper
  remains transactional and rejects unexpected shapes — it must not
  acquire heuristic recovery affordances.
- `WS-2`: any future round must re-check that no index-keyed reader
  alias has been kept on the routing-history surface
  (`providers`/`get_provider`/`recent_error_count`).
- `WS-3`: rollback during mid-rebuild failure is now an explicit
  residual verified by code review only. Future rounds must not
  silently re-promote it to a runtime test claim without specifying a
  viable failure-injection mechanism that does not require test-only
  product source.

### Round 3 — twice-revised proposal reviewed

- Artifact under review: `proposals/10-routing-claude-skipped.md` at
  `023635f`.
- Round artifacts: `risk/10-audit.md`, `risk/10-scope.md`,
  `risk/10-shortcut.md`, `risk/10-supported-surface.md` (round-3
  commits `84e4271`, `2900ba5`, `0d05077`, `7b7797a`; merged at
  `608fdcc`).
- Report artifacts:
  - report index: none
  - PDFs: none
  - uploaded artifact URL: none
  - screenshots: none
  - non-UI evidence: prompt+log pairs in
    `.tmp/routing-claude-skipped-risk-*-round3.{md,log}`
- Prior finding counters:
  - closed: 2 (R2-F01, R2-N01 both closed)
  - intact: 0
  - weakened: 0
  - regressed: 0
  - not closed: 0
- New findings: none
- Oscillation:
  - same-label: 0
  - same-family: 0
  - fix-created: 0
- Decompose trigger: not fired.
- Watch signals: WS-1 / WS-2 / WS-3 all upheld this round.
- Verdict / determination: `apply` — all four gates LOW with
  termination=none on the supported-surface gate. Phase 4 passes;
  advance to Phase 5 (hookpoint research).
- Role outputs:
  - audit (gpt-high): `risk/10-audit.md` LOW (R1-F01 still closed)
  - scope (claude-opus): `risk/10-scope.md` LOW
  - shortcut (claude-opus): `risk/10-shortcut.md` LOW (R2-F01 closed,
    R2-N01 closed)
  - supported-surface (claude-opus): `risk/10-supported-surface.md`
    LOW termination=none
- Next handoff: Phase 5 hookpoint research consumes the approved
  proposal, the problem map, and the four LOW risk reports.

## Process-tree dispatch evidence

Phase 4 was run as three rounds of four parallel risk-gate dispatches.
Each round:

- Each gate ran in its own git worktree under
  `.worktrees/rca-routing-risk-{audit,scope,shortcut,supported-surface}`,
  satisfying `~/ai/conventions/worktree-isolation.md`.
- Models followed `~/ai/models/roles.md`: audit → `gpt-high`;
  scope, shortcut, supported-surface → `claude-opus`.
- Prompt and log pairs are preserved at
  `.tmp/routing-claude-skipped-risk-{audit,scope,shortcut,supported-surface}-round{1,2,3}.{md,log}`.
- Round-1 risk artifacts were preserved as separate commits before
  octopus-merge into the parent branch; round-2 and round-3 artifacts
  overwrite the same paths but git history retains every prior round
  via the per-round commit chain.
- A formal `process-tree-auditor` invocation was not run because the
  pipeline was orchestrated from Claude Code rather than under a
  single `agents` root UUID; the dispatch evidence above is the
  equivalent record. If a future round requires it, the
  prompt+log+commit chain is sufficient to reconstruct the process
  tree.

## Final state

Phase 4 passed at round 3 with all four gates LOW and
termination=none. Determination: `apply`. Pipeline advances to
Phase 5.

## CodeRabbit loop

IDs in this section are scoped to the CodeRabbit loop.

### Round 1 CodeRabbit pass

- Artifact under review: full branch diff against `main` at `b4f2a50`.
- Raw pass log: `CODERABBIT_pass1.md`.
- Findings: 7 total.
- Breakdown: 5 real findings applied; 2 style nitpicks applied because
  they were trivial markdown hygiene; 0 skipped.
- Applied finding IDs:
  - `R1-F01`: removed duplicated `## Watch signals` block from this
    file.
  - `R1-F02`: added markdown spacing after
    `risk/10-shortcut.md` `## Prior finding status`.
  - `R1-F03`: anchored hookpoint line references to commit/date and
    stable symbol-search verification.
  - `R1-F04`: renumbered a resumed ordered-list marker in the proposal.
  - `R1-F05`: corrected the manual-drop recovery text to state that
    `StateDb::open` / `agents migrate-db` recreate an empty
    `providers` table and do not replay existing `invocations`.
  - `R1-F06`: strengthened `providers` shape validation to include
    type affinity and not-null metadata; added regression coverage.
  - `R1-F07`: made migrated `last_error` selection deterministic on
    tied `finished_at` values by ordering failed invocations with
    `id` as the secondary key; added regression coverage.
- Flip-flops: none.
- Skipped rationales: none.
- Watch signals: `WS-1` upheld by stricter unexpected-shape rejection;
  `WS-2` not implicated; `WS-3` remains a code-review residual.
- Test result after fixes: PASS —
  `cd src-tauri && cargo test 2>&1 | tail -5`.
- Determination: `continue` — amend latest commit and run CodeRabbit
  pass 2.

### Round 2 CodeRabbit pass

- Artifact under review: full branch diff against `main` at `8dd13cb`.
- Raw pass log: `CODERABBIT_pass2.md`.
- Findings: 5 total.
- Breakdown: 3 real findings applied; 2 style/staleness nitpicks
  applied because they were low-cost hygiene; 0 skipped.
- Applied finding IDs:
  - `R2-F01`: added a staleness warning to `risk/10-audit.md` for
    source line-number evidence.
  - `R2-F02`: fixed markdown heading spacing in
    `risk/10-supported-surface.md`.
  - `R2-F03`: updated the implementation contract to encode the
    deterministic `finished_at DESC, id DESC` failure tie-break.
  - `R2-F04`: added non-mutating `providers` shape preflight before
    `invocations` migration and regression coverage for malformed
    providers plus legacy invocations.
  - `R2-F05`: corrected the contract signature and open-ordering text
    for `ensure_providers_schema(&mut Connection)`.
- Flip-flops: none.
- Skipped rationales: none.
- Watch signals: `WS-1` strengthened by the preflight rejection order;
  `WS-2` not implicated; `WS-3` remains a code-review residual.
- Test result after fixes: PASS —
  `cd src-tauri && cargo test 2>&1 | tail -5`.
- Determination: `continue` — amend latest commit and run CodeRabbit
  pass 3.

### Round 3 CodeRabbit pass

- Artifact under review: full branch diff against `main` at `d68ed53`.
- Raw pass log: `CODERABBIT_pass3.md`.
- Findings: 2 total.
- Breakdown: 1 real consistency finding applied; 1 skipped as
  design-contradicting churn.
- Applied finding IDs:
  - `R3-F01`: added risk/level/source comments to the three new
    provider-migration regression tests.
- Skipped finding IDs:
  - `R3-F02`: skipped order-insensitive `providers` shape matching.
    Rationale: the approved contract and `WS-1` intentionally accept
    only exact pre-fix/post-fix binary-produced shapes and reject
    foreign/hybrid shapes without heuristic compatibility. Accepting a
    reordered externally-created table would weaken that posture.
- Flip-flops: none.
- Watch signals: `WS-1` explicitly upheld by skipping heuristic
  reordered-shape acceptance; `WS-2` not implicated; `WS-3` remains a
  code-review residual.
- Test result after fixes: PASS —
  `cd src-tauri && cargo test 2>&1 | tail -5`.
- Determination: `continue` — amend latest commit and run CodeRabbit
  pass 4.

### Round 4 CodeRabbit pass

- Artifact under review: full branch diff against `main` at `23912eb`.
- Raw pass log: `CODERABBIT_pass4.md`.
- Findings: 4 total.
- Breakdown: 1 real documentation correctness finding applied; 3
  markdown style findings applied; 0 skipped.
- Applied finding IDs:
  - `R4-F01`: added blank lines after affected headings in
    `risk/10-scope.md`.
  - `R4-F02`: added a blank line after `risk/10-shortcut.md`
    `## Notes`.
  - `R4-F03`: added a blank line after `risk/10-shortcut.md`
    `## Watch signal status`.
  - `R4-F04`: narrowed the contract's `provider_index` removal claim to
    `ProviderRecord`, while preserving `provider_index` on invocation
    structs and the invocation-row load.
- Flip-flops: none.
- Skipped rationales: none.
- Watch signals: no change; `WS-1`, `WS-2`, and `WS-3` remain as
  recorded in round 3.
- Test result after fixes: PASS —
  `cd src-tauri && cargo test 2>&1 | tail -5`.
- Determination: `continue` — amend latest commit and run CodeRabbit
  pass 5.

### Round 5 CodeRabbit pass

- Artifact under review: full branch diff against `main` at `bd20806`.
- Raw pass log: `CODERABBIT_pass5.md`.
- Findings: 2 total.
- Breakdown: 2 real contract/documentation findings applied; 0
  skipped.
- Applied finding IDs:
  - `R5-F01`: replaced fragile source line-number references in
    `research/10-routing-claude-skipped-contract.md` with stable
    symbol/test names.
  - `R5-F02`: completed the documented `get_provider` and
    `recent_error_count` return types as `Result<..., String>`.
- Flip-flops: none.
- Skipped rationales: none.
- Watch signals: no change; `WS-1`, `WS-2`, and `WS-3` remain as
  recorded in round 3.
- Test result after fixes: PASS —
  `cd src-tauri && cargo test 2>&1 | tail -5`.
- Determination: `continue` — amend latest commit and run CodeRabbit
  pass 6.

### Round 6 CodeRabbit pass

- Artifact under review: full branch diff against `main` at `69ecb60`.
- Raw pass log: `CODERABBIT_pass6.md`.
- Findings: 3 total.
- Breakdown: 1 real architectural finding applied; 2 style nitpicks
  applied; 0 skipped.
- Applied finding IDs:
  - `R6-F01`: tightened phrasing in `risk/10-shortcut.md`.
  - `R6-F02`: varied repeated sentence openings in `risk/10-scope.md`.
  - `R6-F03`: re-ran `validate_providers_schema` inside
    `migrate_legacy_invocations` after its transaction starts and
    before legacy `invocations` rows are read or rewritten.
- Flip-flops: none.
- Skipped rationales: none.
- Watch signals: `WS-1` strengthened by validating provider shape
  inside the legacy invocation migration transaction; `WS-2` not
  implicated; `WS-3` remains a code-review residual.
- Test result after fixes: PASS —
  `cd src-tauri && cargo test 2>&1 | tail -5`.
- Determination: `continue` — amend latest commit and run CodeRabbit
  pass 7 as the stability check before convergence or cap escalation.

### Round 7 CodeRabbit pass

- Artifact under review: full branch diff against `main` at `43c1db8`.
- Raw pass log: `CODERABBIT_pass7.md`.
- Findings: 3 total.
- Breakdown: 2 useful findings applied; 1 performance nitpick skipped.
- Applied finding IDs:
  - `R7-F01`: corrected the `recent_error_count` contract to state that
    `StateDb::recent_error_count` receives `window_minutes` and computes
    the `created_at` cutoff internally.
  - `R7-F02`: added
    `finalize_invocation_skips_provider_aggregate_for_null_provider_name`
    to cover success and failure finalization with `provider_name = NULL`.
- Skipped finding IDs:
  - `R7-F03`: skipped new recent-error composite index suggestion.
    Rationale: speculative performance nitpick outside the proposal's
    required schema surface; adding an index expands schema/write
    overhead without evidence from this loop.
- Flip-flops: none.
- Watch signals: `WS-1` and `WS-2` unchanged; `WS-3` remains a
  code-review residual.
- Test result after fixes: PASS —
  `cd src-tauri && cargo test 2>&1 | tail -5`.
- Determination: `continue` — amend latest commit and run CodeRabbit
  pass 8, the configured cap pass.

### Round 8 CodeRabbit pass

- Artifact under review: full branch diff against `main` at `6addfce`.
- Raw pass log: `CODERABBIT_pass8.md`.
- Findings: 4 total.
- Breakdown: 0 applied; 1 style nitpick not applied; 3 substantive
  findings not applied because pass 8 is the configured max-pass cap.
- Not-applied finding IDs:
  - `R8-F01`: optional prose tightening in
    `risk/10-supported-surface.md`; style churn.
  - `R8-F02`: new test metadata comments cite the contract path rather
    than the proposal test-intent path.
  - `R8-F03`: contract promises malformed-schema cases that the current
    `PRAGMA table_info(providers)`-based validation cannot enforce
    (non-table object and foreign-key constraints).
  - `R8-F04`: implementation-side pair to `R8-F03`; suggested
    `providers` object-type and FK validation.
- Flip-flops: none.
- Skipped rationales: `R8-F01` is style churn; `R8-F02`-`R8-F04`
  require human review because they arrived on the cap pass and may
  require choosing between stricter schema validation and narrowing the
  contract.
- Watch signals: `WS-1` remains the affected signal. The cap-pass
  findings show the unexpected-shape contract still needs a human
  decision on object-type/FK scope.
- Test result after fixes: not run; no pass-8 edits were applied.
- Determination: `MAX_PASSES_REACHED` — CodeRabbit did not converge
  within 8 passes.
