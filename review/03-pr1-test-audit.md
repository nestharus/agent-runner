# Test-Audit Gate: PR 1 — chatgpt-usage 5h+weekly

## Overall verdict: PASS

The diff is a faithful translation of `proposals/03-load-balancing-tiers.md`
§2: a tracked `scripts/chatgpt-usage` that emits the two-window `windows`
array (weekly first, 5h second), uses the `if … else empty end` pattern
to short-circuit on partial upstream responses, preserves the
credential-validation exits, and updates both reference-adapter
inventories. The five-test harness exercises the JSON-shape contract on
three response variants plus the credential-failure branch, and the
README inventory check is anchored to the install command and the
adapter-table row rather than a loose substring grep. Coverage of the
HTTP-failure and malformed-JSON branches is absent (acknowledged
implementation-mode PARTIAL). No FAILs and no non-acknowledged
PARTIALs — PR 1 is clear to open from this gate's perspective.

Note on commit SHAs: the prompt cites `75e6f00` / `cd32c93`; the worktree
HEAD has `96ceb54` / `de58dee` with identical commit messages and
identical resulting trees. Audit was performed against the current
worktree HEAD.

## Sub-audit 1 — Spec alignment

Verdict: PASS

Against `proposals/03-load-balancing-tiers.md` §2.2–§2.3 and
`research/03-load-balancing-tiers-hookpoints.md` §1:

- Target emit shape matches the `windows` contract. The jq block at
  `scripts/chatgpt-usage:47-53` produces a `{windows: [...]}` envelope
  in the order weekly → 5h, mirroring `scripts/anthropic-usage:45-54`'s
  longest-window-first convention required by §2.2 line 19 / §2.3
  line 40 for positional `window_id` stability.
- Window identity. `secondary_window` is array index 0 (weekly);
  `primary_window` is array index 1 (5h). That matches §2.2/§2.3 and
  the doc comment at `scripts/chatgpt-usage:11-13`, which correctly
  notes `secondary_window` is weekly and `primary_window` is the
  5-hour window (the legacy installed script's comments had this
  inverted per §2.1).
- Partial upstream responses emit valid JSON with a shorter array.
  Both array entries are wrapped in
  `if .rate_limit.<window>.resets_at then {…} else empty end`
  (`scripts/chatgpt-usage:48-51`), matching the `anthropic-usage`
  idiom §2.3 calls out as required. End-to-end behavior verified by
  the `only-weekly` and `only-five-hour` fixtures.
- Credential validation preserved. `[[ ! -r "$CREDS" ]]` exits 2 with
  stderr `"auth file not readable: $CREDS"`
  (`scripts/chatgpt-usage:25-28`); empty `tokens.access_token` or
  `tokens.account_id` exits 3 with stderr
  (`scripts/chatgpt-usage:30-34`). Both predicates and exit-code
  semantics match the existing-installed-script contract referenced in
  §2.3 line 38, and `set -euo pipefail` at line 20 ensures a `curl`
  failure also produces a non-zero exit before the jq stage, per the
  §Q11 "empty stdout on script failure, not empty windows" contract.
- Reference-adapter inventories updated. `README.md:252` adds the new
  `chatgpt-usage ~/.codex/auth.json` row to the quota-script table and
  `README.md:258` adds the script to the `install -m 755 …` example,
  closing §2.3's reference to `README.md:254-258`.
  `scripts/README.md:208-209` lists `chatgpt-usage ~/.codex/auth.json
  (weekly + 5h windows from /backend-api/wham/usage)` alongside
  `anthropic-usage` and `zai-usage`, closing the §2.3 reference to
  `scripts/README.md:207-209`.

No spec gaps observed.

## Sub-audit 2 — Test quality

Verdict: PASS

Strengths:

- Realistic fixtures. `normal-response.json` nests both
  `primary_window` and `secondary_window` under `rate_limit` with
  RFC3339 `resets_at` strings and integer `used_percent` values,
  matching the response sample in
  `research/03-load-balancing-tiers-data-a.md` and the shape the
  script's doc comment documents at `scripts/chatgpt-usage:11-15`. The
  partial fixtures (`only-weekly.json`, `only-five-hour.json`) each
  omit one tier cleanly, exercising the `if … else empty end`
  branches.
- The HTTP mock intercepts the real call. `write_mock_curl` at
  `scripts/tests/chatgpt-usage.test.sh:79-93` writes a shell stub
  named `curl`; `run_chatgpt_usage:111-119` prepends its directory to
  `PATH` so the script's bare `curl -sS …` call resolves to the stub.
  The stub `cat`s `CHATGPT_USAGE_MOCK_RESPONSE_FILE` to stdout, which
  the script then pipes through jq — i.e. the mock exercises the jq
  pipeline end to end, not a bypass.
- Credential-failure assertions satisfy the §Q11 contract.
  `assert_credential_failure:184-195` combines `assert_status_nonzero`,
  `assert_stdout_empty`, and `assert_stderr_matches`, and it is
  invoked twice from `test_chatgpt_usage_credential_failure_exits_nonzero:197-211`:
  once with a nonexistent path (exit 2 branch) and once with
  `empty-tokens.json` (exit 3 branch). Both exit paths of the
  credential guard are covered with all three contract-level
  assertions.
