# Phase 8 Synthesis — 06-import-replace

**Decision:** ADVANCE to Phase 9 (push + draft PR).

## Round 1 verdicts (5 gates)

| Gate | Verdict | Notes |
|---|---|---|
| test-audit | BLOCKING | TA-F01..F06 — 6 coverage gaps |
| commit-hygiene | BLOCKING | CH-001 (test commit doesn't build alone), CH-002 (audit commit mixed code+docs) |
| supported-surface | MEDIUM | S-PR-F01 (Export CLI), S-PR-F02 (A1 inline), S-PR-F03 (recovery scope), S-PR-F04 (README), S-PR-F05 (carryovers) |
| multi-concern | RECOMMEND-SPLIT | Same Export-CLI concern as S-PR-F01 |
| justification | LOW_CONCERN | J1 = same Export drift; J2 = accepted (foundation primitives reimplemented) |

## Convergent fix-pass items applied

Tip: `7bc4b40 fix(06-import-replace): add Risk/Level/Source annotations to fix-pass tests (TA-R2-F01)`

| ID | Round-1 finding | Fix applied |
|---|---|---|
| F1 | S-PR-F01 / J1 / multi-concern (Export CLI drift) | `SessionSubcommands::Export` marked `#[command(hide = true)]`; remains callable for round-trip oracle, hidden from `--help`. `src-tauri/src/main.rs:177-179` |
| F2 | S-PR-F03 (recovery scope) | `recover_pending_replaces()` moved to top of `run`. All CLI subcommands now run recovery before subcommand dispatch. `src-tauri/src/main.rs:309-313` |
| F3a | TA-F01 postimage exactness | T1/T2 assert `receipt.postimage_sha256 == sha256(export.stdout)`. `src-tauri/tests/initiative_06_import_replace.rs:52,91,1077` |
| F3b | TA-F02 mismatch | `t_session_id_mismatch_in_input`, `t_provider_name_mismatch_in_input`. Product validation at `src-tauri/src/session_replace/mod.rs:730-742` |
| F3c | TA-F03 unsupported records | `t_unsupported_record_class`. Parser rejects all-unsupported canonical input |
| F3d | TA-F04 schema-incompatible | `t_schema_incompatible_exit_14`. Read-only preflight at `src-tauri/src/session_replace/mod.rs:266-313` |
| F3e | TA-F05 malformed input | 4 tests: empty, blank line, missing field, non-UTF-8 |
| F3f | TA-F06 DB replacement | `t_unrelated_session_unchanged_after_replace`. Two-session fixture, asserts B unchanged, A segment identity preserved, chain `last_used_at` refreshed |
| F4 | CH-001 + CH-002 | Squashed test+impl into `2201a57 feat(...): Phase 6 — tests + agents session import-replace`; split `0c1707d` into `f9ce18a risk(...): process-tree audit` (docs) + `ecb198f fix(...): CodeRabbit fix-pass` (code) |
| TA-R2-F01 | R2: missing test annotations | All 9 fix-pass tests now carry Risk/Level/Source/Observable/Residual comment blocks |

## Round 2 verdicts (3 gates re-run after fix-pass)

| Gate | R2 Verdict | Notes |
|---|---|---|
| test-audit | PASS (after TA-R2-F01 cosmetic fix) | TA-F01..F06 closed; 29 integration tests pass |
| commit-hygiene | PASS | History buildable per-commit; no agent trailers; signed |
| supported-surface | LOW (termination=none) | S-PR-F01 + S-PR-F03 closed; S-PR-F02/F04/F05 are non-blocking carryovers |

## Carryover findings (non-blocking, deferred)

| ID | Disposition |
|---|---|
| S-PR-F02 (A1 inlined as private types) | Forward-compat note: `internal::SessionLock`, `internal::SessionMetadata` are private. When sibling 06-locate / 06-pause-handshake PRs land, they will reconcile. Documented at proposal §11/§13. No action this PR. |
| S-PR-F04 (README §10 missing) | Deferred to follow-up doc PR; cohort A (harness) reads proposal directly; cohort B/E discoverability gap acknowledged. |
| S-PR-F05 (R4-F01..F04 prose carryovers) | Phase 4 prose-only; defer to doc PR. |
| Phase 7 max-pass skips | Lease renewal, multimodal canonical schema, race-barrier refactor, strict stderr — all design-scope expansions outside Rev 4 v1. Recorded in audit history. |

## Final cargo test
`cargo test --manifest-path src-tauri/Cargo.toml`: 411 passed, 0 failed.

## Final history
```
7bc4b40 fix(06-import-replace): add Risk/Level/Source annotations to fix-pass tests (TA-R2-F01)
f3dbd70 fix(06-import-replace): address Phase 8 review fix-pass
ecb198f fix(06-import-replace): CodeRabbit fix-pass
f9ce18a risk(06-import-replace): Phase 6 process-tree audit PASS-WITH-ADVISORY
2201a57 feat(06-import-replace): Phase 6 — tests + agents session import-replace
12c5b20 contract(06-import-replace): Phase 6 Step 6a contract
[Phase 5/4/3/2.5 commits]
```

## Net value retained
- 14 problem-map / audit entries retired (per Rev 4 supported-surface review).
- Cohort A (harness): stable `agents session import-replace` CLI with 17-step atomic flow, sentinel-flock + atomic-rename lock, replace_journal crash recovery (orphan canonical handling included), exit-code namespace 0/1/2/10/11/12/13/14/15.
- Forward-compat: when sibling 06 features land, the local `internal::` types either get replaced or extended; no breakage of public CLI receipt shape.

ADVANCE to Phase 9.
