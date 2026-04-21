# Multi-Concern Check: PR 1

## Verdict: single-concern

PR 1 is one coherent unit of work: *introduce a tracked
`chatgpt-usage` reference adapter that emits both the 5-hour and
weekly ChatGPT windows, documented in the two READMEs and covered
by script-level tests.* Each of the three candidate seams
(test/feat, script/docs, harness/script) fails the independence
test — the pieces are mutually load-bearing, and splitting any of
them produces a strictly worse review.

## Evaluation of each candidate concern

### 1. Test commit (`75e6f00`) vs feat commit (`cd32c93`) — **same concern**

The two commits are a red/green pair, not two concerns. The test
commit encodes the contract (`test_chatgpt_usage_emits_two_windows_on_normal_response`,
`_only_weekly_present`, `_only_five_hour_present`,
`_credential_failure_exits_nonzero`,
`scripts_readme_references_chatgpt_usage_adapter`) and the feat
commit is the minimum implementation that satisfies it. They
were authored 5 minutes apart against the same empty slate —
there is no meaningful "PR 1a lands, then we decide on PR 1b"
story. Merging the test alone would ship failing CI; merging the
feat alone would ship untested behavior. Keeping them as two
commits inside one PR already gives reviewers the test-first
narrative without the overhead of two PRs.

### 2. Script change vs docs update (`README.md`, `scripts/README.md`) — **same concern**

The docs are not marketing copy tacked on as an afterthought;
they are part of the scope that the test enforces.
`scripts/tests/chatgpt-usage.test.sh:235-239` runs
`scripts_readme_references_chatgpt_usage_adapter`, which greps
both `README.md` and `scripts/README.md` for `chatgpt-usage`. If
the docs were split into a later PR, this PR's test would fail at
merge. Conversely, if the docs landed first, they would list a
script that did not yet exist in the repo — an obviously broken
intermediate state.

The proposal is explicit about this coupling:
`proposals/03-load-balancing-tiers.md:43-46` records that the
README additions fold in prior scope-risk finding G1 (human-gate
decision C, locked 2026-04-21), meaning the user has already made
the scoping call that the script and its documented-adapter
status ship together. Re-splitting them here would re-open a
decision the human gate already closed.

### 3. New test harness / fixture convention vs the `chatgpt-usage` change — **same concern**

`scripts/tests/chatgpt-usage.test.sh` does introduce assertion
helpers (`assert_eq`, `assert_jq_eq`, `assert_stderr_matches`,
`write_mock_curl`, etc.) and a fixture-directory layout that are
new to the repo. That *could* be packaged as a standalone
"establish script-testing convention" PR. But:

- The helpers are defined inline in the one test file, not
  hoisted into a shared harness. There is nothing for a
  separate PR to extract — splitting would require first
  inventing a shared location, then migrating, which is pure
  churn with no consumer.
- The fixtures are all `chatgpt-usage`-specific
  (`normal-response.json`, `only-weekly.json`, `only-five-hour.json`,
  `empty-tokens.json`). They have no meaning without the script
  under test.
- A "convention-only" PR would have zero behavior and zero
  consumers — the kind of speculative scaffolding
  `AGENTS.md` / `CLAUDE.md` guidance tells us to avoid.

If and when a second script adopts the same pattern, *that* PR
should be the one to hoist the reusable pieces into a shared
harness. Hoisting pre-emptively here would be speculative
abstraction.

## Cross-checks against the `AGENTS.md` split rules

- **"Large deletion is its own PR."** N/A — this PR is purely
  additive. The installed `/home/nes/.local/bin/chatgpt-usage`
  lives outside the repo and is refreshed manually after merge
  (`proposals/03-load-balancing-tiers.md:48`), so no in-tree
  deletion pairs with this addition.
- **"Additive changes go before behavioral changes."** Satisfied
  — this whole PR is the additive step. The behavioral
  consumers (scoring, refresh) land in PR 2 / PR 3 per
  `proposals/03-load-balancing-tiers.md:3,472`, which are
  separately scoped and not mixed in here.
- **"Dependency order matters."** PR 1 is structurally
  independent of PR 2 and PR 3
  (`proposals/03-load-balancing-tiers.md:472`), so no intra-PR
  ordering issue exists to split around.

## Why the files/commits belong together

One new script, the two test assets that cover it (harness +
fixtures), and the two README lines that the harness greps for.
Five files, ~337 insertions, two commits arranged test-then-feat.
Each piece references at least one other piece: the test greps
the READMEs, the fixtures exist only for the test, the feat
satisfies the test, and the READMEs advertise the feat. Splitting
any seam produces either a failing-CI intermediate state or an
empty speculative scaffold PR. Ship as one.
