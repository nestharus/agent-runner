# 06-schema-probe — Multi-Concern Review (Phase 8)

**Verdict: SINGLE_CONCERN**

The diff lands exactly one user-visible capability — `agents session
schema-probe` — together with the minimum supporting surface required
to make that command compile, run, and emit its contracted JSON. No
component of the diff is independently shippable, none is a drive-by
refactor, and every product change traces to a Rev 2 proposal clause.

## Concern map

| Component | User-visible? | Required for capability? | Could be split? |
| --- | --- | --- | --- |
| `schema_probe` module + `Subcommands::Session::SchemaProbe` dispatch (`src-tauri/src/main.rs`, `src-tauri/src/schema_probe/mod.rs`) | Yes — the command itself. | Definitional. | No. |
| `StateDb::open_read_only` + `ReadOnlyOpenError` enum (`src-tauri/src/state/db.rs`) | No (Rust API). | Yes — proposal §6 / contract §2.1 mandate it as the *only* permitted DB open path for the probe (§8 side-effect contract forbids the mutating `StateDb::open`). | No — splitting it lands dead code (initiative 06-session-override-contract.md:118-120 explicitly assigns this API to schema-probe). |
| `default_path()` extraction + `connection()` accessor on `StateDb` | No. | Yes — the probe needs a path resolver that does not create the data dir, and the inspection helpers need read-only SQL access. | No (trivially small; no value as a standalone PR). |
| `Subcommands::Session` parent enum (`src-tauri/src/main.rs:156-176`) | Yes — introduces the `session` group. | Yes — schema-probe lives under it (proposal §2). The same group is owned jointly with 06-locate; whichever lands first introduces it (proposal §2 line 87-98, contract §1.1). | No — proposal §2 explicitly forbids top-level aliases; the group is the entry point for the command. |
| `build.rs` `BUILD_COMMIT` injection | No (build-time). | Yes — proposal §3 makes `binary.commit` a required JSON field; §4 step 3 forbids shelling out to `git` at runtime, forcing compile-time embedding. | No — without it the Rev 1 contract cannot be satisfied. |
| Planning artifacts (`proposals/`, `research/`, `risk/`) | No. | Workflow-required for this initiative; phases 2.5–6 each produce a tracked artifact. | No — splitting them severs the audit chain. |
| Tests + fixtures (`src-tauri/tests/initiative_06_schema_probe.rs`, `src-tauri/tests/fixtures/initiative_06_schema_probe.rs`) | No. | Yes — Phase 6 Step 6b deliverable for T1–T8. | No — tests must land with the code they exercise. |

## Why not MULTI_CONCERN_RECOMMEND_SPLIT

A split would have to choose one of:

1. **Land `open_read_only` separately first.** Produces a public API
   with no caller — a violation of the project's "no half-finished
   implementations" norm and explicitly assigned to schema-probe by
   the initiative (line 118-120). Rejected.
2. **Land `Subcommands::Session` parent first.** A `session` group
   with no children is a clap usability regression (`agents session`
   exits 2 with no listed subcommands). Rejected.
3. **Land `build.rs` `BUILD_COMMIT` separately.** It has no consumer
   outside `binary.commit`; isolating it produces an unused env var.
   Rejected.
4. **Land planning artifacts separately.** Workflow already does this
   (commits `2be658d`…`b81bb93` are pre-implementation); the Phase 6
   commits (`be66405`, `7bdc4ee`, `3385efb`) are the implementation
   trio and are correctly grouped.

Each candidate split produces a PR that is either non-functional in
isolation, dead code, or a regression. The diff's nine commits
already reflect the correct intra-PR phasing.

## Why not MULTI_CONCERN_ACCEPTABLE

The label fits when distinct concerns land together for pragmatic
reasons (e.g., shared infra refactor + feature). Here every non-test
product hunk is *causally required* by the schema-probe contract —
the supporting surface is not "additional concerns bundled in," it is
the feature's irreducible substrate.

## Residual notes

- `state/mod.rs` re-exports `BinaryInfo`, `FeatureMap`,
  `SchemaProbeReport`, `StateDbReport` from `schema_probe`. This is a
  minor API-surface choice (re-export through `state` vs. consume
  through `schema_probe` directly) but does not constitute a separate
  concern.
- `connection()` is `pub(crate)` and is only consumed by
  `schema_probe::mod.rs`; if that consumption pattern is ever
  generalized, a follow-up could narrow it, but it is in-scope here.
- No retrofit of `agents trace` or other read-intent commands to the
  new read-only open (D7 / proposal §7) — correctly out of scope.

**Recommendation:** ship as one PR.
