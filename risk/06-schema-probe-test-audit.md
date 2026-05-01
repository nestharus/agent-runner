# 06-schema-probe — Phase 8 Test Audit

Verdict: **PASS**

## Scope

1. Gate: Test Audit per `~/ai/workflows/pr-review.md`.
2. Branch diff reviewed: `git diff main..06-schema-probe -- src-tauri/`.
3. Contract reviewed: `research/06-schema-probe-contract.md`.
4. Proposal reviewed: `proposals/06-schema-probe.md` Rev 2.
5. Supported-surface report reviewed: `risk/06-schema-probe-supported-surface.md`.
6. Audit history reviewed: `risk/06-schema-probe-audit-history.md`.
7. Process-tree audit reviewed: `risk/06-schema-probe-process-tree-audit.md`.
8. Test files reviewed:
9. `src-tauri/tests/initiative_06_schema_probe.rs`.
10. `src-tauri/tests/fixtures/initiative_06_schema_probe.rs`.
11. `src-tauri/tests/fixtures/mod.rs`.

## Firstness Routing

12. The Phase 6 process-tree audit now exists and returns **PASS**.
13. Evidence: `risk/06-schema-probe-process-tree-audit.md`.
14. It verifies Step 6b and Step 6c as separate invocations.
15. Step 6b invocation: `e150fb81-1bac-40b8-aa73-84d26f32f992`.
16. Step 6c invocation: `f21a6aeb-fc0f-4a7e-87cb-963b05234ff4`.
17. It verifies the Step 6b output index exists.
18. Evidence: `.tmp/phase6/step6b-output-index.md`.
19. It verifies Step 6b output paths exist for the test file, fixture file, and fixture module.
20. It verifies Step 6b risk annotations are present for T1-T8.
21. It verifies Step 6c consumption evidence exists.
22. Evidence: `.tmp/phase6/step6c-reads.md`.
23. The Step 6c read-evidence mtime is `2026-04-30 23:20:17 -0700`.
24. The process-tree audit reports earliest observed product-code mtime as `src-tauri/src/lib.rs` at `2026-04-30 23:23:01 -0700`.
25. That satisfies the required read-before-product-code firstness ordering.
26. The process-tree audit accepts later Phase 7 CodeRabbit edits as non-invalidating.
27. It verifies Step 6c product outputs are tracked.
28. It verifies targeted schema-probe tests passed.
29. It verifies the full Rust suite passed.
30. It reports no blocking, advisory, or needs-input process violations.
31. Firstness routing result: **PASS**.

## Contract Mapping

32. T1 maps to `schema_probe_current_schema_db_emits_compatible_report`.
33. Evidence: `src-tauri/tests/initiative_06_schema_probe.rs:8`.
34. The test asserts exit `0`, single-line compact stdout JSON, binary fields, state path, schema/user/current/min versions, compatibility, nested maps, feature map, storage vocabulary, and `safe_for_import_replace: false`.
35. T2 maps to `schema_probe_missing_db_emits_non_mutating_success_report`.
36. Evidence: `src-tauri/tests/initiative_06_schema_probe.rs:65`.
37. The test asserts exit `0`, no stderr, no default state directory creation, `exists: false`, version `0`, incompatible state, false structural maps, no dotted compatibility keys, and safe false.
38. T3 maps to `schema_probe_old_user_version_exits_schema_incompatible`.
39. Evidence: `src-tauri/tests/initiative_06_schema_probe.rs:93`.
40. The test asserts exit `14` and stderr JSON code `schema-incompatible`.
41. Proposal D6 maps to newer-schema and wrong-index regression tests.
42. Evidence: `src-tauri/tests/initiative_06_schema_probe.rs:115` and `src-tauri/tests/initiative_06_schema_probe.rs:130`.
43. These cover future user versions and matching-name/wrong-definition indexes.
44. T4 maps to `schema_probe_unreadable_db_exits_operational_error`.
45. Evidence: `src-tauri/tests/initiative_06_schema_probe.rs:145`.
46. The test asserts exit `1`, stderr JSON code `operational-error`, and a message containing `state.db`.
47. T5 maps to `open_read_only_preserves_existing_db_physical_snapshot`.
48. Evidence: `src-tauri/tests/initiative_06_schema_probe.rs:167`.
49. T5 also maps to `open_read_only_missing_path_does_not_create_parent_directory`.
50. Evidence: `src-tauri/tests/initiative_06_schema_probe.rs:184`.
51. Together they assert no mtime/length/sidecar mutation and no parent directory creation.
52. T6 maps to five component classifier tests.
53. Evidence: `src-tauri/tests/initiative_06_schema_probe.rs:203`.
54. Covered variants: `Missing`, `NotADatabase`, `PermissionDenied`, `WalSidecarError`, and `Operational`.
55. T7 maps to `schema_probe_report_safe_for_import_replace_predicate_follows_inputs`.
56. Evidence: `src-tauri/tests/initiative_06_schema_probe.rs:320`.
57. The test covers the all-true case plus import-replace disabled, pause disabled, missing DB, incompatible DB, and incomplete storage vocabulary.
58. T8 maps to `schema_probe_report_serializes_nested_compatibility_maps`.
59. Evidence: `src-tauri/tests/initiative_06_schema_probe.rs:348`.
60. The test asserts nested `required_columns` and `required_indexes`, flat `tables`, and absence of dotted keys.
61. Semantic T1-T8 mapping is complete.

