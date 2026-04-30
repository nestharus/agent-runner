# Initiative 05 — Phase 4 Scope Risk Assessment (revision 4)

**Assessor:** `claude-opus` (scope)
**Verdict:** **LOW.** Rev 4 defers Codex migration entirely — a
scope REDUCTION, not an expansion. The deferral is well-scoped: a
single typed error (`MigrationError::CodexMigrationDeferred`)
replaces an unworkable mechanism (`-c experimental_resume`)
verified non-functional in `research/05-codex-resume-verification.md`.
Three new tests in §11.2 pin the Codex deferred path and Codex
chain-identity preservation; three obsolete Codex tests
(`migration_zst_*`, `migration_composes_codex_experimental_resume_argv`)
are correctly removed. Two of three Rev 3 drafting nits are
resolved (`[migrate]` stderr anchor, problem-map §4 enumeration);
one remains (§1.1 "narrowed" wording). One new very-low Rev 4
drafting nit found: §11.1 "Resume strategy compatibility" row
references `compose_resume_args_*` patterns but no such tests
appear in §11.2 (orphaned after Codex argv test deletion). Two
Rev 2 test carry-overs (#4.B `model_override` wins, #4.C
`last_turn_id IS NULL`) remain unchanged. Verdict matches Rev 3.

---

## 1. Rev 4 changes — scope-direction analysis

| Rev 4 change | Direction | Justification trace |
| --- | --- | --- |
| §6 Step 1/3 Codex deferral guard (`MigrationError::CodexMigrationDeferred`) | reduction + 1 typed error | `research/05-codex-resume-verification.md` ("`experimental_resume` not present in CLI source"); replaces unworkable mechanism with explicit failure |
| §6 Step 5 / §6.6: `zstd` crate not added | reduction (drop dep) | downstream of Codex deferral; `.zst` only relevant to Codex rollout files |
| §7: drop `kind = "config"` / `experimental_resume` | reduction (drop variant) | `research/05-codex-resume-verification.md` "Codex 0.125.0 ignores that key" |
| §7: `compose_resume_args()` keeps `target_jsonl_path: Option<&Path>` | unchanged signature plumbing | reserved for follow-up; v1 only Claude path uses it |
| §9.1: `kind = "codex"` retained for chain identity, ignored for migration | reduction (declarable but inert for migration) | preserves §3.1.1 UI mint and §3.3 last-used updates for Codex sessions |
| §11: 3 Codex tests deleted, 3 Codex-deferred / chain-identity tests added | replacement (net 0 in count) | each new test pins a specific Rev 4 surface; each deletion matches a removed §6/§7 surface |
| §12 README: no new sections, content of §12 line 705-706 updates the resuming-a-session bullet | minor narrowing | no README content for `kind = "config"` ever shipped; nothing to remove |
| §13.1: Codex `.zst` and `experimental_resume` removed from surface | reduction | matches §11/§9.1 |
| §13.1: `[migrate]` stderr breadcrumb anchored to §6 step 6→7 | drafting fix from Rev 3 #9.B | resolves Rev 3 anchor finding |
| §13.1: enumerate problem-map §4 #5/#7/#8/#11/#12 | drafting fix from Rev 3 #9.C | resolves Rev 3 enumeration finding |
| §13.1: 6 SQL observability queries (Q1–Q6) | drafting addition | not net-new design; consolidates ad-hoc operator queries |
| §15: broader Codex migration deferral entry citing verification doc | reduction (one residual replaces another) | replaces narrower "Codex compaction format" residual |

**Net direction: scope REDUCTION.** Every Rev 4 change either
removes scope, replaces an unworkable mechanism with an explicit
failure path, or applies a Rev 3 drafting nit. No new design
surface introduced.

---

## 2. Rev 3 nit resolution

