# Justification Review (Phase 8): proposals/10-routing-claude-skipped.md (diff)

## Verdict: LOW_CONCERN

Every code change in the diff traces back to the approved proposal
(`proposals/10-routing-claude-skipped.md`) or its Phase 6a contract
(`research/10-routing-claude-skipped-contract.md`), with no drive-by
cleanup, unrelated fix, or speculative abstraction. The product surface
moved from `(model_name, provider_index)` to `(model_name, provider_name)`
on exactly the two readers and one writer named in the proposal
(`get_provider`, `recent_error_count`, `finalize_invocation`), the
shape-based migration / preflight helpers in `state/db.rs` match the
contract's three-layer validation (object-type, FK, columns) and
transactional rebuild, and the example/test call-site updates are the
ones the proposal §"In scope" enumerates. The release-workflow and
`DECISIONS.md` deltas that show up in `git diff main..HEAD --stat`
are an artifact of the branch being one commit behind `main` (merge-base
`9cadc90`); compared against the merge-base the branch touches no CI,
docs, or unrelated source files. One minor justification note on the
`ProviderRecord.last_error_at` / `last_invoked_at` Rust-type change
is below; it is contract-specified, so it is upheld but worth flagging.

## Findings

### Schema migration helper and three-layer validation
- Severity: low
- Change: `src-tauri/src/state/db.rs` — new `ensure_providers_schema`,
  `validate_providers_schema`, `providers_object_type`,
  `providers_has_foreign_keys`, `providers_columns`,
  `providers_shape_is_pre_fix/post_fix`, `columns_match`,
  `describe_columns`, `providers_schema_sql`; preflight call inserted
  before `ensure_invocations_schema` and inside
  `migrate_legacy_invocations`; old inline `CREATE TABLE providers`
  block deleted from `StateDb::open`.
- Stated purpose link: proposal §Migration / §"Migration error contract"
  and contract §2 "Migration helper" (object-type, FK, column-shape
  layers, transactional rebuild, idempotency, no source mutation).
- Justification: upheld. The R8-F03/F04 close in commit `bb106f7`
  adds the object-type and FK layers required by the contract's pass-8
  human decision; this is the explicitly approved scope.

### `ProviderRecord` re-key (`provider_index` → `provider_name`)
- Severity: low
- Change: `src-tauri/src/state/db.rs:118-130` — struct field rename and
  type change, plus new private `ProviderColumn` helper struct.
- Stated purpose link: proposal §"Schema change" and contract §3
  "ProviderRecord". Contract explicitly specifies `i64`, `Option<String>`
  field types.
- Justification: upheld. The `Option<DateTime<Utc>>` →
  `Option<String>` change for `last_error_at` / `last_invoked_at` is
  a small public-API simplification (no production consumer parses
  these fields), but it is contract-specified and consistent with the
  rebuilt-from-`invocations` data path that emits raw RFC3339 strings.
  Worth a flag because it is not strictly required by RC-1 itself,
  but the contract authorizes it.

### Reader/writer call-site updates
- Severity: low
- Change:
  `src-tauri/src/balancer/mod.rs:258-265, 588-607, 620-639` — pass
  `&model.providers[i].name` to `recent_error_count` and `get_provider`,
  `i64::MAX` / `as i64` follow-on type adjustments;
  `src-tauri/examples/quota_check.rs:123` — pass `&p.name`;
  `src-tauri/src/state/db.rs:1411-1493, 1726-1771` — `finalize_invocation`
  loads `provider_name` and skips aggregate writes when it is `NULL`,
  `get_provider` and `recent_error_count` queries re-keyed.
- Stated purpose link: proposal §"Reader changes", §"Writer changes",
  §"recent_error_count change"; contract §§3-6.
- Justification: upheld. Every call site listed in contract §3
  "Production callers" and §4 "Production callers" is updated, and
  none other.

### `finalize_invocation` skip-write for `provider_name IS NULL`
- Severity: low
- Change: `src-tauri/src/state/db.rs:1456-1493` — wraps the upsert and
  the failure-metadata update in `if let Some(provider_name) = ...`;
  `stderr_snippet` becomes `Option<String>` (writes `NULL` instead of
  empty string when no snippet present).
- Stated purpose link: contract §5 "Skip-write rule" and §5
  "Failure metadata update".
- Justification: upheld. The `Option<String>` snippet change is the
  contract-specified `?1 is the stderr snippet (or None)` semantics.

### New tests
- Severity: low
- Change: `src-tauri/src/balancer/mod.rs:732-748` — one new
  `fallback_recent_error_scoring_uses_provider_name_not_reused_index`
  unit test; `src-tauri/src/state/db.rs:3459-4416, 5229-5544` —
  migration fixtures, helpers, and 12 new unit / particular-integration
  tests; `src-tauri/tests/rca_routing_claude_skipped.rs` (52 lines) —
  Phase 0 RCA red harness.
- Stated purpose link: proposal §Test-intent track (every test maps
  to a named risk row); contract §7 "Test-intent handoff (Phase 6b
  inputs)" lists each one.
- Justification: upheld. Each test carries the required risk / level /
  source comments (R3-F01 from the CodeRabbit history).

### Workflow artifacts (proposal, RCA, contract, hookpoints, risk, audit)
- Severity: low
- Change: ~3,000 lines across `proposals/10-*.md`, `research/10-*.md`,
  `risk/10-*.md`.
- Stated purpose link: proposal/contract require these as Phase 0–6a
  inputs and Phase 4 risk-gate outputs; `risk/10-history.md` records
  the audit history that the Phase 8 review consumes.
- Justification: upheld. These are the project's documented gating
  artifacts for the fix; not drive-by documentation.

## Notes

- `git diff main..HEAD --stat` shows `.github/workflows/release.yml`
  (+15) and `DECISIONS.md` (-42) as if the branch re-added Windows to
  the release matrix. This is a viewing artifact: the merge-base is
  `9cadc90`, and `main` carries one commit ahead (`9df5603` "drop
  Windows from release matrix"). `git diff 9cadc90..HEAD --stat`
  confirms the branch makes no changes to CI workflows or
  `DECISIONS.md`. A rebase before merge will erase the apparent
  delta. No action required for this review.
- `CODERABBIT_summary.md` referenced in the prompt does not exist in
  the worktree. The CodeRabbit loop history is captured in
  `risk/10-history.md` §"CodeRabbit loop" rounds 1-8 and the per-pass
  raw logs `CODERABBIT_pass{1..8}.md` (those raw logs are also not
  committed to the branch — they are local pass scratch). The history
  in `risk/10-history.md` is sufficient to reconstruct the loop's
  applied/skipped findings. The cap-pass note that R8-F03/F04 needed
  human review was resolved by commit `bb106f7` adding the object-type
  and FK validation layers, which match contract §2.
- Watch signals `WS-1` (transactional + unexpected-shape rejection),
  `WS-2` (no index-keyed reader alias), and `WS-3` (mid-rebuild rollback
  is code-review-only) are upheld in the diff: the migration is wrapped
  in `Connection::transaction()` and rejects unexpected shapes via the
  preflight; no index-keyed alias remains on `get_provider` /
  `recent_error_count`; no runtime test claims mid-rebuild rollback.
