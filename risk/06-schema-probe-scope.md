# 06-schema-probe — Phase 4 scope risk gate (claude-opus)

**Verdict: LOW (one MEDIUM finding to clarify before Phase 5).**

The proposal stays inside the initiative's lane: one new `agents session
schema-probe` subcommand, one new read-only `StateDb` open variant
(`open_read_only` / `open_ro`), and small probe-only inspection helpers.
The seven D-decisions all answer questions raised by the harness ask
(`05-session-schema-probe.md`) or by Initiative 06's cross-feature
constraints; none of them quietly add adjacent work. D7 is an explicit
contraction (no retrofit of `trace`, etc.), §6 does not refactor
`StateDb` beyond the new variant + helpers, §7/§8 cover every harness
and initiative anti-scope item with one minor implicit-only gap, and the
test-intent track in §9.1 verifies the probe rather than re-proving
existing schema-ensure correctness.

The single non-trivial scope ambiguity is whether mutating schema-ensure
paths gain `PRAGMA user_version` stamping in *this* PR — §1 / §3.1 use
"may stamp" while §11 implies a future stamper. That is the only place
the diff could quietly grow. Flagged as MEDIUM for clarification.

## Scope-direction analysis

| Question | Direction | Notes |
| --- | --- | --- |
| S1 — single command + read-only open variant | within lane (one ambiguity) | §1 names: `session schema-probe`, `StateDb::open_read_only`, inspection helpers, schema-version constants. Constants are binary-side and read-only. The "mutating schema-ensure paths may stamp `user_version`" wording (§1, §3.1, §11) is the one place the PR could grow into write-side schema-ensure code. See M1. |
| S2 — D-decisions vs harness ask | within lane | D1 (`PRAGMA user_version`), D2 (hardcoded feature map), D3 (read-only open semantics), D4 (`safe_for_import_replace`), D5 (storage vocabulary), D6 (exit codes) all answer fields/codes named in `05-session-schema-probe.md`. D7 is a contraction. None silently add a new surface. |
| S3 — no retrofit of existing commands | correctly held | §1 ¶3, §7 D7, §9.1 D7 row, §13 line "Read-only `StateDb` open variant lands in 06-schema-probe". `trace`, `repl`, `resume`, `--resume`, `migrate-db`, `migrate-config`, `resume-list`, GUI Tauri callers all continue to use mutating `StateDb::open`. |
| S4 — §6 API stays narrow | within lane | New surface = `default_path()`, `open_read_only(&Path) -> Result<Self, ReadOnlyOpenError>`, `user_version(&self)`, `inspect_session_schema(&self)`. No mode flag on `StateDb`, no shared helper extraction from `open`, no public API churn on existing methods. `default_path()` is a small split-out so the probe can resolve without opening; consistent with the harness ask for a no-side-effect path. |
| S5 — anti-scope coverage vs harness §Anti-scope and initiative cross-feature anti-scope | within lane (one implicit-only gap) | Harness items: locate/export/replace ✓ (§7), repair/migrate ✓ (§7 "No DB repair/migration"), third-party `state.db` writes ✓ (§7). Initiative items: auto-resume ✓, provider spawn ✓, quota refresh ✓, config edits ✓, `migrate-config` coupling ✓ (all in §7 + §13). One implicit-only gap: harness "does not expose provider secrets or raw transcript contents" — §8 forbids transcript reads, but config-secret reads are not explicitly forbidden in §7/§8. The §3 JSON shape contains no secret-bearing fields, so this is excluded by construction, just not by named anti-scope. See L1. |
| S6 — test-intent track scope | within lane | §9.1 tests probe-side observables only: PRAGMA authority, feature map shape, no-side-effect on legacy fixtures, WAL read behavior, predicate truth table, storage vocabulary, exit-code mapping, no-retrofit static check, side-effect contract, README truthfulness. None re-prove Initiative 04/05 ensure or backfill correctness; the version-2 / version-3 fixture rows test the *comparison*, not the migration. |

## Findings ≥ MEDIUM

### M1 — `PRAGMA user_version` stamping work is ambiguous in this PR

§1: "Mutating schema-ensure paths **may** stamp `PRAGMA user_version`;
the probe only reads it." §3.1: same "may" wording. §11: "existing DBs
have `user_version = 0` until a new mutating schema path stamps the
current version after ensuring schema." §12 lists the unstamped state as
a residual. §13's compliance checklist has no row for stamping.

Two readings are both consistent with the proposal text:

1. **Stamping ships in this PR** — `StateDb::open` (or
   `ensure_*_schema`) gains a `PRAGMA user_version =
   CURRENT_SCHEMA_VERSION` write after schema-ensure completes. That is
   write-side code outside the "read-only open variant" lane the
   initiative explicitly assigned to this feature
   (`initiatives/06-session-override-contract.md:118-120`).
2. **Stamping is deferred** — only the probe + constants land. Every
   currently-installed DB then reports `user_version = 0` and fails the
   `MINIMUM_SUPPORTED_SCHEMA_VERSION = 3` check, so `compatible` is
   `false` for every real-world DB until a follow-up PR teaches some
   write path to stamp.

The current "may" phrasing leaves the question for Phase 5/6. Either
answer is defensible, but the choice changes the diff's blast radius
(touching `StateDb::open` / `ensure_*_schema` vs. not). Phase 4 cannot
verify the lane boundary without that decision being explicit.

## LOW nits

- **L1 — Provider-secret anti-scope only implicit.** §7 names config
  edits and transcript reads as forbidden but does not explicitly forbid
  *reading* config files / provider credentials. The §3 schema contains
  no secret fields, so secrets cannot leak through the documented JSON;
  still, the harness anti-scope line "does not expose provider secrets"
  has no direct counterpart in §7/§8. Adding "no config or credential
  reads" to §7 would close the implicit-only gap.
- **L2 — `default_path()` is a small additive split-out of
  `open_default()`.** Strictly within the harness ask ("no
  side-effect path to ask where the default state DB is",
  problem-map §6 #6), but worth naming as a mini-API addition so Phase 5
  hookpoints don't treat it as zero-cost.
- **L3 — D5 storage-vocabulary duplication is acknowledged.** §3.3 and
  §12 already note that if 06-locate has not landed, schema-probe
  defines its own `claude_code` / `codex_session` / `other` enum.
  Initiative-level convention `no-backwards-compatibility.md` is
  consistent with this; reuse on merge is correctly framed as a Phase 5
  hookpoint decision, not a v1 dependency.
- **L4 — `inspect_session_schema` returns a structured probe-only type.**
  §6.2 keeps the helper non-mutating and probe-shaped; naming is left to
  Phase 5. No drift toward a general-purpose `StateDb` introspection
  API.
- **L5 — README updates (§10) are framed as documentation, not as a
  separate user-facing surface change.** Stays within the harness ask
  ("README documents the JSON shape and refusal semantics").