| Rev 3 nit | Status | Evidence |
| --- | --- | --- |
| #9.A — §1.1 "narrowed" wording (register grew 6→8) | **NOT resolved** | proposal :19 and :32 still read "narrowed from problem-map §7"; A1–A8 = 8 entries; problem-map A1–A6 = 6 entries; direction is still expansion (A7/A8 derivative) |
| #9.B — §13.1 `[migrate]` stderr breadcrumb not anchored | **resolved** | proposal :776 anchors emission to "the migration helper (§6 step 6, after the segment row is opened and before §6 step 7 composes target argv)" with TTY-independent always-once semantics |
| #9.C — §13.1 omits problem-map §4 #5/#7/#8 | **resolved** | proposal :743 covers §4 #5; :745 covers §4 #7; :746 covers §4 #8; bonus :749/:750 cover §4 #11/#12 |

Two of three resolved. #9.A was within the Rev 3 LOW envelope and
remains so under Rev 4 (one-word fix; not a scoping issue).

---

## 3. Coverage matrix — Rev 4

Walked five problem items, 14 initiative scope bullets, six
Rev 2 additions, four Rev 3 amendments, six Rev 4 changes.

| Source | Coverage location | Gap? |
| --- | --- | --- |
| Problem #1–#5 | §4.5/§8.1-§8.6, §2/§3, §4.1/§4.3/§8.5, §5+§6 | none |
| Init scope (14 in-scope items) | §2, §4, §5, §6, §7, §9.1, §9.2, §4.5, §6.6, §9.1.1, §8.4–§8.5, §10 | none |
| Init scope item: "New resume strategy `kind = config` for Codex" | **Rev 4 explicitly removes this in-scope item** | controlled removal — initiative file flagged for sync; see #8.A |
| Rev 2 additions (six bullets) | §3.4, §5.1, §6 step 5, §6.6 step 3, §11.2 + §12 + §14 | none |
| Rev 3 amendments (§1.1, §1.2, §11.1, §13.1) | §1.1, §1.2, §11.1, §13.1 | drafting only — see #8.B |
| Rev 4: §6 step 1/3 Codex deferred guard | §6 + §3.1.1 + §9.1 + §15 | none |
| Rev 4: `MigrationError::CodexMigrationDeferred` | §6 step 1/3, §11.2, §13.1 | none |
| Rev 4: §7 `kind = "config"` removed | §7, §12 :706, §14 (no Codex section needed) | none |
| Rev 4: `kind = "codex"` declarable but inert for migration | §9.1, §3.1.1, §11.2 (`chain_mint_works_for_codex_ingestion`), §15 | none |
| Rev 4: §13.1 `[migrate]` anchor, §4 enumeration, SQL queries | §13.1 :776, :737-752, :778-840 | none |
| Rev 4: §15 broader Codex deferral | §15 first entry | none |

**Coverage complete.** The initiative-file scope item "New resume
strategy `kind = config`" is a known mismatch with Rev 4 — see #8.A.

---

## 4. §11 test list — Rev 4 deltas

### Tests deleted (all justified)

| Deleted | Why valid | Surface that no longer exists |
| --- | --- | --- |
| `migration_zst_*` (decompress/atomic) | `zstd` dep dropped (§6.5 / §6.6 Codex-only path deferred) | `.zst` handling removed |
| `migration_composes_codex_experimental_resume_argv` | `kind = "config"` / `ConfigArgument` removed (§7) | argv composition for `experimental_resume` no longer exists |

Tests removed match Rev 4's §11/§13.1 commitment to "remove Codex
`.zst` and `experimental_resume` tests/surface." Rev 3 §11.2 had
this `migration_composes_codex_experimental_resume_argv` test as
the sole anchor for §11.1's "config resume strategy" group; Rev 4
correctly removes it because the group's surface no longer exists.

### Tests added (all justified)

