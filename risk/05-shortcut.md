# Shortcut Risk Assessment: proposals/05-session-migration.md (rev 4)

## Verdict: LOW

Rev 4 is a Codex-deferral pass: §6 returns a typed
`MigrationError::CodexMigrationDeferred` for any Codex source/target,
§7 deletes the `kind = "config"` resume strategy entirely, the
`zstd` crate dependency is dropped, and §11/§13.1 lose their
Codex-`.zst` and `experimental_resume` test/surface coverage in
exchange for Codex-deferred and Codex-chain-identity coverage.
None of these edits introduce a hidden shortcut. The deferral
satisfies `~/ai/conventions/no-deferred-stubs.md` (typed error,
named follow-up, pinned negative test) and the `kind = "config"`
removal satisfies `~/ai/conventions/no-backwards-compatibility.md`
(deleted, not aliased / re-exported / `#[deprecated]`-tagged).
Rev 3's LOW carries forward; Rev 2's F1 (legacy fallback
prohibition) and F2 (silent offset=0 fallback fix) remain
resolved because Rev 4 does not edit §4, §6.6, §13, or §14
around those anchors.

Rev 4 introduces one new audit/test-audit watchpoint: §11.1's
"Resume strategy compatibility" row at line 633 cites a
`compose_resume_args_*` test family, but §11.2 does not actually
list a test by that name. This is a coherence gap in the same
shape as the Rev 3 `[migrate]` stderr observation (now closed by
§13.1 line 776's explicit emit-site) — a thin gap, not a hidden
shortcut, since the rest of §6 / §11 / §13.1 preserves the
behavior the row would pin. Flag for audit/test-audit, does not
change shortcut verdict.

## Findings (severity >= medium)

None.

## Rev 2 finding resolution status (re-validation under Rev 4)

### F1 (Rev 1 MEDIUM): `find_provider_for_session` legacy fallback — STILL RESOLVED

Rev 4 does not edit §1, §4, §13, or §14 around the four
`find_provider_for_session` anchors (§1 line 17, §4 line 198,
§13 line 724, §14 line 852). The Rev 4 amendments touch §6, §7,
§9.1, §11, §13.1, §15 — none reintroduce the legacy fallback or
weaken the §14 prohibition. §13.1 line 766's rollback paragraph
("the prior binary's `find_provider_for_session()` still works
against unmodified `session_turns`") is a forward-direction
rollback statement, not a back-channel for keeping the function
alive in the current binary.

**STILL RESOLVED.**

### F2 (Rev 1 MEDIUM): §6.6 step 3 silent fallback — STILL RESOLVED

Rev 4 does not edit §6.6 step 3, §6 step 4, or §11.2's
`migration_errors_when_compaction_boundary_not_in_jsonl` test.
The compaction-aware target build is now Claude-only (§6.6 line
421: "Codex migration is deferred in v1, so the compaction-aware
target build is only exercised for Claude-Code plaintext JSONL"),
but the hard-error contract on the Claude path is preserved
verbatim — both raise sites (§6 step 4 line 363; §6.6 step 3
line 408) and the test (§11.2 line 671) are untouched.

**STILL RESOLVED.**

## Per-pattern evidence (eight required checks, re-run against Rev 4)

### 1. Per-call chain caching

Resolver SQL (§4 step 2) and `decide_migration` (§5) read fresh
per call. No `OnceCell` / `lazy_static` / `Mutex` / static
introduced by any Rev 4 edit. Grep
`cache|memo|lazy_static|OnceCell|RefCell|Mutex`: 4 hits, all
references to prompt cache cost (§1.1 A2, §6.5, §12, §14). No
Rust caching primitives. PASS.

### 2. Background migration loop

No timer / interval / scheduled task. Migration only at resume
time per explicit user invocation. Codex deferral is a typed-
error path, not a "retry later" loop. Grep
`timer|retry|backoff|periodic|sleep|wait|schedule|re-probe`: 1
hit ("user reissues `--migrate` to retry") — manual, unchanged.
PASS.

### 3. Threshold gating in disguise

The 95% threshold (§5 step 6) gates only "should this existing
chain migrate to a sibling." §13.1 line 761 says
`compute_projections` math is "claimed bit-for-bit equivalent."
§5 line 322 (Rev 4 addition) handles the Codex active-provider
case: `decide_migration` may return `Migrate` so the normal
threshold/manual policy is observable; the §6 mechanic then
raises the typed error. **The threshold itself is unchanged** —
Codex active provider gets a typed error from the mechanic, not
a different threshold. §1.1 A4 still pins the extraction-
equivalence assumption with named invalidator. PASS.

### 4. Chain-stickiness bypass

§4 step 4 picks active segment by `ORDER BY started_at DESC LIMIT
1` filtered to `ended_at IS NULL`. §4.1 — "always exit 1 — never
auto-pick on ambiguity". Rev 4's Codex deferral does not
introduce auto-resolution: failed Codex `--migrate` returns the
typed error, leaves the active segment alone, user must re-run
with a different target. PASS.

### 5. Backwards-compat shim for `find_provider_for_session`

See F1 above. Rev 4 adds no new compat-shim language. Grep
`compat|shim|backward|legacy|transitional|alias`: 3 hits — §4 /
§14 refusal, §13 line 724 "deleted, not deprecated", and a
descriptive Rev 4 changelog reference. Zero violations. PASS.

### 6. Session_id ghost identity

§10 — `chain_id` alongside `session.id`; existing fields
preserved. §13 — `resume_acceptance` (segment-scoped) explicitly
distinguished from chain ledger. §3.3 — `last_used_at` lives on
`session_chains`. §1.1 A8 reaffirms session_id is per-segment.
Rev 4 specific: `chain_mint_works_for_codex_ingestion` pins that
Codex chains keep their chain_id and segment ledger separately
from session_id, even though file-copy migration is deferred.
PASS.

### 7. Dual-write of model arg

§3.1 records `model_name` in `session_chains` at chain mint (one
site). §4 step 5 reads in deterministic precedence (§1.1 A8:
latest invocation → chain model → provider default →
`ResumeError::ModelInferenceImpossible`). §11.2's
`agent_session_chain_records_model_at_mint`,
`agent_resume_no_dash_m_uses_session_recorded_model`, and the
`resolve_resume_falls_back_*` family pin the precedence
ordering. Rev 4 does not introduce a parallel write site. PASS.

### 8. `is_compaction_boundary` zombie

§6.6 actively reads. §3.4 plumbs end-to-end through `ScriptTurn`,
`SessionTurnIngest`, both `INSERT OR IGNORE INTO session_turns`
SQL statements. §11.1 row "`is_compaction_boundary` ingest
plumbing" cites both
`turn_script_optional_compaction_field_defaults_false` and
`turn_script_compaction_field_propagates_to_session_turns` —
column non-zombie pinned at parse and DB layers.

The Rev 4 `codex-turns` non-update at §9.1.1 line 569 does NOT
make the column a zombie on the Codex side: Codex chain identity
uses the column's default (`0`), the migration mechanic that
reads it is Claude-only and raises the deferred typed error
before reading it for Codex chains. The column is live for the
Claude path it is built for. PASS.

## Specific shortcut traps (re-validated under Rev 4)

- **Migration on resume-without-need**: §5 step 7 below threshold
  → `Stay`. Single-provider, no-storage-sibling, and worse-
  sibling pools all `Stay`. `--migrate` is per-call only. Rev 4
  adds `decide_migration_returns_codex_deferred_for_codex_provider`
  pinning "Codex active + Claude-Code sibling → `Migrate` then
  mechanic raises typed error" and "Codex active + no eligible
  sibling → `Stay` with logged deferred reason." PASS.
- **JSONL byte rewrite**: §6.5 plaintext `source[offset..]`
  unchanged; §6 step 11 / §14 explicit refusals of `sed`-style
  rewrites. Rev 4 removes the `.zst` round-trip test alongside
  the deleted `.zst` code path — clean delete, not coverage loss.
  Claude plaintext byte-equality remains pinned by
  `migration_copies_claude_jsonl_to_target_projects_dir` and
  `migration_truncates_target_jsonl_at_latest_compaction_boundary`.
  PASS.
- **Chain merging**: §4.1 always-exit-1 contract preserved;
  `resolve_resume_errors_ambiguous_when_both_recent` pins it.
  No Rev 4 edit. PASS.
- **Compaction-aware target build "skip"**: F2 still resolved;
  both raise sites and `migration_errors_when_compaction_boundary_not_in_jsonl`
  preserved verbatim. Rev 4 narrows to Claude only, contract
  unchanged. PASS.
- **Provider session_storage fallback**: §9.1 still requires both
  Claude-Code source and target to declare storage. Rev 4 §9.1
  line 540 makes Codex storage identity-only; migration trigger
  on a Codex chain raises the typed error regardless of source/
  target storage declarations. PASS.
- **Default_model fallback as forced default**: §4 step 5 raises
  `ResumeError::ModelInferenceImpossible` on exhaustion of all
  four sources; §1.1 A8 names the resolution chain, not a forced
  default. PASS.

## Rev 4-specific shortcut trap evaluations

### A. Codex deferral as deferred-stub

Per `~/ai/conventions/no-deferred-stubs.md`, deferred work needs
(1) a typed error, not a silent stub, (2) a named follow-up
work unit, (3) a test that asserts the deferred state.

| Requirement | Rev 4 evidence | Pass? |
|---|---|---|
| Typed error, not silent skip | `MigrationError::CodexMigrationDeferred { provider }` referenced at §1 line 23, §6 step 1 line 351, §6 step 3 line 361, §9.1 line 540, §11.2 line 667 (test), §15 line 866 | Yes |
| Errors loudly, doesn't fall through to partial migration | §6 step 1 line 351: "v1 supports Claude-Code migration only ... but cross-account file copy is deferred per §15." The error is returned BEFORE any target file is touched. §11.2 test (`migration_mechanic_errors_codex_deferred_on_codex_active_provider`) explicitly asserts "no target file/segment is written." | Yes |
| Concrete §15 entry with named unblocker | §15 line 866: "either wait for Codex to expose a documented path-resume mechanism, OR design a state-DB-aware migration path for Codex (couples to Codex internals; lower priority)." Cites `research/05-codex-resume-verification.md` as the verification source. | Yes |
| `[providers.session_storage] kind = "codex"` declarable but limited | §9.1 line 540 explicitly documents the v1 limitation: chain identity yes, migration no. README §12 line 705 also documents: "`kind = "codex"` is declarable for chain identity but migration is deferred in v1." | Yes |
| Negative test pins the deferred state | §11.2 line 667 `migration_mechanic_errors_codex_deferred_on_codex_active_provider`; §11.2 line 665 `decide_migration_returns_codex_deferred_for_codex_provider` also pins the decision-side behavior. Removing the deferred guard breaks both tests. | Yes |

Notice the convention's "Test the deferred stub as deferred"
clause is satisfied by two tests at two different layers
(decision and mechanic), not just one. That makes the deferred
contract harder to silently weaken.

PASS.

### B. Codex chain identity as orphan

Rev 4 keeps Codex chain identity (chain mint at ingestion,
segment ledger). The risk is that this is half-finished — chain
rows mint but nothing reads or maintains them.

| Behavior | Pinned by | Pass? |
|---|---|---|
| `chain_mint_works_for_codex_ingestion` test | §11.2 line 655: "ingest a Codex turn for a fresh `(provider, session_id)` pair; assert `session_chains` and `session_chain_segments` rows exist. This pins that Codex chain identity is preserved even though migration is deferred." | Yes |
| Codex chains can be resumed by id within same provider | §6 step 1 line 351: "resume-by-id within the same provider still works through Codex's native `resume` subcommand". §9.1 line 540 reinforces. §7's `kind = "subcommand"` for Codex is preserved (line 458 example). | Yes |
| `[migrate]` log line is NOT emitted for Codex chains | §13.1 line 776: "The `[migrate]` ... line is emitted on stderr from the migration helper (§6 step 6, after the segment row is opened and before §6 step 7 composes target argv)." For Codex, §6 step 1 / step 3 raise the typed error BEFORE step 6. The emission is therefore gated on actually opening a segment, which Codex never does. Behaviorally consistent. | Yes (by ordering) |
| `agents resume --list <UUID>` works for Codex chains | §8.5 line 490: "Diagnostic-only: list all chains matching the input session_id with their previews. Reuses the resolver's preview-building code path. Always exits 0; does not spawn anything." The resolver is storage-kind-agnostic; it reads `session_chain_segments` directly. Codex chains appear in the list. | Yes |

The one soft spot: there is no §11.2 test that asserts the
`[migrate]` line is NOT emitted on a Codex deferred path.
`migration_mechanic_errors_codex_deferred_on_codex_active_provider`
pins absence of target file/segment but does not pin absence of
stderr substring. This is the same shape as the Rev 3
observability watchpoint and is reflected below.

PASS (with one observability test-audit watchpoint, see
Implementation-risk notes).

### C. `kind = "config"` removal — clean delete

Per `~/ai/conventions/no-backwards-compatibility.md`, removed
code is gone, not deprecated:

| Anti-pattern | Rev 4 search result | Pass? |
|---|---|---|
| `ResumeStrategyKind::Config` enum variant present | §7 line 428 — only `Flag` and `Subcommand` variants; the omission is not a `#[deprecated]` keep | Yes |
| `ConfigArgument` enum present | §7 line 440: "The `ConfigArgument` enum and `ResumeStrategyKind::Config` variant are NOT introduced in v1." | Yes |
| `#[deprecated]` / type alias / re-export | Grep against the proposal returns no hits | Yes |
| Test for the strategy still listed | §11.2 search: `migration_composes_codex_experimental_resume_argv` is absent. Rev 3 had this test; Rev 4 deletes it. | Yes (deleted, not skipped) |
| `kind = "config"` mentioned only as refusal | §1 line 25 ("drop `kind = "config"`"), §12 line 706 ("no `kind = "config"` strategy ships in v1"), §13.1 line 766 ("Rev 4 removes the `kind = "config"` resume strategy"). All refusals. | Yes |

`experimental_resume` mentions in the proposal (§1 line 25,
§1 line 27, §1.1 A7 invalidator phrasing) all reference the
*non-existence* of the key per
`research/05-codex-resume-verification.md`. None propose using
it.

PASS.

### D. `compose_resume_args(target_jsonl_path)` parameter

The parameter is reserved for the deferred Codex follow-up but
only Claude callers use it in v1.

| Concern | Rev 4 evidence | Pass? |
|---|---|---|
| Parameter is `Option<&Path>`, not `&Path` | §7 line 448: `target_jsonl_path: Option<&Path>` | Yes (non-migrating callers pass `None`) |
| No dead code that consumes the parameter for non-Claude paths | §7 line 452: "In v1, the only migration path that passes a target path is Claude-Code JSONL copy" | Yes |
| Reserved-for-deferred-follow-up explicit at decl site | §7 line 452: "The parameter is reserved for the deferred Codex migration follow-up — see §15." | Yes |
| Parameter is actually used today | Claude migration (§6 step 5) plumbs the target path through, but today's `flag` arm ignores it (composes `--resume <session_id>`). The parameter is plumbed but no arm of `ResumeConfig` consumes it. | Borderline |

This is the soft-tension row. `~/ai/conventions/no-deferred-stubs.md`
forbids placeholder stubs that return `None`/`{}`/`[]`, not every
signature parameter that lacks a consuming arm. The Rev 4 design
is honestly described as deferred-use, and §11.1 row 633 pins
behavioral invariance (adding `Some(path)` must not change
existing argv). The cleaner alternative would be to add the
parameter only when the Codex follow-up lands; eager plumbing is
a stylistic call, not a convention violation when explicitly
documented and regression-pinned.

PASS (documented soft tension, not a violation).

### E. Removed `.zst` and `zstd` dependency

Rev 4 drops the `zstd` crate dependency and the
decompress→slice→recompress pipeline.

| Anti-pattern | Rev 4 search result | Pass? |
|---|---|---|
| `decompress → slice → recompress` pipeline language outside the changelog | §1 line 12 references this pattern only as "Superseded by Rev 4". §6.5 says "**Plain JSONL only** (`kind = "claude_code"`)". No surviving description of the `.zst` pipeline. | Yes |
| `zstd = "0.13"` Cargo.toml line in §6.5 or §1 | Grep returns no hits anywhere in the proposal (the historical pre-Rev-4 mention now lives in §1's changelog only) | Yes |
| `migration_zst_round_trip_preserves_post_offset_bytes` test | §11.2 list: not present (Rev 3 had it; Rev 4 removes it alongside the code path it pinned) | Yes (deleted, not skipped) |
| `migration_copies_codex_rollout_with_zst_extension` | §11.2 list: not present (Rev 3 had it; Rev 4 removes it) | Yes (deleted) |

Both `.zst`-targeted tests are deleted alongside the deleted
code path. That is the canonical no-backwards-compatibility
shape: when the code goes, the test goes with it.

PASS.

### F. Carry-over Rev 2 traps

- **F1** (`find_provider_for_session` legacy fallback): re-confirmed
  resolved above. No Rev 4 edit reintroduces it.
- **F2** (§6.6 step 3 silent fallback): re-confirmed resolved
  above. The compaction-aware target build is now Claude-only,
  but the hard-error contract on the Claude path is preserved
  verbatim and `migration_errors_when_compaction_boundary_not_in_jsonl`
  still pins it.

PASS.

## Per-pattern grep summary (Rev 4)

| Pattern | Hits | Interpretation |
|---|---|---|
| `cache\|memo\|lazy_static\|OnceCell\|RefCell\|Mutex` | 4 | All references to prompt-cache cost (§1.1 A2, §6.5, §12, §14). No Rust caching primitives. |
| `timer\|retry\|backoff\|periodic\|sleep\|wait\|schedule\|re-probe` | 1 | "user reissues `--migrate` to retry" — manual, unchanged. |
| `5 min\|timeout\|expire\|ttl` | 0 | No time-based clear / probe. |
| `silently\|sed\|byte rewrite` | 6 | All refusals or warnings, unchanged from Rev 3. |
| `compat\|shim\|backward\|legacy\|transitional\|alias` | 3 | One refusal in §4 / §14; one §13 line 724 "deleted, not deprecated"; one descriptive Rev 4 changelog reference. Zero violations. |
| `find_provider_for_session` | 5 | All deletion / replacement / prohibition (§1, §4, §13, §13.1 rollback, §14). The §13.1 hit describes prior-binary continued operation under rollback, not a runtime fallback in current binary. |
| `experimental_resume` | 3 | All in §1 changelog and §1.1 A7 invalidator wording, describing absence/non-use. No live use. |
| `kind = "config"\|ResumeStrategyKind::Config\|ConfigArgument` | 4 | All refusals: §1 line 25, §7 line 440, §12 line 706, §13.1 line 766. Zero live introductions. |
| `zstd\|\.zst` | 3 | All in §1 changelog explaining removal; no live `zstd` dependency or `.zst` code path. |
| `MigrationError::CodexMigrationDeferred` | 5 | §1, §6 step 1, §6 step 3, §9.1, §11.2 test, §15. Consistent typed error across declaration, raise sites, declared limitation, test pin, and unresolved entry. |
| `migrate-db` | 5 | §1, §8.5.1, §13.1, §14, §14 — escape valve fully specified via §2 backfill loop. Unchanged from Rev 3. |
| `MigrationError::CompactionBoundaryNotInJsonl` | 4 | §1, §6 step 4, §6.6 step 3, §11.2 — F2 still resolved. |
| `\[migrate\]` | 4 | §13.1 line 774 announces; §13.1 line 776 mechanizes (helper site, ordering); §13.1 line 842 audit-amendment note. No §11.2 test pins the substring (carry-over watchpoint). |
| `compose_resume_args` | 5 | §1, §7 declaration, §7 fn signature, §11.1 row 633, §13 line 726. **No `compose_resume_args_*` test in §11.2** (new watchpoint). |

## Implementation-risk notes (not shortcut violations)

- **Rev 3 watchpoint, now half-closed: `[migrate]` stderr line.**
  Rev 4 §13.1 line 776 mechanizes the emission site (helper site
  in §6 step 6, after the segment row opens and before §6 step
  7). What remains: no §11.2 test pins the substring — neither
  a positive Claude-migration test nor a negative Codex-deferred
  test. Fix is two CLI tests, comparable to the existing
  `[resume]` line.
- **Rev 4 new: `compose_resume_args_*` test family absent.**
  §11.1 line 633 cites it under "Resume strategy compatibility"
  (existing flag/subcommand argv unchanged with `target_jsonl_path`
  added), but §11.2 has no such test. §13 line 726's signature-
  update note obligates call-site updates but doesn't pin argv
  invariance. Audit/test-audit watchpoint, not shortcut. Fix is
  one or two unit tests with `None` and `Some(path)` for Claude
  `flag` and Codex `subcommand` arms.
- **Rev 4 new: no negative-emission test for `[migrate]` on Codex
  deferred path.** `migration_mechanic_errors_codex_deferred_on_codex_active_provider`
  pins absence of target file and segment but not absence of
  stderr substring. Fix: add a `not_contains` assertion.
- **Rev 2 carry: §6.10 dual-trigger interaction**, **§3.1.1
  transactional ingest mint**, and **`compute_projections` perf
  regression risk** — all behaviorally pinned where required;
  audit-gate carries unchanged from Rev 3.

## Conclusion

Verdict: **LOW**.

Rev 4 deletes the `kind = "config"` resume strategy (clean
delete, not aliased), drops the `zstd` dependency and `.zst`
code path (clean delete, tests removed alongside), and gates
Codex migration behind `MigrationError::CodexMigrationDeferred`
(typed error, named §15 follow-up, two-layer negative tests at
decision and mechanic). Codex chain identity is preserved
through ingestion mint and segment ledger but does not exercise
the deferred file-copy path; the `[migrate]` stderr line fires
only after a segment is opened, which Codex deferred paths never
reach.

Both Rev 1 MEDIUM findings (F1, F2) remain resolved — Rev 4 does
not edit the proposal sections that carry those resolutions.

Rev 4-specific evaluation:

- **A. Codex deferral as deferred-stub**: typed error, named
  follow-up, two negative tests, declared v1 limitation in §9.1
  / §12. PASS.
- **B. Codex chain identity as orphan**: mint pinned by
  `chain_mint_works_for_codex_ingestion`; same-provider resume
  preserved; `[migrate]` ordering guarantees no emission on
  deferred path; `agents resume --list` is storage-agnostic.
  PASS.
- **C. `kind = "config"` removal**: enum, argument, test, and
  TOML all deleted (not deprecated/aliased/re-exported). PASS.
- **D. `compose_resume_args` parameter**: `Option<&Path>`
  explicitly documented as reserved-for-deferred at §7 line 452;
  invariance pinned at §11.1 line 633. Soft tension, not a
  violation. PASS.
- **E. `.zst` / `zstd` removal**: clean delete; both `.zst`
  tests removed alongside the deleted code path. PASS.
- **F. Rev 2 carry-overs**: F1 and F2 re-confirmed resolved.

Eight shortcut patterns and six trap categories pass. Two new
audit/test-audit watchpoints (§11.1 line 633 cites a
`compose_resume_args_*` test family that §11.2 doesn't list;
no negative-emission test for `[migrate]` on Codex deferred
paths). The Rev 3 `[migrate]` mechanization watchpoint is
upgraded — emission site is now located at §13.1 line 776 even
though the substring is not yet test-pinned. Carry-over audit-
gate watchpoints (§6.10 dual-trigger, §3.1.1 transactional ingest
mint, `compute_projections` perf untracked) are unchanged.

Verdict: **LOW**.
