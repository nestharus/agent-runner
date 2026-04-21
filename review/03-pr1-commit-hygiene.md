# Commit Hygiene Audit: PR 1

**Branch:** `feat/03-pr1-chatgpt-usage`
**Base:** `main`
**Commits reviewed:** `de58dee` (test), `96ceb54` (feat)
**Mode:** audit-only (read-only; branch not modified)

## Verdict: CLEAN

The two-commit red/green TDD structure is appropriate for this PR and is
executed correctly: the test commit precedes the feature commit, each
commit is internally single-concern, the cumulative diff tells a clean
story (tests assert the contract → code delivers it), and no
drop-then-restore or transient-regression patterns are present.
Per-commit test verification confirms a genuine red → green transition
(5/5 fail at `de58dee`, 5/5 pass at `96ceb54`). The one weakness is
message quality: both commits are subject-only with no body explaining
WHY (motivation, risk, alternatives). That is a non-blocking
observation, not a REORGANIZE trigger — the subjects themselves are
specific and scoped (neither is "wip", "fix", or "address feedback").

## Per-commit evaluation

### `de58dee` — test(pr1): chatgpt-usage 5h+weekly window emission

**Message evaluation**
- **Type/scope:** `test(pr1)` — type is correct. Scope `pr1` is a
  PR-number label rather than a module (guide suggests a module
  scope like `chatgpt-usage`), but this is consistent with the
  initiative-03 convention used on sibling branches and not a
  reorg trigger on its own.