| Added | §11.2 line | Pins which surface | Justification |
| --- | --- | --- | --- |
| `chain_mint_works_for_codex_ingestion` | :655 | §3.1.1 UI mint for Codex | Codex chain identity still mints — without this test, Rev 4's "Codex chain identity preserved" claim is unverified |
| `decide_migration_returns_codex_deferred_for_codex_provider` | :665 | §5 step 6 Codex limitation | covers both branches: with and without Claude-Code sibling |
| `migration_mechanic_errors_codex_deferred_on_codex_active_provider` | :667 | §6 step 1/3 typed error | pins `MigrationError::CodexMigrationDeferred` against accidental removal |

Net §11.2 count change: 0 (3 in / 3 out). All three additions are
narrow, single-surface tests; no creep.

### §11.1 group changes

The "migration mechanic" theme row at proposal :628 now reads
"Claude JSONL copy, Codex deferred guard, segment ledger, and
races" with patterns `migration_copies_*`,
`migration_appends_chain_segment_*`,
`migration_returning_clause_aborts_on_concurrent_close`,
source-path errors, `migration_mechanic_errors_codex_deferred_*`.
Cleanly groups the Rev 4 additions.

The "compaction-aware target build" row at :629 is now scoped to
"Claude" only (Codex compaction subsumed by §15 broader deferral).

### §11.1 orphaned-pattern issue (very low — see #8.B)

The "Resume strategy compatibility" row at proposal :633 references
patterns `compose_resume_args_*`. No `compose_resume_args_*` test
appears in §11.2. Rev 3 had `migration_composes_codex_experimental_resume_argv`
as this group's anchor; Rev 4 deleted that test but left the §11.1
row's pattern reference unchanged. Either the row should be removed,
or §11.2 should add a small unit test pinning that:

- existing `flag` and `subcommand` argv stay unchanged when
  `target_jsonl_path: None` is passed, AND
- a TOML with `kind = "config"` fails to parse (negative test for
  the v1 invariant).

Carry-over from Rev 2 #4.B and #4.C does not subsume this — those
are resolver/segment-close defensives, not strategy compatibility.

---

## 5. §13.1 — supported-surface (Rev 4)

### Adjacent paths — full enumeration of problem-map §4

| §13.1 line | Problem-map §4 # | Status under Rev 4 |
| --- | --- | --- |
| :739 `agents repl <model>` | #1 | ✓ |
| :740 `agents repl <model> --resume <UUID>` | #2 | ✓ |
| :741 `agents resume -m <model> --session-id <UUID> -f <file>` | #3 | ✓ |
| :742 `agents -m <model> --resume <UUID> "prompt"` | #4 | ✓ |
| :743 `agents --resume <UUID>` no prompt + `-m` → `run_repl` | #5 | ✓ (added — Rev 3 #9.C resolution) |
| :744 `agents trace` / `--json` | #6 | ✓ |
| :745 Direct user-terminal CLI usage via session ingestion | #7 | ✓ (added — Rev 3 #9.C resolution) |
| :746 `cargo run --example session_scan` | #8 | ✓ (added — Rev 3 #9.C resolution) |
| :747 `agents quota_check` example | #9 | ✓ |
| :748 Tauri `test_model_with_db_path` | #10 | ✓ |
| :749 Session ingestion through balanced execution | #11 | ✓ (added) |
| :750 Post-success `ingest_and_emit_session_id` | #12 | ✓ (added) |
| :751 PoolsView/StatusView | §3 #7 | ✓ (additive, "remain read-only") |

**12/12 §4 paths enumerated**, plus §3 #7 frontend reference.
Rev 3 #9.C cleared.

### `[migrate]` stderr line — anchor verified

