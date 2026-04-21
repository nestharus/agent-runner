# Justification: PR 1

## Verdict: JUSTIFIED

Every hunk in `feat/03-pr1-chatgpt-usage` (commits `75e6f00` + `cd32c93`,
computed against merge-base `90f433d` since main has since advanced with
unrelated docs commits) maps directly to either
`proposals/03-load-balancing-tiers.md` §2.3 (the tracked script + docs
fold-in) or §2.4 (the five enumerated test cases). No incidental
cleanups, no unrelated defects, no cross-cutting infrastructure. The
shell test harness introduced in `scripts/tests/` is scoped entirely
to this script — assertion helpers and the `compgen`-based runner live
inline in the single test file, so no shared library has been pulled
into scope prematurely.

## Hunks kept

- `scripts/chatgpt-usage` (new, 53 lines) — the core concern. Matches
  §2.3 point-for-point: credential validation mirroring the legacy
  installed script, unchanged HTTP call, and the conditional
  `if ... else empty end` multi-window emit with weekly at index 0
  and 5-hour at index 1.
- `scripts/tests/chatgpt-usage.test.sh` (new, 248 lines) — implements
  exactly the five test cases §2.4 calls for
  (`test_chatgpt_usage_emits_two_windows_on_normal_response`,
  `..._only_weekly_present`, `..._only_five_hour_present`,
  `..._credential_failure_exits_nonzero`,
  `scripts_readme_references_chatgpt_usage_adapter`). Helpers
  (`assert_eq`, `assert_status_zero`, `write_mock_curl`, etc.) are
  the minimum needed to run those five tests without hitting the live
  endpoint.
- `scripts/tests/fixtures/chatgpt-usage/normal-response.json` — drives
  the two-window happy path.
- `scripts/tests/fixtures/chatgpt-usage/only-weekly.json` — drives
  §2.4's "only secondary_window" branch.
- `scripts/tests/fixtures/chatgpt-usage/only-five-hour.json` — drives
  §2.4's "only primary_window" branch.
- `scripts/tests/fixtures/chatgpt-usage/empty-tokens.json` — drives
  the credential-failure case.
- `README.md` (+2 net lines) — adds `chatgpt-usage` to the
  reference-adapter table and install example. Explicitly called out
  by §2.3 and human-gate decision C.
- `scripts/README.md` (+3 net lines) — adds `chatgpt-usage` to the
  reference-adapter inventory. Explicitly called out by §2.3 and
  resolves prior scope-risk finding G1.

## Hunks that should move elsewhere

None.

## Non-blocking observations

- `assert_weekly_window` and `assert_five_hour_window`
  (`scripts/tests/chatgpt-usage.test.sh:123-141`) have identical
  bodies. Keeping them separate reads as self-documenting labels for
  future test authors; the duplication is 4 lines and not worth
  fixing in this PR.
- Each test function installs its own `trap "rm -rf '$tmpdir'" EXIT`,
  and `test_chatgpt_usage_credential_failure_exits_nonzero`
  additionally calls `assert_credential_failure` twice with nested
  subdirectories under the same outer `$tmpdir`. The outer trap
  still removes everything at EXIT, so cleanup is correct; if a
  future test invokes several independent tempdirs it may be worth
  adopting a central cleanup list, but that is clearly outside PR 1
  scope.
- The `main` dispatcher discovers tests via
  `compgen -A function | grep -E '^(test_|scripts_readme_)'`
  (`chatgpt-usage.test.sh:228-235`). This regex is specific to the
  two naming prefixes used in this file, so it is not silently
  recruiting generic helper names — good containment. If a follow-up
  PR adds a second test script (e.g., for PR 2 or PR 3), the
  dispatcher pattern can be generalized then rather than now.
- `scripts/chatgpt-usage` has no `shellcheck` disable comments and
  passes the same strict-mode conventions as `anthropic-usage`;
  worth confirming under `shellcheck` in the audit gate but not a
  justification concern.