- **Subject:** specific (names the feature and the behavior being
  tested — "5h+weekly window emission"). Under 72 chars. Minor style
  nit: it is an elliptical noun phrase, not imperative ("cover 5h +
  weekly window emission" would be more strictly imperative).
- **Body:** *missing.* Does not explain why a test-first split was
  chosen, why both windows are asserted in one harness, or what
  contract the fixtures encode. The guide calls for
  "what and why, not how" in the body — this commit has neither.

**Scope**
Single-concern. Adds `scripts/tests/chatgpt-usage.test.sh` (260 lines:
shared assertion helpers, mock-curl shim, 4 test functions + 1 README
anchor test) and 4 fixtures (`normal-response`, `only-weekly`,
`only-five-hour`, `empty-tokens`). All changes are scaffolding for one
feature. No unrelated hitchhikers.

**Red-state verification** (verified via worktree at `de58dee`)
- `bash scripts/tests/chatgpt-usage.test.sh` → exit **1**
- All 5 tests FAIL with consistent causes:
  - `scripts_readme_references_chatgpt_usage_adapter`: README.md
    install row / adapter table row absent at this commit
  - The 4 runtime tests: `scripts/chatgpt-usage` does not exist at
    this commit → `No such file or directory` (exit 127) → assertions
    that expected exit 0 fail; the credential-failure test's
    stderr-pattern assertion fails because bash's "No such file"
    message does not match the expected credential pattern.
- This is a **clean red**: the tests fail for exactly the reason they
  should at this point in the chain (feature not yet implemented).
- Size: 291 insertions, 0 deletions. Appropriate for the scope.

### `96ceb54` — feat(pr1): chatgpt-usage emits 5h + weekly windows

**Message evaluation**
- **Type/scope:** `feat(pr1)` — type is correct. Same scope nit as
  above.
- **Subject:** specific (names the script and the two windows it
  emits). Under 72 chars. Minor style nit: "emits" is indicative
  mood; strict imperative would be "emit 5h + weekly windows from
  chatgpt-usage".
- **Body:** *missing.* Does not explain why both windows are
  surfaced (equivalent to the existing `anthropic-usage` and
  `zai-usage` adapter pattern), why `secondary_window` is ordered
  first in the emitted array, or why the `(if ... else empty end)`
  pattern is used (so tests can assert `.windows | length` shrinks
  when one side is absent). The rationale is implicit in the code
  comments and fixtures but not in the commit message.

**Scope**
Single-concern: ships the feature tested in `de58dee`. Contents:
- `scripts/chatgpt-usage` (new 53-line script) — the implementation.
- `README.md` (+2/-1) — adapter-inventory table row and install
  command update (mandatory docs update: the test at `de58dee`
  asserts these exact README anchors, so omitting them would leave
  the feat commit red).
- `scripts/README.md` (+3/-2) — adapter inventory reference update,
  also required by the README anchor test.

The README edits are not a separable concern: they are the
documentation half of the same adapter-surface contract the tests
assert, and splitting them would leave either the test or the feat
commit in a broken state. Bundling is correct.

**Green-state verification** (verified via worktree at `96ceb54`)
- `bash scripts/tests/chatgpt-usage.test.sh` → exit **0**
- All 5 tests PASS (`scripts_readme_references_chatgpt_usage_adapter`,
  `test_chatgpt_usage_credential_failure_exits_nonzero`,
  `test_chatgpt_usage_emits_one_window_when_only_five_hour_present`,
  `test_chatgpt_usage_emits_one_window_when_only_weekly_present`,
  `test_chatgpt_usage_emits_two_windows_on_normal_response`).
- This is a **clean green**: the feat commit flips every assertion
  the test commit encoded, with no tests left red and no tests
  skipped.
- Size: 58 insertions, 3 deletions across 3 files. Small and focused.

## Cross-commit checks

- **Ordering:** test commit precedes feat commit (`git log main..HEAD`:
  `96ceb54` → `de58dee`). Correct for red/green TDD.
- **Drop-then-restore:** none. Both commits are additive; no removal
  in `de58dee` that is re-added in `96ceb54`, and no intermediate
  simplification that a later commit walks back.
- **Transient regressions:** none. Main is green pre-branch; test
  commit is red against the new test harness only (expected); feat
  commit is fully green. At no point does a pre-existing test break.
- **Cumulative diff:** 5 files, +349/-3 lines. Matches the PR
  description (script + test harness + fixtures + 2 README edits).

## If REORGANIZE: what to change

Not triggered. If the author wants to volunteer a cleanup pass
anyway (not required to pass this gate), the only material
improvement would be adding message bodies:

```
test(pr1): chatgpt-usage 5h+weekly window emission

Encode the ChatGPT usage-adapter contract ahead of the
implementation in 96ceb54: both primary (5h) and secondary
(weekly) windows are surfaced; either side being absent in the
upstream response produces a shorter `windows` array, not a
placeholder row. Fixtures cover the full-response, weekly-only,
five-hour-only, and empty-tokens paths. The README anchor test
exists so removing the script from the docs surface counts as a
regression.
```

```
feat(pr1): chatgpt-usage emits 5h + weekly windows

Mirror the anthropic-usage / zai-usage adapter pattern: emit a
`{windows: [...]}` envelope with secondary (weekly) ordered
first, primary (5h) second, and each entry omitted when the
upstream response lacks the corresponding window. This keeps the
downstream scorer's multi-window consumer logic uniform across
providers. Script reads credentials from `~/.codex/auth.json`
(Codex) or `~/.config/opencode/auth.json` (opencode) and calls
`/backend-api/wham/usage` under the OpenAI OAuth account header.
```

Rewriting these messages would be a pure message-only reorganize
(no hunk re-staging), so `git rebase -i main` with `reword` on both
commits is sufficient. Not required.

## Non-blocking observations

1. **Scope label `pr1` vs module scope.** The guide suggests module
   scope (`chatgpt-usage`) over PR-number scope. Sibling branches
   (`feat/03-pr2-*`, `feat/03-pr3-*`) use the same `pr1`/`pr2`/`pr3`
   convention, so this is initiative-wide stylistic consistency, not
   a per-PR defect. Flag for the initiative, not this PR.

2. **Subject mood.** Both subjects lean indicative
   (`emission`, `emits`) rather than imperative (`emit`,
   `cover`). Minor; does not obstruct review.

3. **Message body absence.** The strongest signal in the guide for
   REORGANIZE on messages is vagueness (`wip`, `fix`,
   `address feedback`). These subjects are specific, so this does
   not cross the threshold — but the "why" content is genuinely
   absent, and a future maintainer bisecting will lack context
   beyond the subject line.

4. **Test harness anchor-test depth.** The
   `scripts_readme_references_chatgpt_usage_adapter` anchor tests
   are encoded in the test commit and do real work (they caught a
   hypothetical docs-removal regression). This is a quiet strength
   of `de58dee` worth preserving in any future message rewrite.

5. **Per-commit run of the project's full test suite was not
   performed** — only the PR-scoped harness
   (`scripts/tests/chatgpt-usage.test.sh`) was run at each commit.
   This is appropriate scope for commit-hygiene audit (the
   commit-hygiene gate asks "does each commit stand on its own with
   respect to what it touches"), but a separate per-commit-CI pass
   is the orchestrator's concern, not this operator's.
