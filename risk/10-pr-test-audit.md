# Test Audit (Phase 8): proposals/10-routing-claude-skipped.md (round 2)

## Verdict: LOW

Round 2 re-audited HEAD `fd27f03` after fix-pass commit `67bb06c`.
The three round-1 findings are closed. The updated Phase 6b output index
now maps each post-Phase-6b test to a named risk, level, proposal source,
emitting commit, and firstness route; the three contract-only annotations
now cite `proposals/10-routing-claude-skipped.md` §Test-intent track; and
the preserved Phase 0 RCA harness now has inline risk/source metadata
immediately above its `#[test]` attribute.

No assertion relaxation, baseline regeneration, coverage deletion,
input-space narrowing, or risk annotation removal was found in the
round-2 fix pass. No new test-audit finding was introduced.

## Prior finding status

| Finding | Round-1 severity | Round-2 status | Evidence |
|---|---:|---|---|
| `R1-F01` Post-Phase-6b tests have no firstness evidence | blocking | closed | `risk/10-step6b-output-index.md` now has `## Post-Phase-6b additions (firstness evidence)` with all six post-6b tests mapped to risk, level, proposal source, commit, and firstness route. |
| `R1-F02` Some test annotations cite the contract instead of the proposal source | medium | closed | `src-tauri/src/state/db.rs:4330-4332`, `src-tauri/src/state/db.rs:4395-4398`, and `src-tauri/src/state/db.rs:5231-5234` now cite `proposals/10-routing-claude-skipped.md` §Test-intent track. |
| `R1-F03` Phase 0 RCA harness lacks inline risk metadata | low | closed | `src-tauri/tests/rca_routing_claude_skipped.rs:33-35` has `// Risk:` and `// Source:` comments immediately above `#[test]`. |

## Firstness routing

