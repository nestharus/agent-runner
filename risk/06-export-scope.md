# 06-export — Phase 4 Scope Risk Assessment (Rev 2)

**Assessor:** `claude-opus` (scope)
**Verdict:** **LOW** — Rev 2 makes a single, surgical edit (§8 pins the
`STATE_DIR` mkdir clause by importing 06-locate's accepted wording and
deletes the Rev 1 "Phase 5 must either identify a read-only locator path
or revise this proposal" escape hatch). The change tightens the
side-effect contract; it does not expand surface, anti-scope, exit
codes, schema, public types, or assumption register. R1-F01 is an audit
finding; this scope report verifies its closure as audit-only and
confirms no scope regression. The three Rev 1 LOW drafting nits (L1
boundary-summary canonical shape, L2 `RecordSource` storage_type
tightening, L3 in-memory residual phrasing) remain unchanged in Rev 2;
they were not in the R1 cure scope and stay parked for Phase 5/6
hookpoints. No new scope finding.

---

## 1. R1-F01 closure check (audit-only)

R1-F01 was raised by the Phase 4 Round 1 **audit** gate (MEDIUM):
"§8 deferred read-only locator clause to Phase 5; pin in proposal
contract" (`risk/06-export-audit-history.md` Round 1).

| Cure obligation | Rev 2 evidence | Status |
| --- | --- | --- |
| Pin the STATE_DIR mkdir behavior in the proposal contract rather than deferring to Phase 5. | §8 third paragraph (`proposals/06-export.md:339-350`) now reads: "`agents session export` may create the locator adapter `state_dir` directory (`src-tauri/src/sessions/mod.rs:184-185`) when `locate_transcript` is invoked. This directory creation is the same behavior `trace --json` and `agents session locate` already exhibit and is part of the existing transcript-locator contract that the harness anti-scope explicitly permits ('Running configured transcript locators is allowed only if already part of the current trace/session contract'). No file inside the directory is written by `export`." | **closed** |
| Match 06-locate's §8 wording (precedent already accepted by Phase 4). | Compared to `worktrees/06-locate/proposals/06-locate.md:243`. Export's clause is the locate clause with `agents session locate` → `agents session export` and "the same behavior `trace --json` already exhibits" → "the same behavior `trace --json` and `agents session locate` already exhibit". Substantively identical. | **closed** |
| Remove Rev 1's Phase 5 escape hatch ("Phase 5 must either identify a read-only locator path or revise this proposal"). | String absent from Rev 2 §8 and absent from §12 residuals. The §1 changelog entry confirms "removes Phase 5 deferral language." | **closed** |

R1-F01 status: **closed**.

The closure is correct for the audit gate's framing. From a scope
perspective the cure is also tighter than Rev 1: by adopting locate's
already-accepted clause, the contract loses its conditional escape
("Phase 5 may revise") and binds export to the same already-permitted
side-effect envelope as locate and trace.

## 2. Fresh assessment of Rev 2 deltas

The Rev 2 changelog (`proposals/06-export.md:35-39`) lists exactly one
delta:

> §8: explicit `STATE_DIR` mkdir clause matching 06-locate's §8.
> Closes R1-F01 by pinning the contract; removes Phase 5 deferral
> language.

Walked the diff against §1, §1.1, §1.2, §2, §3, §4, §5, §6, §7, §9, §10,
§11, §12, §13. Only §1 changelog and §8 paragraph 3 changed in scope-
relevant text. The rest of the proposal is unchanged from Rev 1, and my
Rev 1 audit (sections 1–8, including the L1–L3 drafting nits) carries
forward verbatim.

### 2.1 §8 paragraph 3 — anti-scope and side-effect coherence

The new sentence permits one named filesystem side effect: creation of
the locator adapter `state_dir` directory at
`src-tauri/src/sessions/mod.rs:184-185`. Three coherence checks:

| Check | Result |
| --- | --- |
| Does the new sentence contradict §7 anti-scope? | No. §7 forbids "DB writes, transcript writes, temp files, adapter cursor writes, state repair, scans, turn scripts, migrations, or pause/resume lock commands." Directory creation by the locator adapter is none of those: it is not a transcript write, not a temp file owned by export, not an adapter cursor write (the new clause explicitly says "No file inside the directory is written by `export`"). |
| Does the new sentence contradict §8 paragraphs 1–2? | No. §8 paragraph 1 lists DB row writes, cursor writes, telemetry, invocation, trace, cache. §8 paragraph 2 lists transcript bytes/permissions/mtimes, parent dirs, temp files, replacement files, provider launches, turn scripts. The new paragraph 3 is the documented exception that exists *because* the locator adapter is invoked — and the proposal cites the harness anti-scope exception verbatim. |
| Does the new sentence introduce a side effect not already present in 06-locate's accepted contract? | No. Export inherits this exact behavior from locate (which is the gate it depends on for `locate_session_metadata`). The clause was always implicit in §4 step 5; Rev 2 makes it explicit. |

§8 paragraph 3 is therefore a pin, not an expansion.

### 2.2 §1 changelog — register integrity

The Rev 2 changelog adds one bullet under "**Rev 2 changes**" describing
the §8 pin and its motivation. No assumption rows were added to §1.1, no
register row was deleted, no row's `Used by` column was renumbered. A6
("provider JSONL line order is the stable conversation order") and A8
("`sha2` can be added as a direct dependency") remain the only Rev 1
register additions; A7 (Codex compaction-deferral) remains the only
narrowed assumption. Register hygiene preserved.

### 2.3 No regression on §13 cross-feature constraints

Re-walked all ten constraint rows in §13 against Rev 2 §8:

| Constraint | Status under Rev 2 |
| --- | --- |
| Shared error namespace (`10`/`11`/`12`/`15`) | unchanged |
| Single ownership via `StateDb::resolve_resume` | unchanged |
| Read-only `StateDb` open variant from 06-schema-probe | unchanged; Rev 2 explicit STATE_DIR clause is *not* a substitute for schema-probe's read-only open — it covers a different side effect (filesystem dir vs. SQLite open). |
| Lock observation for sibling features | N/A (export is locker-free) |
| No auto-resume | unchanged |
| No provider spawn | unchanged |
| No quota refresh | unchanged |
| No config edits | unchanged; STATE_DIR mkdir is not a config edit |
| No coupling to `migrate-config` | unchanged |
| Reusable canonical reader | unchanged |
| Harness receives canonical JSONL, not provider-native | unchanged |

All ten rows continue to hold. Rev 2 §8 strengthens row 8 (no config
edits) by explicitly drawing the boundary between "configured locator
script may create its state_dir" and "config files are mutated."

### 2.4 No regression on §7 anti-scope

Re-walked §7's nine bullets against Rev 2 §8. All bullets continue to
hold. The STATE_DIR mkdir is not a temp file, scan, turn script, or
migration; it is not a write to `state.db`; it is not a transcript
mutation; it is not an alternate format; it is not import/replace; it is
not GUI; it is not provider-private metadata preservation. Anti-scope
unchanged.

### 2.5 No regression on §6 public API

Public types in §6 (`CanonicalRecord`, `CanonicalRole`, `ContentChunk`,
`RecordSource`, `ExportError`, `read_canonical_transcript`) are
untouched by Rev 2. The reusable reader API contract for
`06-import-replace` consumption is preserved. L2 (RecordSource
storage_type tightening) remains a Phase 5/6 ergonomics nit — not
escalated.

### 2.6 No regression on §3 schema or §5 exit codes

§3 record schema (8 required top-level fields, 6 required source
subfields, 3 chunk variants) and §5 exit-code table (`0`/`1`/`2`/`10`/
`11`/`12`/`15`) are unchanged. L1 (boundary-summary canonical shape)
remains a Phase 5/6 drafting nit — not escalated.

### 2.7 No regression on §11 supported-surface

§11.1 (local CLI binary only, harness primary consumer, additive
rollback) is unchanged. The STATE_DIR mkdir disclosure does not change
deployment mode, cohort, blast-radius framing, migration path, rollback
path, or observability claim. Sibling supported-surface report is
unaffected.

### 2.8 No regression on §12 residuals

§12 lists seven residuals (parser drift, Codex compaction, timestamp
regression, in-memory buffering, `sha2` direct dep, `Other` rejection,
no native-payload preservation). None changed in Rev 2. The Rev 1
"STATE_DIR escalation to Phase 5" was *not* in §12 (it lived in §8 prose
only); removing it from §8 does not require a §12 edit. L3 (in-memory
residual phrasing) remains a Phase 5/6 drafting nit.

## 3. Drift audit (S5)

| Section | Rev 2 surface | Drift? |
| --- | --- | --- |
| §1 scope statement | unchanged + 1 changelog bullet | no |
| §1.1 assumption register | unchanged (8 rows) | no |
| §1.2 net-value statement | unchanged | no |
| §2 subcommand surface | unchanged | no |
| §3 per-record schema | unchanged | no |
| §4 resolution flow (10 steps) | unchanged | no |
| §5 exit codes | unchanged | no |
| §6 reusable reader API | unchanged | no |
| §7 anti-scope (9 bullets) | unchanged | no |
| §8 side-effect contract | paragraph 3 added (STATE_DIR mkdir clause matching locate) | scope-tightening, not expansion |
| §9 test-intent track | unchanged | no |
| §10 README updates | unchanged | no |
| §11 supported-surface track | unchanged | no |
| §12 residuals | unchanged | no |
| §13 cross-feature constraints (10 rows) | unchanged | no |

No drift. Rev 2 is a single targeted pin.

## 4. Watch-flag re-evaluation

Rev 1 captured seven watch flags (WF1–WF7). WF1 was the STATE_DIR Phase
5 escalation; Rev 2 closes its underlying concern by pinning the clause,
so WF1 is **discharged**. WF2–WF7 carry forward unchanged (each was
already judged "not a Phase 4 scope finding" in Rev 1; the Rev 2 §8 edit
does not affect any of them).

## 5. Findings

### Severity ≥ MEDIUM

None.

### Severity LOW (drafting nits — carried from Rev 1, unchanged in Rev 2)

- **L1** — §4 step 8 + §3 do not pin canonical shape of Claude
  compaction-boundary record (role / chunk / `unsupported_record`).
  Phase 5 hookpoint or Rev 3 prose pin would resolve.
- **L2** — `RecordSource.storage_type: SessionStorageType` includes
  `Other` even though §4 step 6 fails before any record is built.
  Tighter public type or one-line invariant note in §6 would
  resolve.
- **L3** — §12 bullet 4 in-memory residual is softer than §6 D7's
  commitment. A stated size band or explicit post-v1 streaming
  deferral would make the residual actionable.

L1–L3 are not Phase 4 scope concerns. They were not in R1-F01's cure
scope and are not Rev 2 regressions. They remain available for Rev 3 or
Phase 5/6 hookpoint adoption.

## 6. Verdict and recommendation

**Verdict: LOW.**

R1-F01 is closed (audit-only). Rev 2 introduces no scope regression,
no anti-scope drift, no constraint breakage, no register expansion, no
public-type change, no schema/exit/test-intent change, no residual
churn. The single edit (§8 paragraph 3) tightens the side-effect
contract by removing the Phase 5 escape hatch and binding export to the
same STATE_DIR mkdir envelope that 06-locate already enjoys under
already-accepted harness contract terms. No further scope action
required for Round 2.
