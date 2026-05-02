# Multi-Concern Review (Phase 8): proposals/10-routing-claude-skipped.md (diff)

## Verdict: SINGLE_CONCERN

The branch implements exactly one product change — re-keying the
`providers` aggregate and recent-error suppression by `provider_name`
(routing-claude-skipped fix). All product-source edits, the new
RCA-style integration test, and the workflow artifacts (`proposals/10-…`,
`research/10-…`, `risk/10-…`) are evidence for and execution of that
single concern. The two `.github/workflows/release.yml` /
`DECISIONS.md` lines that appear in the requested `git diff main..HEAD`
are not changes authored by this branch: they reflect commit `9df5603`
(PR #24, "drop Windows from release matrix; project is Unix-only")
which landed on `main` after this branch's merge base
(`9cadc90`). `git log main..HEAD -- .github/workflows/release.yml
DECISIONS.md` returns no commits, and `git diff main...HEAD` (three-dot,
branch-only from merge base) shows only the 18 routing-claude-skipped
files. Rebasing onto current `main` will make those phantom diffs
disappear; no split is warranted.

## Concern enumeration

Single concern: **routing fallback aggregate keyed by provider identity
(`(model_name, provider_name)`) instead of provider index.**

Branch-only files (`git diff main...HEAD --stat`):

- Product source — fix
  - `src-tauri/src/state/db.rs` — `ProviderRecord` retypes
    (`provider_index → provider_name`, counts to `i64`),
    `ensure_providers_schema` shape-based migration with rebuild from
    `invocations`, `validate_providers_schema` three-layer validator
    (object type, FK presence, column shape), `get_provider` and
    `recent_error_count` signatures retake `provider_name`,
    `finalize_invocation` aggregate upsert by name. (~1238 lines.)
  - `src-tauri/src/balancer/mod.rs` lines 258–262, 588–608, 620–632:
    `compute_projections_from_records`, `score_by_invocation_count`,
    `round_robin_fallback` updated to pass `&model.providers[i].name`;
    `min_count` widened to `i64`; new
    `fallback_recent_error_scoring_uses_provider_name_not_reused_index`
    test.
  - `src-tauri/examples/quota_check.rs` line 123: developer-tool call
    site update so the workspace builds after `get_provider`'s
    signature change (proposal §In scope explicitly bundles this).
- Product test — risk regression
  - `src-tauri/tests/rca_routing_claude_skipped.rs` — Phase 0 RCA
    harness for RC-1 (52 lines).
- Workflow evidence for the same fix (Phase 0–8 artifacts)
  - `proposals/10-routing-claude-skipped.md`,
    `research/10-routing-claude-skipped-{rca,problem-map,hookpoints,contract}.md`,
    `risk/10-{audit,history,scope,shortcut,supported-surface,test-residuals,step6b-log,step6b-output-index,step6b-prompt}.md`.

## Decomposition recommendation

N/A (verdict is SINGLE_CONCERN).

## Notes

- Two-dot vs. three-dot caveat: the prompt requested
  `git diff main..HEAD`, which conflates "behind main on an unrelated
  concern" with "this branch's contribution." The correct
  branch-contribution view is `git diff main...HEAD` or the union of
  files in `git log main..HEAD --name-only`. Both confirm 18 files,
  zero of them under `.github/` or `DECISIONS.md`.
- Migration rebuild SQL, the three-layer schema validator, and the
  developer-tool call-site update in `quota_check.rs` are not separate
  concerns from the fix: each is required to keep the workspace
  building and the schema reconciled within the same change.
- The Phase 4–8 risk artifacts and Phase 0–2.5 research artifacts are
  evidence-for-the-fix per the workflow, not separate concerns; the
  anti-pattern note in the prompt explicitly covers this case.
- Action item for the implementer (out of this review's scope):
  rebase onto current `main` (which now contains the PR #24
  Windows-removal + D-006 commit) so the two-dot diff matches the
  three-dot diff before the PR is opened.