Proposal :776 mechanizes the line: "The `[migrate] <source-provider> ->
<target-provider> reason=<transition_reason>` line is emitted on
stderr from the migration helper (§6 step 6, after the segment row
is opened and before §6 step 7 composes target argv). Mirrors the
existing `[resume] -> <provider>` line at `src-tauri/src/main.rs`
(find the resume selection log site at implementation time).
Always emitted, regardless of TTY, exactly once per migration
event."

Anchor specifies (a) which §6 step (between 6 and 7), (b) reference
line for emission style (`[resume] ->`), (c) TTY-independence,
(d) once-per-event semantics. Rev 3 #9.B cleared.

Note: the line is anchored from §13.1 backward to §6 — but §6
itself does not enumerate the emission as a numbered step. This
is acceptable (the anchor is unambiguous) but a §6 reader without
§13.1 will not see it. Non-blocking.

### SQL observability queries (six)

| Query | Purpose | Tables touched |
| --- | --- | --- |
| Q1 :782 | Active chains on a provider | `session_chain_segments` |
| Q2 :787 | Migrations in past 24h | `session_chain_segments` |
| Q3 :794 | Chains sharing session_id (ambiguity) | `session_chain_segments` |
| Q4 :800 | Live-state turns post-compaction | `session_chain_segments` + `session_turns` |
| Q5 :823 | Quota-threshold migrations per chain | `session_chain_segments` |
| Q6 :830 | Open segments with no recent invocation (orphans) | `session_chain_segments` + `invocations` |

**Six queries cover the in-scope observability surface**: chain
inventory (Q1), migration audit (Q2/Q5), ambiguity detection
(Q3), live-state inspection (Q4), and orphan/dead-segment
detection (Q6). For a v1 first delivery that does not promise
trace UI for chains, this set is sufficient. Operators can build
ad-hoc joins on top.

### Rollback path — re-validated under Rev 4

Proposal :766 states: "Because Rev 4 removes the `kind = "config"`
resume strategy, there is no v1 schema or config drift to undo for
Codex." Confirms that Rev 4 strictly improves the rollback story
(no removed-key migration surface to handle).

### Cohort coverage — UI-only Codex users

§13.1 :735-736 commits UI-only Claude Code / Codex users as reachable
via session ingestion. Codex UI sessions still mint chain identity
(§3.1.1 + `chain_mint_works_for_codex_ingestion`); their resume
through agent-runner uses the existing Codex `subcommand` strategy.
Cross-account Codex migration is the only feature withheld from
this cohort. Acceptable per §15 broader Codex deferral.

---

## 6. Single-PR boundary — re-validate under Rev 4

Three split candidates re-evaluated:

### Split A — schema-only prereq PR
Unchanged from Rev 2/Rev 3 verdict. Schema is additive but inert
without resolver, mint, and backfill which all consume it.
Rejected.

### Split B — resolver+CLI vs migration mechanic
Unchanged from Rev 2/Rev 3 verdict. Resolver depends on chain
identity which depends on mint which depends on schema which the
migration mechanic also depends on. Splitting creates a no-op
intermediate state. Rejected.

### Split C — Codex-deferred carve-out (NEW under Rev 4)
Rev 4 effectively does this carve-out: Codex migration is OUT
of v1; only Codex chain identity remains. The remaining
mechanism is Claude-only and tightly coupled (schema + resolver
+ executor + CLI all participate). No further split is available
without producing dead intermediate code. Rejected.

**Single-PR boundary: justified.** Rev 4's Codex deferral is the
correct carve-out and reaches the residual minimum.

---

## 7. Scope creep check (Rev 4)

### New surface introduced

| Net-new in Rev 4 | Justified? |
| --- | --- |
| `MigrationError::CodexMigrationDeferred { provider }` typed error | yes — replaces `experimental_resume` mechanism that Rev 4 verification proved non-functional |
| `chain_mint_works_for_codex_ingestion` test | yes — pins Rev 4's "Codex chain identity preserved" claim that would otherwise be unverified |
| `decide_migration_returns_codex_deferred_for_codex_provider` test | yes — covers the §5 step 6 Codex behavior added in Rev 4 |
| `migration_mechanic_errors_codex_deferred_on_codex_active_provider` test | yes — pins the new typed error against accidental removal |
| §13.1 `[migrate]` line anchor | drafting fix from Rev 3 #9.B; not new design |
| §13.1 problem-map §4 #5/#7/#8 enumeration | drafting fix from Rev 3 #9.C; not new design |
| §13.1 SQL queries Q1–Q6 | observability documentation; not new code; queryable on existing tables |

**No speculative additions.** Each new surface has a direct
justification in Rev 4's locked answers (Q3 update, Q7 update)
and the verification doc.

### Scope-out vs §13.1 alignment (re-walked under Rev 4)

| Scope-out item | §13.1 / §15 stance | Conflict? |
| --- | --- | --- |
| Mid-process REPL migration | §15 unresolved | no |
| Cross-org cache prophylaxis | §14 + §15 unresolved | no |
| Cross-CLI migration | §15 unresolved | no |
| Codex compaction adapter | §15 (subsumed by broader Codex deferral) | no |
| Codex cross-account migration | §15 first entry (Rev 4 promotes from §15 sub-entry to top entry) | no |
| `transcript_preview` adapter | §15 unresolved | no |
| GC / archival | §15 unresolved | no |
| Frontend chain visibility | §13.1 :751 explicit "remain read-only on chain data in v1" | no |
| Per-chain quota accounting | §15 unresolved | no |
| Retroactive merging beyond first-read | §13.1 backfill is one-shot | no |

**No silent expansion.** Rev 4 in fact narrows: cross-account
Codex migration moves from "implicit in §6" to "explicitly
deferred in §15."

---

## 8. Drafting issues found in Rev 4

### #8.A — Initiative file scope item out of sync (very low)

`initiatives/05-session-migration.md:84-85` still lists "New
resume strategy `kind = "config"` for Codex's `experimental_resume`."
as in-scope. Rev 4 removes this. The proposal is the source of
truth for what ships, but the initiative file should be amended
to either delete that bullet or annotate "(deferred — see §15)"
during the next initiative-file update. **One-line fix; outside
the proposal, so non-blocking for the proposal-level scope gate.**

### #8.B — §11.1 "Resume strategy compatibility" patterns orphaned (very low)

Proposal :633 references `compose_resume_args_*` patterns. No
`compose_resume_args_*` test appears in §11.2 after Rev 4 deleted
`migration_composes_codex_experimental_resume_argv`.

**Fix options:**
- Delete the §11.1 row entirely (the surface it pinned no longer
  exists).
- Or add a small §11.2 unit test pinning the v1 invariant: existing
  `flag`/`subcommand` argv unchanged when `target_jsonl_path: None`
  is passed; `kind = "config"` TOML fails to parse.

The acceptance criterion "no `config` strategy parses in v1" is
worth a negative test on its own, since it's a specific Rev 4
invariant that could regress silently if a future PR re-adds
`ConfigArgument`. **Recommend the additive option.**

Severity: very low. The compose-args change is mechanical (added
`Option<&Path>` parameter, threaded through two call sites); the
existing balancer/CLI tests will catch any visible argv change.

### #8.C — §1.1 "narrowed" wording carry-over from Rev 3 (very low)

Proposal :19 and :32 still read "narrowed from problem-map §7"
while A1–A8 = 8 entries vs problem-map A1–A6 = 6 entries.
Direction is expansion, not narrowing. Recommended fix from Rev 3
report still applies: "approved register, consolidated and
extended from the problem-map draft." One-word fix; not a
scoping issue.

---

## 9. Carry-overs from Rev 2

### #4.B — `resolve_resume_user_override_wins_over_chain_recorded_model`

**Still missing.** §11.2 has tests for `model_override = None`
falling through the four-step chain (proposal :648, :649, :650,
:651), but no test for `Some(m)` short-circuiting (resolver §4
step 5 first branch). Proposal §4 :241 commits "If `model_override`
is `Some(m)`: use `m`." — unpinned. Rev 4 did not address.
**Severity: low.**

### #4.C — `migration_segment_close_last_turn_id_null_when_no_session_turns`

**Still missing.** Proposal §3.2 :176 commits `last_turn_id IS NULL`
when no `session_turns` rows exist for the segment. No §11.2 test
pins this defensive case. Rev 4 did not address.
**Severity: low.**

Both carry-overs are §11.2 additions, not Rev 4 scope changes;
unchanged severity through Rev 4.

---

## 10. Other Rev 3 observations under Rev 4

| Rev 3 obs | Rev 4 status |
| --- | --- |
| #4.B `model_override` test | still missing (low) |
| #4.C `last_turn_id IS NULL` test | still missing (low) |
| §3.4 batch-insert path not pinned distinctly | unchanged (very low) |
| §6.5 three failure-mode tests absent | **closed** — §6.5 zstd path removed under Rev 4 |
| §5.1 dedicated refactor-parity test | unchanged (very low; existing suite covers) |
| §14 stale "additive only" wording | unchanged (very low) |
| #9.A "narrowed" wording (§1.1) | still present — see #8.C |
| #9.B `[migrate]` anchor | **closed** under Rev 4 |
| #9.C problem-map §4 enumeration | **closed** under Rev 4 |

Net: three Rev 3 / Rev 2 observations close under Rev 4; new
finding #8.B replaces #9.B/#9.C; #8.C carries forward; #4.B/#4.C
remain.

---

## 11. Summary

- **Coverage:** complete. 12/12 problem-map §4 paths enumerated;
  every Rev 4 change traces to locked answers (Q3 / Q7 updates),
  the verification doc, or a Rev 3 nit fix.
- **Direction:** Rev 4 is a scope REDUCTION. Codex migration
  defers; Codex chain identity preserved. Single typed error
  replaces an unworkable mechanism.
- **Single-PR boundary:** justified. Rev 4 reaches the residual
  minimum (Codex deferred, Claude-only mechanism remains tightly
  coupled).
- **Test list:** three Codex tests deleted (justified by surface
  removal); three Codex-deferred / chain-identity tests added
  (each pins a specific Rev 4 surface); net §11.2 count change
  is zero. §11.1 grouping correctly updated except for one
  orphaned pattern row (#8.B).
- **README scope:** correctly reflects Rev 4 (§12 :705-706 update
  the resuming-a-session bullet; no `kind = "config"` README
  content was ever shipped, so nothing needs removing).
- **Creep:** none. The new typed error and three new tests are
  narrow and well-justified. SQL queries Q1–Q6 are documentation,
  not new code.
- **Drafting:**
  - #8.A — initiative file's "New resume strategy `kind = config`"
    bullet out of sync with Rev 4 (one-line fix in initiative file).
  - #8.B — §11.1 "Resume strategy compatibility" row references
    patterns with no §11.2 anchor (delete row OR add small
    `compose_resume_args_*` test pinning v1 invariant).
  - #8.C — §1.1 "narrowed" wording carry-over from Rev 3
    (one-word fix).
- **Carry-overs:** #4.B `model_override` short-circuit and #4.C
  `last_turn_id IS NULL` segment-close defensive remain at low
  severity; unchanged.

**Verdict: LOW.** Rev 4's Codex deferral is correctly scoped: it
reduces v1 ambition where verification showed the proposed
mechanism was unworkable, while preserving Codex chain-identity
participation (ingestion mint, segment ledger, same-provider
resume-by-id). The new tests pin the deferred-error and
chain-identity claims; the deletions match removed surface; the
single-PR boundary holds; the README scope and §13.1 supported
surface are aligned. Two of three Rev 3 nits resolved; one
remains. One new very-low Rev 4 drafting nit found. Two Rev 2
test carry-overs remain at low severity. None of the findings
is a scoping issue — all are one-line drafting fixes or
test-list additions.