## Risk Annotations

62. Every test or test group has an inline risk annotation.
63. Every test or test group has an explicit level annotation.
64. Every test or test group cites a source row or proposal section.
65. Every test or test group records an observable signal.
66. Every test or test group records a residual.
67. Additional D6 tests are annotated to proposal §5/§9.1 rather than contract T rows.
68. The annotation requirement is satisfied.

## Validator Level

69. T1-T4 use binary-spawn particular-integration tests where CLI exit/stdout/stderr behavior is load-bearing.
70. T5-T6 use direct component tests where `StateDb::open_read_only` behavior is the target API.
71. T7 uses a unit-level report-construction predicate test, the cheapest reliable validator for the boolean rule.
72. T8 uses a unit-level serialization test, the cheapest reliable validator for map shape.
73. Validator levels are appropriate.

## Fixture Externality

74. Fixtures are external to test bodies in `src-tauri/tests/fixtures/initiative_06_schema_probe.rs`.
75. `src-tauri/tests/fixtures/mod.rs` exposes only the dedicated fixture module.
76. Test bodies call named fixtures such as `current_schema_db_fixture`, `missing_db_fixture`, and `wal_sidecar_error_fixture`.
77. Durable setup details live in fixture helpers, not inline test bodies.
78. The fixture module owns temp dirs, XDG env redirection, seeded SQLite schemas, chmod setup, sidecar setup, and JSON assertions.
79. Fixture state is per-test via `tempfile::TempDir`.
80. The WAL sidecar fixture intentionally leaks a connection to preserve sidecars; audit history records CodeRabbit R2-F04 as churn for that fixture comment.
81. No hidden shared durable fixture state was found.
82. No fixture-in-test-body loophole was found.

## Assertion Discipline

83. The test diff under `src-tauri/tests` is additive: 898 insertions, 0 deletions.
84. No regenerated baseline was found.
85. No removed coverage was found.
86. No narrowed input-space edit was found in existing tests.
87. No risk annotation removal was found.
88. Assertions are specific enough to fail on wrong exit codes, wrong JSON channels, missing structural keys, dotted compatibility keys, wrong feature values, wrong storage vocabulary, and wrong enum variants.
89. The tests do not merely assert command success.
90. The predicate fixture duplicates the production predicate shape, but it uses controlled report inputs and still checks the constructed report's value.
91. This is acceptable for the unit-level predicate test; T1 validates the real CLI disabled-feature outcome.

## Residuals

92. Unix-only test module means Windows permission behavior remains outside this target.
93. This is consistent with inline residuals and the contract's platform-variance caveat.
94. Structural incompatibility is not exhaustively fuzzed; the wrong-index and old/future-version tests cover representative high-risk cases.
95. The tests do not prove future feature-map maintenance after sibling commands land.
96. This is a forward-review residual already named in proposal §9.1.
97. None of these semantic residuals appears to collapse the supported-surface net value.
98. Supported-surface termination signal remains none.

## Verification

99. Ran `cargo test --manifest-path src-tauri/Cargo.toml --test initiative_06_schema_probe`.
100. Result: PASS, 15 passed, 0 failed.
101. Process-tree audit reports `cargo test --manifest-path src-tauri/Cargo.toml` passed with 397 tests.
102. Process-tree audit reports `cargo fmt --manifest-path src-tauri/Cargo.toml --check` passed.
103. Ran `git diff --check main..06-schema-probe -- src-tauri`.
104. Result: no whitespace errors.

## Decision

105. Phase 6 firstness evidence: **PASS** after process-tree audit.
106. Semantic test coverage: **PASS** for T1-T8 mapping.
107. Fixture externality: **PASS**.
108. Risk annotations: **PASS**.
109. Validator-level appropriateness: **PASS**.
110. Assertion discipline: **PASS**.
111. No relaxed assertions, regenerated baselines, removed coverage, narrowed input space, or risk-annotation removal found.
112. Final verdict: **PASS**.
