# 06-schema-probe — Phase 4 Supported-Surface Risk Report (Rev 1)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

The proposal lands `agents session schema-probe` as a strictly additive,
physically read-only CLI surface. Every assumption in §1.1 holds against
problem-map evidence; six of the ten observability gaps in
`research/06-schema-probe-problem-map.md` §6 are closed, three are
addressed via structured exit/JSON, and the one remaining gap (GUI DB
divergence) is honestly residualized. Adjacent paths (trace, resume,
repl, migrate-db, migrate-config, hidden resume-list, direct CLI
ingestion, GUI Tauri commands) are not retrofitted in v1 (D7) and
remain bit-identical. Harness acceptance criteria (1-7) are all
covered. The largest soft spot is a sequencing dependency, not an
assumption fault: until a future mutating-open PR stamps
`PRAGMA user_version`, every existing DB returns exit `14`, so the
near-term harness experience is permanent refusal — but that refusal
is the product behavior the harness explicitly asked for, so net
value remains positive. Phase 5 (hookpoints) and Phase 6
(implementation) may proceed with the residual coordination items
below.

## Concern 1 — Assumption walk on §1.1

| ID | Verdict | Note |
| --- | --- | --- |
| A1 read-only open is feasible | **HOLDS** | `StateDb` is a thin `rusqlite::Connection` wrapper (problem-map §1 #9). SQLite `mode=ro` + `PRAGMA user_version` + `sqlite_master` + `PRAGMA table_info`/`index_info` are all non-mutating. §6.1 D3 is concretely scoped: no parent-dir create, no `journal_mode=WAL`, no schema-ensure, no backfill. Invalidator (compatibility requires mutation) does not fire — A6 confines compatibility to structural-plus-version checks. |
| A2 `PRAGMA user_version` is the right source | **HOLDS (REPHRASED)** | The slot is correct (problem-map §5 #7-9: currently unused, no competing metadata table). The honest read is that v1 *defines* the contract; *populating* it on legacy DBs is a separate mutating-open PR's job. The proposal is explicit: probe never stamps; mutating paths "may" stamp. This is not a fault — it is an acknowledged sequencing seam (§12 residual #1) — but see Findings #1. |
| A3 compiled features are binary-bound | **HOLDS** | Command support is compiled into clap arms/modules (problem-map §2 #13). D2a hardcodes the map so clap-only/Cargo-feature accidents cannot leak into the contract. Each sibling PR owns its own row update. Invalidator (feature safety depends on runtime config) does not fire because schema-probe reports *compiled* support, not config validity. |
| A4 CLI default DB is the right v1 target | **HOLDS** | Harness invokes the CLI binary; CLI callers consistently use `StateDb::open_default()` at `dirs::data_dir()/oulipoly-agent-runner/state.db` (problem-map §1 #11-12). GUI/Tauri DB divergence is honestly out-of-scope (§12 residual #3). Invalidator does not fire — the harness request itself targets CLI state. |
| A5 reviewable parallel to 06-locate | **HOLDS** | §2 handles both branches: extend locate's `SessionSubcommands` if present, else introduce the group with only `SchemaProbe`. The "final merged surface must have one `session` group" rule prevents top-level alias drift. §3.3 D5 storage vocabulary is duplicated locally if needed and reused if locate is upstream. |
| A6 structural+version inspection sufficient | **HOLDS** | Required tables, columns, and indexes are enumerated in §6.2 and traceable to current `db.rs` (problem-map §1 #13-22). Compatibility deliberately does *not* claim chain-backfill repair completeness; the partial-chain skip condition surfaced in problem-map §2 #6 is honestly carried forward as §12 residual #2. Invalidator (compatibility requires data invariants) does not fire — schema-probe scopes itself out of data-correctness claims. |

**Termination signal #1 (`invalidated-assumption`) — DOES NOT FIRE.**

## Concern 2 — Net value vs. problem-map §6

| §6 gap | Closed by | Status |
| --- | --- | --- |
| §6.1 no CLI-level "DB compatible" check | §3 success JSON + §5 exit `14` | Closed |
| §6.2 no way to discover `schema_version`/`user_version` | §3 `state_db.schema_version`/`user_version` | Closed |
| §6.3 no binary feature list | §3 `features` map (D2a) | Closed |
| §6.4 no CLI output naming supported storage types | §3 `supported_storage_types` (D5) | Closed |
| §6.5 no structured refusal report | §5 stderr JSON with `code: schema-incompatible` + failed booleans | Closed |
| §6.6 no no-side-effect path to ask where the default DB is | §3 `state_db.path` + `StateDb::default_path()` | Closed |
| §6.7 GUI/CLI path divergence not surfaced | §11.1 honest exclusion + §12 residual | Residualized (acceptable, A4 scope) |
| §6.8 `trace --json` does not observe binary/schema | new surface owns this | Closed |
| §6.9 no command reports DB path without opening it | §3 + §4 step 4 (missing-DB success path) | Closed |
| §6.10 no structured missing/old/incompatible/inaccessible distinction | §5 exit matrix `0`/`1`/`2`/`14` with three subcodes | Closed |

Nine of ten §6 gaps are closed; the tenth is honestly residualized
within the v1 scope statement. The proposal also retires problem-map
§2 risks #1 (mutating open inspection), #2 (WAL-on-open hint), #3
(silent schema drift on newer binary), #7 (no public compatibility
surface), and #18 (missing-DB invisible). It does not retire #5/#6
(unconditional backfill, partial-chain skip) nor #19 (`CREATE IF NOT
EXISTS` masking absence) — those remain attached to mutating `open`,
which D7 deliberately does not modify.

**Net value:** positive. The harness can pin `schema_version`, gate on
`features`, refuse on exit `14`, and refuse-write-ops on
`safe_for_import_replace == false` from the day this PR lands, even
before sibling features ship.

**Termination signal #2 (`non-positive-value`) — DOES NOT FIRE.**

## Concern 3 — Adjacent path preservation

§1 line 28-33 and §7 enumerate unchanged surfaces: `trace`, `repl`,
`resume`, top-level `--resume`, hidden `resume-list`, `migrate-db`,
`migrate-config`, the existing mutating `StateDb::open`, all sibling
Initiative 06 commands, GUI/Tauri state commands, `session_scan`,
`quota_check`, direct CLI ingestion via `turn_script`. D7 forbids
retrofitting any existing read-intent command to `open_read_only` in
v1. §13 cross-feature checklist confirms no auto-resume, no provider
spawn, no quota refresh, no config edits, no `migrate-config`
coupling. §8 side-effect contract forbids transcript reads, config
edits, invocation/telemetry rows, adapter state, and quota/discovery
mutation. **PRESERVED.**

## Concern 4 — Migration / rollback / observability accuracy

- **Migration:** §11.1 correctly reports that existing DBs have
  `user_version = 0` and harness must treat exit `14` as refusal.
  This is honest. The proposal does *not* claim schema-probe stamps
  the version; it explicitly assigns stamping to a future mutating
  schema-ensure path (§1 line 25-26, §12 residual #1).
- **Rollback:** schema-probe writes no durable state; revert is
  binary uninstall. A future-stamped `user_version` is inert under
  the prior binary (no compatibility shim required, consistent with
  `no-backwards-compatibility.md`).
- **Observability:** stdout JSON on success, stderr JSON on refusal,
  no telemetry, no invocation/trace/quota rows, no transcript reads.
  Side-effect contract in §8 is the binding promise; §9.1 includes a
  particular-integration test that snapshots config/data sentinels
  and row counts. Accurate.

## Concern 5 — Harness acceptance criteria coverage

Walking the seven checkboxes in
`agent-harness/.../05-session-schema-probe.md:55-62`:

1. Binary identity + DB path + schema/user_version + features +
   storage types — §3 covers all five.
2. Read-only open, no schema-ensure/migration — §6.1 D3 + §8.
3. Missing/incompatible tables/columns/indexes → exit `14` structured
   stderr — §5 + §9.1 row "D6 exit mapping for older/newer/missing
   structures."
4. Feature flags reflect compiled support for
   locate/export/import-replace/pause-handshake — §3.2 D2.
5. Tests verify no mtime/schema change — §9.1 row "D3 read-only open
   has no schema/backfill side effects" (mtime + content snapshot).
6. README documents JSON shape + refusal — §10.
7. Harness can decide adapter-vs-fallback — yes via `features` plus
   `safe_for_import_replace`.

**All seven covered.**

## Concern 6 — Forward-compat for 06-locate's residual A6

06-locate (`/home/nes/projects/agent-runner/worktrees/06-locate/proposals/06-locate.md:245`,
:310) carries the caveat: "Physical read-only DB open is not in
06-locate. Current `StateDb::open` side effects remain until
06-schema-probe introduces the read-only variant." The initiative
contract (`initiatives/06-session-override-contract.md:118-120`)
binds: "Read-only `StateDb` open variant lands in 06-schema-probe."

§6 of this proposal lands `StateDb::open_read_only(&Path)` with the
exact semantics 06-locate's A6 residual was waiting on. **The API
unblocks the residual.** §7 D7 explicitly does *not* retrofit locate
or trace onto the new variant in this PR. That is consistent with
the initiative wording ("variant lands" ≠ "all callers retrofit") and
with `no-deferred-stubs.md` (no half-finished retrofit), but it does
mean 06-locate's residual A6 remains documented in locate's proposal
even after schema-probe merges. A separate cleanup PR is required to
flip locate to `open_read_only`. See Findings #3.

## Findings

1. **Stamping-PR coordination (medium-impact, near-term).** The
   proposal's near-term harness experience is permanent exit `14`
   because no shipped command stamps `PRAGMA user_version`.
   Functionally this is the requested "refuse rather than corrupt"
   behavior, so net value stays positive — but Phase 5 hookpoints
   should record an explicit pointer to the future PR that adds
   stamping to `StateDb::open` and/or `migrate-db`, and §10 README
   text should make the user-facing recovery path concrete (e.g.
   "until release X, exit 14 is expected; harness should fall back").
2. **`safe_for_import_replace` is permanently `false` in Rev 1** by
   construction (both gating features are `false`). The structured
   JSON does let callers distinguish "false because features missing"
   from "false because DB incompatible," but the README in §10
   should call this out so harness authors do not interpret the
   boolean as a DB diagnostic in v1. Minor.
3. **D7 leaves locate's A6 caveat documentary, not retired.** The
   open_read_only API lands and is consumed by schema-probe's own
   call site; locate's A6 residual remains attached to locate's
   proposal until a follow-on retrofit PR. Track this as a
   coordination item between Phase 6 implementation and any future
   06-locate Rev. Not a blocker.
4. **Storage-vocabulary duplication risk.** §3.3 D5 already
   addresses this: if locate has landed, reuse the public enum;
   otherwise duplicate locally. Phase 5 should pin which branch the
   implementation expects so the merge order is unambiguous.
5. **WAL/permission read variability is platform-dependent.** §9.1
   row "D3 WAL read behavior" honestly residualizes this; no action
   required beyond the current note.

**Verdict: LOW. No termination signal fires. Proposal is cleared for
Phase 5 hookpoints with the five findings above carried forward as
coordination items, not blockers.**
