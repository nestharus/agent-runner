# 06-import-replace — Phase 4 Scope Risk Assessment (Rev 3)

**Assessor:** scope reviewer
**Verdict:** **LOW.** Rev 3 is a targeted closing pass against the
single Round 2 audit finding (AIR-R2-F01). The finding is closed at
the scope level on all three required axes: the journal payload now
persists the resolved recovery identity (chain_id,
active_segment_id, provider_name, storage_type) plus a
journal-attached `canonical_records_path` so DB recovery does not
re-derive rows from stale resolver state or from provider-native
postimage bytes; the success-flow has been reordered so journal
deletion is the LAST durable cleanup step, strictly after
postimage_sha256 verification, fresh export round-trip
verification, and SQLite commit; and four new T-rows in §9 cover
the recovery scenarios (rename-only, ambiguous-hash,
canonical-records-preserved, no-delete-before-verify). The four
Round 1 closures (AIR-R1-F01..F04) all still hold — Rev 3 only
strengthens F02's closure and does not loosen any of F01/F03/F04.
Anti-scope is intact, the single-PR boundary is still justified
after the journal grows by four identity fields plus an attached
canonical records file, and every cross-feature constraint in
`06-session-override-contract.md:106-122` still maps to a numbered
section. Rev 3's expanded journal + reordered flow are race-free
for the documented threat model (single-process crash, cooperative
SessionLock surface, eight enumerated crash states in §8). One
prior watch-flag (Rev 2 W4 — journal/lock ordering invariant) is
expanded but not closed and carried forward as W4'; the other
three carry-over watch-flags (W1/W2/W3') are unchanged. Three
carry-over Rev 1 nits (N1/N2/N3) untouched. No findings at MEDIUM
or higher.

---

## 1. Closure check on AIR-R2-F01

Audit-only closure: each required-change bullet from
`risk/06-import-replace-audit.md` (Rev 2 audit, AIR-R2-F01) is
matched against the Rev 3 text that resolves it. No new audit work
is performed here.

### Required change 1 — persist resolved recovery identity in the journal before transcript mutation

R2 audit required `provider_name`, `storage_type`, `chain_id`,
active `segment_id` (or equivalent stable segment key),
`session_id`, canonical `jsonl_path`, expected preimage/postimage
hashes, and enough canonical postimage material or parser metadata
to rebuild `session_turns` without relying on stale resolver
output.

Rev 3 journal format (`proposals/06-import-replace.md:330-347`):

```json
{
  "schema_version": 1,
  "operation": "import-replace",
  "started_at": "...",
  "session_id": "...",
  "chain_id": "...",
  "active_segment_id": 42,
  "provider_name": "claude2",
  "storage_type": "claude_code",
  "jsonl_path": "...",
  "preimage_sha256": "...",
  "postimage_sha256": "...",
  "canonical_records_path": "...",
  "db_state_pending": true,
  "expected_turn_count": 18
}
```

Mapping:

| Audit-required field | Rev 3 field | Notes |
| --- | --- | --- |
| `provider_name` | `provider_name` | frozen at §4 step 8 (`:264-265`) |
| `storage_type` | `storage_type` | frozen at §4 step 8; restricted to `claude_code` / `codex_session` per §4 step 7 (`:262-265`) |
| `chain_id` | `chain_id` | frozen at §4 step 8 (`:264-265`) |
| active segment key | `active_segment_id` | frozen at §4 step 8; explicitly used by recovery to refresh segment recency (`:434-437`) |
| `session_id` | `session_id` | resolved active provider session id, not raw input |
| canonical `jsonl_path` | `jsonl_path` | resolved path |
| preimage / postimage hashes | `preimage_sha256` / `postimage_sha256` | canonical export hashes (`:349-351`) |
| canonical postimage material to rebuild `session_turns` | `canonical_records_path` | side-file at `<state-data-dir>/replace_journal/session-<id>.canonical.jsonl` written before transcript rename and fsynced (`:282-286`); recovery rebuilds DB rows from this file, not from provider-native postimage bytes (`:354-356`, `:434-437`) |

§4 explicitly freezes the resolved identity for the operation at
step 8 ("Freeze the resolved identity for the operation:
`session_id`, `chain_id`, `active_segment_id`, `provider_name`,
`storage_type`, and `jsonl_path`", `:264-265`) and the journal is
written at success-flow step 1 with that frozen identity attached.
The recovery contract (§6 startup-recovery, `:421-446`) reads
identity from the journal, not from the resolver — directly
addressing the audit's "must rediscover provider/storage/chain
context from potentially stale DB/config state" concern.

§4 also mandates that recovery rebuilds `session_turns` from
`canonical_records_path`, not by re-parsing the postimage
transcript (`:354-356`: "recovery must not re-read the postimage
transcript and infer DB rows from provider-rendered bytes"). This
is the "enough canonical postimage material … to rebuild
`session_turns` without relying on stale resolver output" leg.

**Required change 1: closed.**

### Required change 2 — fresh postimage export verification before journal deletion (or quarantine on failure)

R2 audit required either moving fresh export verification before
journal deletion, or stating that any post-DB verification failure
leaves/quarantines the journal instead of deleting it.

Rev 3 §4 success flow ordering (`:280-321`):

1. Step 7: Compute `postimage_sha256` from the new transcript
   under the canonical reader; verify against journal's recorded
   `postimage_sha256`. Mismatch → roll back SQLite, exit `1`,
   leave journal + canonical records file in place (`:305-309`).
2. Step 8: Run fresh export verification — parse the new
   transcript through the canonical reader and compare the
   resulting canonical bytes to `canonical_records_path`. Mismatch
   → roll back SQLite, exit `1` with a specific
   fresh-export-verification error, leave journal + canonical
   records file in place (`:310-314`).
3. Step 9: Only after step 8 succeeds, commit the SQLite
   transaction (`:315`).
4. Step 10: Delete the journal entry and canonical records file,
   then fsync the `replace_journal` directory. **"This is the last
   durable cleanup step."** (`:316-318`)

The success-flow narrative restates the invariant: "Any failure in
success-flow steps 3-9 leaves the journal plus canonical records
file in place; that journal is the recovery signal." (`:324-326`)
§4 also restates it after the journal format block (`:368-370`):
"The filesystem journal entry and canonical records file are
deleted only after postimage verification, fresh export
verification, and SQLite commit all succeed."

§5 exit `1` row is consistent: postimage verification and
fresh-export verification map to `1 operational-error` with the
journal preserved (`:379`). §8 crash states 5–8 explicitly cover
the post-rename / pre-DB-commit and verification-failure cases
(`:587-614`). §6 recovery flow item 6 explicitly handles the
ambiguous case (hash matches neither preimage nor postimage)
deterministically: move journal to `replace_journal/quarantine/`,
preserve canonical records file, log a
`"manual recovery needed"` warning (`:441-446`).

**Required change 2: closed.**

### Required change 3 — recovery test simulating stale/ambiguous resolver-visible DB rows after rename

R2 audit required adding a recovery test that proves startup
recovery uses journal identity rather than rediscovery through
broken state.

Rev 3 §9.1 adds four new T-rows (`:645-650`):

| Test row | Coverage |
| --- | --- |
| **T-recovery-rename-only** | Kill between rename and DB commit; recovery rebuilds derived state from journal-attached canonical records file (not from rediscovered resolver context) and refreshes the journal-frozen segment. Directly proves the "use journal identity, not stale resolver" requirement. |
| **T-recovery-ambiguous-hash** | Pending journal + transcript edited so canonical export hash matches neither preimage nor postimage; recovery moves journal to quarantine, leaves transcript and DB untouched. Proves the ambiguous branch. |
| **T-recovery-canonical-records-preserved** | Canonical records file survives crash and remains byte-for-byte equal to normalized input; the file is the DB recovery source. Proves the side-file durability invariant. |
| **T-no-deletion-before-verify** | Inject postimage-hash mismatch after rename; command exits operationally without deleting recovery artifacts; SQLite transaction is not committed. Proves the deletion-ordering invariant. |

Plus the Rev 2 row "Journal post-rename recovery" is retained
(`:644`), and the existing "Atomic temp/rename" component row
(`:643`) is updated to reflect the deletion-after-verify
ordering ("recovery deletes preimage-matching journals and
canonical records files; post-rename failures recover DB from
postimage and delete recovery artifacts").

T-recovery-rename-only is the load-bearing R2 test: it requires
"stale `session_turns`" pre-seeded in the test fixture and proves
that recovery replaces them from `canonical_records_path` and
refreshes the journal-frozen segment, not the resolver-rediscovered
one. That is exactly what AIR-R2-F01's third bullet requires.

**Required change 3: closed.**

### Audit-history reconciliation

`risk/06-import-replace-audit-history.md` Round 1 entry (file as
recorded in `4a598ac`) records the four R1 findings closed by
Rev 2. Round 2 audit (`risk/06-import-replace-audit.md`) records
AIR-R2-F01 HIGH open as the only outstanding gate, with the three
required-change bullets above. Rev 3's "Rev 3 changes" preamble
(`:41-56`) explicitly tags the change set against AIR-R2-F01 in
both §4 and §6 (and a side-effect-contract update in §8). The
expected next-row in `audit-history.md` after Rev 3 is "Round 2 —
AIR-R2-F01 closed by Rev 3 (journal expansion + deletion-last
ordering + recovery T-rows)."

---

## 2. Regression check on Round 1 closures (AIR-R1-F01..F04)

Walked Rev 3 against each Rev 2 closure path. No regressions.

### AIR-R1-F01 — Canonical-bytes-vs-provider-native (still closed)

Rev 3 leaves the renderer contract intact:

- §1 anti-scope clause "The replacement transcript file does not
  store canonical JSONL in v1. It stores provider-native bytes
  rendered from canonical input for the resolved storage type."
  (`:85-87`) — unchanged from Rev 2.
- §3 step 11 + `CanonicalToProviderRenderer` contract
  (`:191-238`) — unchanged.
- §6 `src-tauri/src/session_replace/render/` module
  (`:399-403`) — unchanged.
- §13 rows "Provider transcript file receives provider-native
  bytes" / "Lossy canonical-to-provider re-encoding is refused" —
  both still Yes (`:776-777`).
- §9.1 unsupported-record-class test row preserved (`:641`).

The Rev 3 journal change has a touchpoint here: §4 (`:354-356`)
explicitly forbids recovery from re-deriving DB rows from the
postimage provider-native bytes, which would have re-opened a
different shape of the F01 problem (provider-bytes → DB
inference). Recovery must use `canonical_records_path` instead.
This makes F01's closure stronger, not weaker. **Held.**

### AIR-R1-F02 — Crash recovery (still closed; strengthened by Rev 3)

This is the closure that the R2 audit found insufficient and Rev 3
expands. The R2 audit accepted that Rev 2's direction was correct
("Rev 2 adds the right recovery mechanism") but the journal payload
and deletion order were insufficient. Rev 3's three-axis closure of
AIR-R2-F01 (above) directly addresses both gaps. **Held and
strengthened.**

### AIR-R1-F03 — Cooperative-lock surface (still closed)

- §1 Rev 2 changes bullet remains (`:34-36`).
- §13 row text on PR #17 + advisory carve-out unchanged (`:773`).
- §12 residual #3 ("Running invocation rows are not treated as
  authoritative busy locks") unchanged (`:752-753`).

Rev 3 did not retouch the lock-observation claim. **Held.**

### AIR-R1-F04 — Canonical-record field-loss (still closed)

- §6 reusable-API bullet on intentional `NULL`/defaults for
  `parent_turn_id` / `is_sidechain` /
  `is_compaction_boundary` (`:407-410`) — unchanged.
- §7 #4 "documented data loss in v1" (`:506-510`) — unchanged.
- §7 last-paragraph future-extension note (`:537-538`) —
  unchanged.
- §9.1 "DB metadata loss is explicit" row (`:652`) — unchanged.
- §12 residual #5 (`:756-758`) — unchanged.
- §13 "State consistency" row text "Yes, with documented
  canonical-field loss" (`:780`) — unchanged.

Rev 3 did not retouch the field-loss model. **Held.**

---

## 3. Race-freeness of Rev 3 expanded journal + reordered flow under the documented threat model

**Documented threat model.** The proposal documents:

- Single-process crash at any point in the success flow (§8 crash
  states 1–8, `:587-614`).
- Cooperative SessionLock surface; non-cooperating external
  writers are an explicit residual (§12 #3, `:752-753`; §13 row
  on lock observation, `:773`).
- TOCTOU between the early/preflight preimage hash and the
  protected commit window (§4 explanation `:322-324`; §4 success
  flow step 3 + step 4 under-lock recheck, `:291-299`).

**Race-freeness walk against §8 crash states 1–8.**

| Crash state | When | Recovery deterministic under Rev 3? |
| --- | --- | --- |
| 1 — pre-temp | Before step 4 writes `<jsonl_path>.tmp-…` | Yes. Journal exists or not; on-disk transcript still preimage. §6 recovery item 5 deletes journal on preimage match. No DB mutation. |
| 2 — post-temp pre-rename | After step 4, before step 5 | Yes. Same as #1; opportunistic temp cleanup at §4 step 10 (`:270-272`) handles the lingering `.tmp-import-replace-<uuid>` on the next attempt. |
| 3 — post-fsync pre-rename | After temp fsync, before rename | Yes. Same as #2. |
| 4 — post-rename pre-DB | After step 5 rename, before step 6 begin | Yes. Recovery item 4 (`:434-437`) sees postimage hash and re-applies DB updates idempotently from `canonical_records_path`, refreshes the journal-frozen segment, deletes the journal + canonical records file. |
| 5 — mid-DB transaction | During steps 6–8 | Yes. SQLite either commits or rolls back per its own durability. Recovery item 4 re-applies DB idempotently from `canonical_records_path`. Idempotent re-replace is safe because step 7 of §7 deletes-then-inserts on `(provider_name, session_id)`. |
| 6 — post-commit pre-journal-delete | After step 9, before step 10 | Yes. Recovery item 4 re-applies DB idempotently and deletes journal + canonical records file. |
| 7 — preimage-only | Hash matches `preimage_sha256` only | Yes. Recovery item 5 (`:438-440`) deletes journal + canonical records file; no DB mutation. Covers steps 1–4 crash windows. |
| 8 — ambiguous | Hash matches neither, or transcript unparseable | Yes. Recovery item 6 (`:441-446`) moves journal to `replace_journal/quarantine/`, preserves canonical records file, logs warning, leaves transcript and DB untouched. Covers verification-failure (steps 7–8) leftovers and external corruption between crash and startup. |

**Reordered-flow correctness.** The Rev 3 deletion-last invariant
("This is the last durable cleanup step", `:316-318`) closes the
specific R2 ordering gap: under Rev 2, journal deletion happened
before fresh export verification, so a successful DB commit
followed by a fresh-export-verification failure left the system
mid-flight without a journal. Under Rev 3, any failure path in
steps 3–9 leaves the journal + canonical records file in place,
and recovery has a deterministic branch for every state these
failures can leave on disk. The hash-domain consistency
established under F01 (journal hashes are canonical export hashes;
recovery rehashes through the canonical reader, not raw provider
bytes) means the recovery branches do compose with the
provider-native renderer.

**Verdict:** Rev 3 is race-free for the documented threat model.
The expanded journal + reordered flow close the exact ordering and
identity gaps AIR-R2-F01 named, without re-opening any of the
crash states the F02 closure relied on.

**Out of the documented threat model.** The Rev 2 W4 watch-flag
about concurrent same-session import-replace processes (two
processes targeting the same `<id>` racing over a shared
pre-lock journal path) is *outside* the documented threat model
(which is single-process). Rev 3 did not change the §4
step 1 → step 2 ordering (journal write *before* lock acquire)
nor the journal path scheme (`session-<session_id>.pending` plus
the same-prefix `.canonical.jsonl`), so that pre-lock window is
unchanged. With Rev 3 the race surface widens slightly because
the pre-lock window now also writes the side-file
`session-<session_id>.canonical.jsonl`, which a busy concurrent B
"may unlink … idempotently before exit" (`:288-290`) — exactly
the same shape as Rev 2 W4, applied to two paths instead of one.
Carrying as W4' below; not a finding because it is outside the
documented threat model and is fixable at Phase 5 hookpoint time
without scope change.

---

## 4. Anti-scope and cross-feature constraints (no regression)

### Anti-scope (vs `06-session-override-contract.md:117-122` and harness)

| Anti-scope clause | Rev 3 stance | Compliance |
| --- | --- | --- |
| No auto-resume | §1 (`:78-82`); §11 (`:719-721`); §13 rows | yes |
| No provider spawn | §1; §11; §13 row | yes |
| No quota refresh | §1; §11; §13 row | yes |
| No config edits | §1; §11; §13 row | yes |
| No coupling to `migrate-config` | §1; §11; §13 row | yes |
| No GUI/Tauri/daemon/server | §1 (`:81`); §11.1 (`:701-702`) | yes |
| No provider-native JSONL as stable public input | §1 (`:84-85`); §3; §10 | yes |
| No manual recovery CLI in v1 | §6 last paragraph (`:448-450`); §12 #2 (`:749-750`); §13 row (`:787`) | yes |

Rev 3 does not introduce any new public surface. The journal
schema growth, the canonical_records_path side-file, and the
quarantine directory are all under `<state-data-dir>/replace_journal/`
and are documented as private implementation state (`:349`,
`:736-738`).

### Cross-feature constraints (`06-session-override-contract.md:106-122`)

Every row in §13 still maps to its own numbered section. No row
loosened by Rev 3; the journal-related rows tightened:

| Constraint | Rev 3 | Notes |
| --- | --- | --- |
| Shared error-code namespace 10–15 | yes | §5 unchanged; sub-codes under exit `15` still inside namespace |
| Single ownership via `StateDb::resolve_resume` | yes | §4 step 6 unchanged |
| Lock observation once pause-handshake lands | yes within cooperative surface | F03 closure text unchanged |
| Refuses if not exclusively owned | yes within cooperative lock surface | non-cooperating writers remain a §12 residual |
| Read-only `StateDb` open / schema compatibility | yes | §4 step 5 → exit `14` |
| Reusable canonical reader from export | yes | A3, §3, §9 |
| Provider transcript receives provider-native bytes | yes | §3 / §6 renderer module unchanged |
| Lossy canonical-to-provider re-encoding refused | yes | §3 / §9 unsupported-record-class test |
| Two-phase atomic file replacement | yes | §4 / §8 unchanged |
| Durable journal closes post-rename/pre-DB crash recovery | yes (strengthened) | journal payload + deletion-after-verify ordering — Rev 3 closure of AIR-R2-F01 |
| State consistency covers required rows | yes, with documented canonical-field loss | §7 D4a unchanged |
| No manual recovery CLI in v1 | yes | §6 / §12 |

---

## 5. Single-PR boundary

Re-evaluated against Rev 3's deltas (journal payload growth +
canonical_records_path side-file + quarantine directory + four
new T-rows). Same three split candidates considered as Rev 2:

**Split A — `session_replace/render/` as a separate prereq PR.**
Unchanged from Rev 2. Renderer is private API consumed only by
`session_import_replace/`; splitting yields a private renderer
with no caller. **Rejected.**

**Split B — durable journal + recovery contract as a follow-up PR.**
Rev 3 makes this even more clearly the wrong split. The journal
is the recovery signal that closes both F02 and AIR-R2-F01.
Landing import-replace without it re-introduces both blockers.
**Rejected.**

**Split C — DB consistency helper vs CLI surface.** Unchanged.
**Rejected.**

**Split D (new in Rev 3) — recovery scanner as a prereq sibling
PR.** The startup-recovery contract (§6, `:421-446`) only fires
on journals written by import-replace; without import-replace, the
scanner has nothing to recover. **Rejected.**

**Single-PR boundary: still justified.**

---

## 6. Scope-direction analysis (Rev 3 vs Rev 2)

| Surface vs Rev 2 | Direction | Reason |
| --- | --- | --- |
| Journal payload | targeted addition | required to close AIR-R2-F01 axis 1; identity fields are private state and do not extend public surface |
| `canonical_records_path` side-file | targeted addition | required so recovery rebuilds DB rows from canonical, not from re-parsed provider-native bytes |
| Success-flow ordering | scope-tightening | required to close AIR-R2-F01 axis 2; deletion strictly after verification + commit |
| Startup recovery contract | targeted addition | three explicit branches (postimage / preimage / ambiguous + quarantine) replace the previously-vague single branch |
| §9 test track | additive coverage | four new T-rows specifically target AIR-R2-F01 axis 3 |
| §8 side-effects | additive | enumerates canonical_records_path write + quarantine directory; no new public side-effects |
| Anti-scope | unchanged | eight harness/initiative clauses still intact |
| Public surface | unchanged | §2 / §6 receipt JSON / §5 exit codes identical to Rev 2 |
| AIR-R1-F01..F04 closures | unchanged or strengthened | F02 strengthened; F01/F03/F04 untouched |

Net direction: Rev 3 makes targeted closures against the single
R2 audit finding without expanding the public surface and
strengthens F02's existing closure. The new internal state
(journal payload growth + side-file + quarantine directory) is
the minimum work to satisfy the audit and is bounded to v1 scope.

---

## 7. No-regression check (Rev 2 → Rev 3)

| Rev 2 scope item | Rev 3 status | Notes |
| --- | --- | --- |
| Anti-scope (8 clauses) | held | unchanged |
| Cross-feature constraints | held | "Durable journal closes …" row tightened by Rev 3 |
| Coverage matrix (problem-map §1–§7) | held | additive coverage on AIR-R2-F01 closure surfaces |
| Single-PR boundary | held | re-justified against journal payload + side-file + quarantine |
| W1 stale-temp ordering | unchanged | §4 step 10 same wording; still a Phase 5 hookpoint concern |
| W2 schema-probe flag flip | unchanged | §4 step 5 same; coordination with 06-schema-probe still pending |
| W3' renderer round-trip parity | unchanged | §9.1 postimage round-trip row carries it |
| W4 journal/lock ordering invariant | expanded → W4' | §4 step 1 → step 2 pre-lock window unchanged; widened to also cover canonical_records_path side-file |
| N1 exit-namespace 16/17 omission note | unchanged | §5 / §13 still do not carry the one-line note |
| N2 §7 conditional `source_file` hedge | unchanged | now §7 #5; same defensive language |
| N3 §1.1 "validated and narrowed" wording | unchanged | §1.1 line still says "validated and narrowed" |

**No regression.** Rev 3 was a targeted AIR-R2-F01 pass and did
not retouch any prior nit or watch-flag wording. Rev 2 W4 widens
into W4' (below) because Rev 3 added a second path subject to the
same pre-lock race; this is informational, not a regression.

---

## 8. Findings (severity ≥ MEDIUM)

**None.**

---

## 9. Watch-flags (informational; not findings)

### W1 — opportunistic stale-temp cleanup ordering (carry-over)

§4 step 10 ("Clean stale import-replace temp files in the target
transcript directory whose names match this feature's temp-file
convention and are not currently locked by another live replace
operation", `:270-272`) still runs before `SessionLock::acquire`
at step 2 of the success flow. The proposal does not specify how
step 10 distinguishes a stale temp from a temp owned by another
in-flight import-replace. Phase 5 hookpoints should pin the
mechanism (per-temp flock sentinel, mtime threshold, or post-lock
cleanup). Not a scope issue.

### W2 — schema-probe `safe_for_import_replace` flag flip (carry-over)

Problem-map §3 #3 says 06-schema-probe returns
`safe_for_import_replace = false` until both `session_import_replace`
and `session_pause_handshake` features are present. The flip lands
in the schema-probe branch when this feature ships. §4 step 5
correctly defers the flip itself to schema-probe; this remains a
Phase 5 sequencing watch-flag, not a scope issue.

### W3' — renderer round-trip parity (carry-over)

§3 last paragraph of the renderer contract requires that "Every
supported rendered record must round-trip through export back to
the canonical input." §9.1 "Postimage round-trip" is the
end-to-end anchor. Phase 5 should sample real Claude / Codex
transcripts and verify renderer/export round-trip parity per the
Rev 2 W3' bullets. Unchanged from Rev 2.

Note: Rev 3 strengthens this watch-flag's load-bearing role.
Step 8 of the success flow now requires fresh export verification
under the lock against `canonical_records_path` before SQLite
commit. If renderer round-trip parity fails on a real-world
record class, Rev 3's `1 operational-error` exit and quarantine
branch are the correct fail-closed behavior — but Phase 5 still
needs to pre-empt the common cases so quarantine is rare.

### W4' — journal/lock ordering invariant + side-file race (expands Rev 2 W4; new in Rev 3)

§4 success-flow step 1 writes both
`<state-data-dir>/replace_journal/session-<session_id>.pending`
and the side-file
`<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl`
**before** step 2 acquires `SessionLock`, and step 2's busy
branch ("this handled busy failure may unlink the journal and
canonical records file idempotently before exit", `:288-290`)
deletes the same paths the lock holder has just written. Two
import-replace processes against the same `<id>` use the same
paths.

Race walk:

1. A writes journal + side-file (preimage X_A, postimage Y_A,
   canonical records R_A), fsyncs.
2. B writes journal + side-file (preimage X_B, postimage Y_B,
   canonical records R_B), overwriting A's bytes on the same
   paths, fsyncs.
3. A acquires lock. B sees busy.
4. B unlinks journal + side-file ("idempotently") and exits 13.
5. A is now mid-flight with no on-disk journal *and* no on-disk
   side-file.

Rev 3's step 4 partially mitigates the journal half ("If the
under-lock preimage differs from the journal's
`preimage_sha256`, atomically rewrite and fsync the journal
before writing a transcript temp file", `:295-299`) — but the
rewrite path:

- Only restores the journal if the under-lock preimage differs
  from the journal's preimage. If the inputs share the same
  preimage but different postimages (e.g., two different
  canonical-record sequences on the same starting transcript),
  the rewrite branch does not fire and A continues with B's
  postimage in the journal.
- Does not restore `canonical_records_path`. §4 success-flow
  step 6 then "Replace `session_turns` rows for this
  provider/session from `canonical_records_path`" — A would write
  A's bytes to disk but B's records to the DB (or fail because
  the file is gone).

Rev 2 W4 already raised the journal-only half. Rev 3 widens it to
the side-file. The clean fixes Rev 2 already named both still
apply at Phase 5 hookpoint time without scope change:

- Move both the journal write and the canonical records write to
  **after** lock acquisition. (The crash window F02 / AIR-R2-F01
  cares about is post-rename, which can only happen under the
  lock anyway, so the durability requirement is preserved.)
- Keep the pre-lock writes but use a per-attempt unique filename
  (e.g., `session-<id>-<uuid>.pending` and
  `session-<id>-<uuid>.canonical.jsonl`), so a busy process never
  deletes the lock holder's files. Recovery would then enumerate
  any matching glob and reconcile per-file.

W4' is informational because it is **outside the documented
threat model** (single-process; cooperative-lock surface). Two
concurrent import-replace processes against the same session id
are not a documented operating mode. The proposal's lock surface
is what excludes the case in normal operation. Audit may re-raise
W4' as a contract-level finding if the threat model is widened in
Round 3; flagging here so Phase 5 picks the resolution before
implementation.

---

## 10. Nits (severity LOW)

### N1 — exit-namespace 16/17 omission note (carry-over from Rev 1)

§5 lists exits `0` / `1` / `2` / `10`–`15`. Shared namespace at
`06-session-override-contract.md:106-111` also reserves `16`
(lock-token-invalid) and `17` (lock-expired). Import-replace
acquires its own lock under owner `"import-replace"` (§4 step 2)
and does not accept caller-supplied lock tokens, so 16/17 are not
reachable on this surface. Neither §5's preamble nor §13's row 1
says so. A one-line note ("16 and 17 are pause/resume-handshake
token vocabulary; not reachable on this surface") would close the
small ambiguity. Drafting only.

### N2 — §7 #5 conditional `source_file` write (carry-over from Rev 1)

§7 #5 reads "Set `source_file` to the replaced `jsonl_path` when
the current schema/helper supports it; otherwise keep existing
ingest helper behavior if the column is not meaningful in this
branch." Given A1 commits to "earlier Initiative 06 surfaces land
before import-replace," the schema state at merge time should be
known: `session_turns.source_file` exists today (problem-map §3 #5
records `source_file = ''`). Recommended fix: commit
unconditionally to `source_file = jsonl_path` and remove the
hedge. Drafting.

### N3 — §1.1 "validated and narrowed" wording (carry-over from Rev 1)

§1.1 line reads "approved register validated and narrowed from
`research/06-import-replace-problem-map.md` §7." Counts match
(A1–A10 in both). "Narrowed" reads as a count reduction.
Recommended fix: "consolidated and re-themed from the problem-map
draft, with the same row count." Drafting.

---

## 11. Summary

- **Audit closure:** AIR-R2-F01 closed at the scope level on all
  three required-change axes — journal payload now persists
  resolved recovery identity (chain_id, active_segment_id,
  provider_name, storage_type) plus a journal-attached
  `canonical_records_path`; success-flow reordered so journal
  deletion is the LAST step, strictly after postimage_sha256
  verification, fresh export round-trip verification, and SQLite
  commit; four new T-rows in §9 cover recovery scenarios
  (rename-only, ambiguous-hash, canonical-records-preserved,
  no-delete-before-verify).
- **R1 closures:** AIR-R1-F01..F04 all still hold. F02 is
  strengthened by Rev 3; F01/F03/F04 untouched and intact.
- **Race-freeness:** Rev 3's expanded journal + reordered flow
  are race-free for the documented threat model (single-process
  crash, cooperative SessionLock surface, eight enumerated crash
  states in §8). Every crash state has a deterministic recovery
  branch.
- **Anti-scope:** eight harness/initiative clauses still intact.
- **Cross-feature constraints:** all rows in §13 still satisfied;
  the "Durable journal closes …" row is tightened.
- **Coverage:** complete; problem-map §1–§7 still maps with
  additive coverage on AIR-R2-F01 closure surfaces.
- **Single-PR boundary:** still justified after the journal
  payload growth, side-file, and quarantine directory. Four
  split candidates (A/B/C/D) all produce dead intermediate state
  or reopen audit findings.
- **No regression:** Rev 2 anti-scope, constraints, coverage,
  single-PR boundary, and three nits all hold. Rev 2 W1/W2/W3'
  carried; Rev 2 W4 expanded to W4' (now also covers the
  canonical_records_path side-file race) — both halves still
  outside the documented threat model.
- **Findings:** none at MEDIUM or higher.
- **Watch-flags:** four total — W1 stale-temp cleanup ordering
  (carry), W2 schema-probe flag flip (carry), W3' renderer
  round-trip parity (carry; load-bearing under Rev 3 step 8),
  W4' journal/lock ordering + side-file race (expanded from
  Rev 2 W4; outside documented threat model).
- **Nits:** three carry-over drafting items (N1, N2, N3 from
  Rev 1).

**Verdict: LOW.** Rev 3 closes AIR-R2-F01 at the scope level
without expanding the public surface, regressing anti-scope, or
breaking the single-PR boundary. R1 closures all still standing.
The expanded journal + reordered flow are race-free for the
documented threat model. Phase 5 dispatch can proceed; W4' is the
only watch-flag worth pinning at hookpoint time, and it is
outside the documented threat model so does not block this gate.
