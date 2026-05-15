# Agent Runner Quota Migration Eval

## Behavior

Direct pooled dispatch and headless resume retry within the configured provider
pool after a failed provider is classified as `quota_exhausted`. The retry path
marks the failed provider exhausted before re-selection, so routing and resume
migration can exclude that provider and choose another eligible pool member.

Headless resume treats heuristic stderr quota detection and diagnostic-model
quota output as the same trigger. A stderr shape that produces `Heuristic
classification based on stderr content` and a diagnostics model that emits
`quota_exhausted` both drive the same mark, finalize, migrate, and retry flow.

## Boundaries

- Retry is bounded by the configured pool size.
- Exhausted providers are finalized and marked before another provider is
  selected.
- All-exhausted direct and resume dispatches exit nonzero and emit
  `BLOCKED:all-providers-exhausted`.
- Non-quota failures return the original provider failure and do not mark quota
  state or retry another provider.
- Resume migration stays inside the configured same-family/migratable pool; no
  cross-family fallback is claimed.

## Verification

Resume coverage lives in `src-tauri/tests/age100_resume_quota_migration.rs`:

- `resume_quota_exhausted_marks_provider_and_migrates_to_next_pool_member`
- `resume_retries_n_minus_one_quota_exhausted_providers_then_succeeds`
- `resume_all_pool_members_quota_exhausted_returns_all_providers_exhausted`
- `resume_non_quota_failure_does_not_migrate_or_mark_exhausted`
- `resume_heuristic_stderr_quota_uses_same_path_as_diagnostic_model_quota`

Direct pooled dispatch coverage lives in
`src-tauri/tests/age100_one_shot_quota_migration.rs`:

- `one_shot_all_pool_members_quota_exhausted_returns_blocked_all_providers_exhausted`