- README assertions are anchored, not loose.
  `scripts_readme_references_chatgpt_usage_adapter:213-228` runs three
  separate checks: `^install .*scripts/chatgpt-usage` (must be the
  install line, not prose), `^\| `chatgpt-usage ` (must be a table
  row, not a code-block mention), and a fixed-string match for
  `chatgpt-usage ~/.codex/auth.json` against `scripts/README.md`. A
  removal note like `"removed the chatgpt-usage script"` would not
  satisfy any of the three. This properly enforces the §2.3
  scope-risk-G1 docs requirement.
- Pre-impl / post-impl behavior holds. Re-verified during this audit
  by moving `scripts/chatgpt-usage` aside and re-running: 4/5 tests
  fail (`test_chatgpt_usage_*` exit 127). The README test stays green
  because the README hunks landed in the same commit as the script
  (`96ceb54`), so the docs invariant test continues to pass against
  the patched READMEs even with the script removed — that's
  consistent with the docs/script split. Post-impl: `5/5 PASS`,
  confirmed.
- Each test scopes its own tmpdir cleanup. `trap "rm -rf '$tmpdir'"
  EXIT` is set inside each test function body, and the runner invokes
  each test in a subshell at line 248
  (`if ( "$test_name" ); then`), so traps do not leak between tests.
  Function discovery uses `LC_ALL=C sort` for stable ordering.

The tests are not tautological: every assertion would fail on a real
regression to the primary `windows` shape (positional ordering, field
mapping, partial-response handling, credential exits, or README
inventory). The harness is self-contained, re-runnable, and cleanly
fails when the implementation is missing.

## Sub-audit 3 — Coverage delta

Verdict: PARTIAL (implementation-mode PARTIAL is acknowledged)

Baseline: `main` has no `scripts/chatgpt-usage`, so this is net-new
coverage rather than a delta against a prior test suite.

Branches covered (5 tests, all passing):

- Happy path: both windows present → 2-entry array in weekly-first
  order (`test_chatgpt_usage_emits_two_windows_on_normal_response`,
  using `normal-response.json`).
- Partial upstream: only `secondary_window` present → 1-entry array
  (`test_chatgpt_usage_emits_one_window_when_only_weekly_present`).
- Partial upstream: only `primary_window` present → 1-entry array
  (`test_chatgpt_usage_emits_one_window_when_only_five_hour_present`).
- Unreadable auth file → exit 2, empty stdout, stderr matches
  `(credential|auth|readable)`
  (credential failure test, first branch).
- Missing `tokens.access_token` / `tokens.account_id` → exit 3, empty
  stdout, stderr matches
  `(token|account|credential|auth|missing)` (credential failure test,
  second branch, using `empty-tokens.json`).
- Docs invariant: `chatgpt-usage` install command + table row + the
  `scripts/README.md` inventory line all present and well-anchored.

Branches *not* covered:

- `curl` exit non-zero (network error, HTTP timeout via `--max-time
  20`) — relies on `set -euo pipefail` to propagate; nothing asserts
  the resulting exit/stderr shape.
- `curl` emits non-JSON (e.g. an HTML 5xx body) → jq throws → script
  exits non-zero with jq's stderr. Untested.
- `{}` response or `{"rate_limit":{}}` (both windows absent from the
  JSON) → `{"windows": []}`. Verified by hand from the jq filter; no
  test pins this to the contract.
- `rate_limit` present but both `*_window.resets_at` values missing or
  null → same empty-array output. Untested.
- `used_percent` types other than integer/float (strings, missing
  entirely). The `// 0` default fires only on `null`; a string value
  would silently pass through and break Rust-side parsing. Out of
  scope for a shell script but a contract boundary worth noting.
- Single-field credential gap (only `tokens.access_token` empty, or
  only `tokens.account_id` empty) — the OR predicate at script line 33
  guarantees either gap exits 3, but the single-field cases aren't
  individually fixtured.

Without a CI baseline for shell scripts, this set is not shippable on
its own as full coverage, but it is sufficient for the primary
`windows`-emit contract that PR 2 and PR 3 will lean on. Per the
orchestrator rules, implementation-mode coverage-delta PARTIAL is
acknowledged and does not block PR opening.

## Blocking issues

None. No FAIL verdicts. The only PARTIAL is the acknowledged
implementation-mode coverage delta.

## Non-blocking observations

- Add a fixture for an empty `{}` upstream response and assert
  `.windows | length == 0`; this would pin the jq block's "don't fail
  on missing rate_limit" behavior and harden the contract PR 2 relies
  on.
- Add a mock-`curl` variant that exits non-zero (e.g. `exit 22`) and
  assert non-zero script exit + empty stdout. A one-line addition
  that closes the `set -euo pipefail` safety net.
- Consider pinning the exit codes themselves (2 for unreadable file,
  3 for missing tokens) rather than just "non-zero". The proposal
  doesn't mandate specific codes, but the script parallels
  `anthropic-usage` which uses the same convention, and pinning them
  prevents silent drift.
- The PATH-shadowed `curl` mock does not assert URL or header values.
  A regression that pointed `chatgpt-usage` at the wrong endpoint, or
  dropped the `ChatGPT-Account-Id` header, would still pass the JSON
  assertions. Capturing `"$@"` in the mock and grepping the captured
  args would close this. Out of scope for this PR.
- The test file's discovery pattern
  (`compgen -A function | grep -E '^(test_|scripts_readme_)'`) is
  slightly surprising because the non-`test_` prefix exists only so
  `scripts_readme_references_chatgpt_usage_adapter` is discovered.
  Renaming it to `test_readme_references_chatgpt_usage_adapter` would
  let the discovery filter be just `^test_`, matching the precedent
  patterns elsewhere.
- Branch HEAD SHAs in the worktree (`96ceb54` / `de58dee`) differ from
  the prompt's quoted SHAs (`75e6f00` / `cd32c93`). Commit messages
  and trees are identical; flagging only so the orchestrator's audit
  log isn't surprised.