| Test or group | Risk | Level | Source | Firstness route |
|---|---|---|---|---|
| `src-tauri/tests/rca_routing_claude_skipped.rs::fallback_count_routing_uses_current_provider_identity_not_stale_index_history` | RC-1 fallback identity | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track; `research/10-routing-claude-skipped-rca.md` §Reproduction | Bug reproduction after defect, before fix -> no firstness-gap route. Preserved as Phase 0 RED at `9cadc90`; now carries inline risk/source metadata. |
| `src-tauri/src/state/db.rs::recent_error_count_uses_provider_name_not_reused_index_history` | `recent_error_count` identity drift | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | Complete Phase 6b cell in `risk/10-step6b-output-index.md`; separate test-writer evidence in commit `e84adaf`. |
| `src-tauri/src/balancer/mod.rs::fallback_recent_error_scoring_uses_provider_name_not_reused_index` | Balancer recent-error call-site | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | Complete Phase 6b cell in `risk/10-step6b-output-index.md`; cheapest reliable validator is unit coverage of the fallback call site. |
| `src-tauri/src/state/db.rs::providers_migration_rebuilds_aggregate_from_invocations_by_provider_name` | Providers migration from pre-fix aggregate shape | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | Complete Phase 6b cell in `risk/10-step6b-output-index.md`; on-disk DB open is the cheapest reliable migration validator. |
| `src-tauri/src/state/db.rs::provider_aggregate_round_trip_follows_name_after_reorder` | Aggregate writer/reader round-trip after provider reorder | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | Complete Phase 6b cell in `risk/10-step6b-output-index.md`; uses supported `start_invocation`/`finalize_invocation` path. |
| `src-tauri/src/state/db.rs::provider_aggregate_round_trip_does_not_inherit_renamed_provider_history` | Aggregate writer/reader round-trip after provider rename | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | Complete Phase 6b cell in `risk/10-step6b-output-index.md`; validates intended name identity directly. |
| `src-tauri/src/state/db.rs::quota_schema_remains_name_keyed_after_provider_migration` | Quota path unchanged regression | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | Complete Phase 6b cell in `risk/10-step6b-output-index.md`; unit schema assertion is the cheapest regression guard. |
| `src-tauri/src/state/db.rs::providers_migration_rejects_unexpected_shape_without_mutating_source_tables` | Migration error contract - unexpected shape rejected | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | Complete Phase 6b cell in `risk/10-step6b-output-index.md`; covers reachable early-rejection behavior, not WS-3 mid-rebuild rollback. |
| `src-tauri/src/state/db.rs::providers_migration_is_idempotent_across_reopens` | Migration `ensure_providers_schema` idempotent across reopens | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | Complete Phase 6b cell in `risk/10-step6b-output-index.md`; second open/no-op assertion is the cheapest reliable validator. |
| `src-tauri/src/state/db.rs::providers_migration_last_error_at_uses_most_recent_failure_not_later_success` | Migration `last_error_at` reflects most recent failed invocation | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | Complete Phase 6b cell in `risk/10-step6b-output-index.md`; fixture encodes success-after-failure and expected failed timestamp. |
| `src-tauri/src/state/db.rs::providers_migration_rejects_wrong_affinity_shape` | Migration error contract - wrong column affinity rejected | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | Post-Phase-6b cell now mapped in `risk/10-step6b-output-index.md`; CodeRabbit pass folded into `5c10702`, test added before corresponding affinity-rejection branch, RED at pre-hunk HEAD. |
| `src-tauri/src/state/db.rs::providers_preflight_rejects_malformed_shape_before_invocations_migration` | Migration error contract rejects before source-table mutation | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track; `research/10-routing-claude-skipped-contract.md` §2 Migration helper | Post-Phase-6b cell now mapped in `risk/10-step6b-output-index.md`; CodeRabbit pass folded into `5c10702`, test added before preflight call-ordering change, RED at pre-hunk HEAD. |
| `src-tauri/src/state/db.rs::providers_migration_last_error_ties_use_highest_invocation_id` | Migration `last_error_at` deterministic tie-break | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track; `research/10-routing-claude-skipped-contract.md` §2 Migration helper | Post-Phase-6b cell now mapped in `risk/10-step6b-output-index.md`; CodeRabbit pass folded into `5c10702`, test added before deterministic tie-break clause, RED at pre-hunk HEAD. |
| `src-tauri/src/state/db.rs::finalize_invocation_skips_provider_aggregate_for_null_provider_name` | Null-provider legacy rows must not synthesize aggregate identity | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track; `research/10-routing-claude-skipped-contract.md` §5 finalize_invocation | Post-Phase-6b cell now mapped in `risk/10-step6b-output-index.md`; CodeRabbit pass folded into `5c10702`, test added before skip-write clause, RED at pre-hunk HEAD. |
| `src-tauri/src/state/db.rs::providers_migration_rejects_non_table_object_named_providers` | Migration error contract - providers as non-table object rejected | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | Post-Phase-6b cell now mapped in `risk/10-step6b-output-index.md`; R8-F03 close-out in `bb106f7`, test added before `providers_object_type()` validation branch, RED at pre-hunk HEAD. |
| `src-tauri/src/state/db.rs::providers_migration_rejects_table_with_foreign_keys` | Migration error contract - providers with foreign keys rejected | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | Post-Phase-6b cell now mapped in `risk/10-step6b-output-index.md`; R8-F04 close-out in `bb106f7`, test added before `providers_has_foreign_keys()` validation branch, RED at pre-hunk HEAD. |

## Findings (round 2, Phase 8)

None.

## Notes

- Audited `git diff main..HEAD --stat` at HEAD `fd27f03`, as requested.
- `git diff --check main..HEAD` returned clean.
- `risk/10-test-residuals.md` still keeps WS-3 as a code-review residual; the runtime tests continue to cover early rejection, idempotency, and recovery branches without claiming to verify mid-rebuild rollback.
