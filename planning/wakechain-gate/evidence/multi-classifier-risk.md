# Wakechain multi-classifier risk notes

This file is context only and is not a waiver.

Prior gate carry-over:

| Source | Status on this branch |
|---|---|
| #43 wake-confirm gate split-only commit `2845c30` | Re-applied with `git cherry-pick --no-commit` before auditor dispatch if absent from this branch. It splits test helper construction in `crates/oulipoly-runtime/src/sessions/mod.rs` and environment/command parsing helpers in `scripts/opencode-turns` without changing behavior. |
| #42 lock-leak gate declarations at `5447631` | Already present in this lineage. No additional #42 split commit is required. |

#44 focused risk surface:

| Function | Intended single classification | Notes |
|---|---|---|
| `wake_sweep_plan` | orchestration | Buckets candidate dispositions and delegates selection. |
| `select_recoverable_sweep_candidates` | filter | Selects oldest/newest recoverable candidates under a cap. |
| `wake_sweep_candidate_disposition` | orchestration | Sequences named predicates and maps to disposition. |
| `reap_abandoned_sweep_candidates` | orchestration | Applies `mark_pending_abandoned` to selected debris. |
| `wake_sweep_candidate_has_live_owner` | predicate | Answers whether any pending row has a live matched owner. |
| `mailbox_row_has_live_owner_identity` | predicate | Compares live process identity with a mapped row identity. |
| `mailbox_row_owner_identity` | mapper | Projects optional mailbox row fields into `ProcessIdentity`. |
| `mark_pending_abandoned` | orchestration | Transactionally marks bounded rows and releases the wake claim when rows changed. |
| `pending_wake_session_ids` | orchestration | Combines oldest/newest SQL selections and deduplicates. |
| `pending_wake_session_ids_by_oldest_seq` | filter | SQL filter over eligible pending wake sessions. |

If the function-classification auditor identifies a remaining multi-classifier body, split the body instead of waiving it.
