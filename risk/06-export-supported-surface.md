# 06-export — Phase 4 Supported-Surface Risk Report (Rev 1)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

`proposals/06-export.md` Rev 1 reduces a concrete current-state
observability gap on the supported CLI surface (no command emits a
canonical transcript by session id; no source-preimage audit signal;
no "no partial stdout on error" guarantee) and pays for it with a
small, additive, read-only subcommand and one read-only Rust parser
module. A1-A8 in §1.1 hold against the approved problem map and the
already-merged 06-locate Phase 8 LOW verdict. Migration cost is zero
on user state; rollback is uninstall-or-avoid; observability is the
JSONL/stderr-JSON pair already established by `session locate`. No
adjacent supported path (`session locate`, `trace --json`, `resume`,
`repl --resume`, top-level `--resume`, hidden `resume-list`,
`migrate-db`, `migrate-config`, direct CLI ingestion) is BROKEN or
DEGRADED. One known dependency on `06-schema-probe`'s read-only
`StateDb` open is correctly conditioned by §8 and §13; one
sub-finding on `locate_transcript`'s `STATE_DIR` side effect is
correctly flagged for Phase 5 resolution. Termination signals do not
fire. Three cosmetic findings recorded below; none block Phase 5.

## Concern 1 — Assumption invalidation check

A1-A8 evaluated against the approved `research/06-export-problem-map.md`,
the merged 06-locate branch surfaces, the 06-schema-probe Phase 8
artifacts in this repo's git log, and the harness request at
`agent-harness/tmp/scratch/agent-runner-feature-requests/02-session-export.md`.

| ID | Verdict | Evidence used |
| --- | --- | --- |
| A1 (locate before export, supplies `SessionMetadata` etc.) | HOLDS | Initiative sequence at `06-session-override-contract.md:41-50` puts locate first and export third. Locate Phase 8 supported-surface verification at `worktrees/06-locate/risk/06-locate-supported-surface-pr.md` returned LOW; `SessionMetadata` field set is locked. Proposal §1.1 A1 names the same fields the consumed locate API exposes. |
| A2 (schema-probe before export, supplies read-only `StateDb` open) | HOLDS | Initiative sequence at `06-session-override-contract.md:44-50` puts schema-probe second. Local `git log` shows 06-schema-probe through Phase 8 ahead of 06-export's Phase 3 (`risk(06-schema-probe): Phase 4 Round 2 LOW × 4`, `feat(06-schema-probe): Phase 6 Step 6c`, `review(06-schema-probe): Phase 8 synthesis PR comment`). The proposal's §4 step 3, §8, and §13 all condition on the read-only open; no proposal section starts from today's mutating `StateDb::open_default()`. |
| A3 (canonical source = provider JSONL, not `session_turns`) | HOLDS | Harness explicitly says "one canonical transcript record, not one `session_turns` quota row" (`02-session-export.md:20`); problem map §1.23 confirms `session_turns` stores no content/offset/line/hash; problem map §2.2 confirms batch ingestion writes empty `source_file`. Proposal §4, §6, §7 all enforce this — no `session_turns` fallback path is reachable from §4. |
| A4 (per-record source metadata computable at read time from raw JSONL bytes) | HOLDS | The four required fields (`line`, `byte_start`, `byte_end`, `sha256`) are pure functions of the raw byte stream. §3 D1 mandates them on every record; §6 carves out `jsonl.rs` as the byte-tracking scanner. The invalidator (canonical record merging/splitting native records) is not triggered by v1's chunk schema in §3 D2: text/tool_call/tool_result chunks are 1-to-1 with native records they originate from. |
| A5 (`SessionStorageType` is sufficient for v1 parser dispatch) | HOLDS | Locate emits exactly `claude_code`, `codex_session`, `other` (`worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:23-39`). The two adapter scripts at `scripts/claude-code-turns:57-86` and `scripts/codex-turns:56-87` confirm materially different native line shapes per storage. §4 step 6 fail-closes `Other` to exit `12`; the cross-format leakage invalidator is fail-closed by the same step. |
| A6 (provider JSONL line order is stable conversation order) | HOLDS | Existing adapters walk line-by-line; Codex synthesizes ids from `<file>:<line_no>` because payload ids may be null (problem map §1.36). §4 step 9 and §9 ordering-test row make this fail-closed — timestamp regression is exit `15` rather than re-sorting. The "benign clock skew" residual is acknowledged in §12 and pinned by the ordering test row in §9. |
| A7 (Claude compaction via `isCompactSummary`; Codex compaction not v1) | HOLDS | Problem map §1.45-1.46 documents the existing `isCompactSummary` precedent in compaction backfill and Initiative-05 migration. §4 step 8 + §9 D4 rows pin the Claude live-state path. Codex full-transcript path is acknowledged in §12 as a residual; the proposal does not promise Codex compaction-aware export. The invalidator names a specific Phase 5 finding that would unlock follow-up work, not v1 fault. |
| A8 (`sha2` may become a direct Rust dep) | HOLDS | `src-tauri/Cargo.lock:3142-3149` already carries `sha2` transitively; problem map §1.47 confirms it is not yet a direct dep. No project convention forbids it; the workflow's `no-deferred-stubs.md` does not block adding a direct dep that is required for the v1 contract. §12 names the invalidator (dep-policy rejection) as a Phase 5 surfaceable concern. |

**Termination signal #1 (`invalidated-assumption`) does not fire.**

## Concern 2 — Net value on the current supported surface

### Risks reduced (problem-map §6 entries this proposal retires)

| §6 entry | Retired by |
| --- | --- |
| §6 #1 No CLI command emits canonical transcript by session id | §2 subcommand surface (`agents session export <session-id>`) |
| §6 #2 No CLI surface combines ownership + storage + transcript path + canonical content | §3 record schema (carries `session_id`, `provider_name`, `source.storage_type`, `source.jsonl_path`, `content`); §4 step 5 reuses `locate_session_metadata` for the ownership/path leg |
| §6 #3 Per-record `line`/`byte_start`/`byte_end`/`sha256` not persisted/emitted | §3 D1 requires all four on every record; §6 `jsonl.rs` owns the byte-level scanner |
| §6 #4 No structured distinction between safe-unsupported and abort-unsafe records | §3 5-condition `unsupported_record` gate; §5 maps unsafe to exit `15`; §9 row pins both branches |
| §6 #5 No source-preimage audit signal | §3 D1 `source.sha256` over exact native line bytes excluding terminator |
| §6 #6 No "no partial stdout transcript on error" guarantee in any current command | §4 step 10 (validate full `Vec<CanonicalRecord>` before writing); §8 (no partial stdout); §9 "No partial stdout on parser error" row |
| §6 #8 Compaction-boundary export semantics unspecified | §4 step 8 D4 (live transcript from latest supported boundary; full transcript otherwise); §9 D4 rows |

§6 #7 (storage-type support only indirectly observable) is intentionally
deferred to `06-schema-probe`'s feature-flag surface; the proposal's
§11.1 + README §10 cover the export-specific portion (the supported
v1 set is documented). §6 #9 (locator-failure durability) is out of
scope per §7. Net retired: **seven §6 entries**, with two cosmetic
deferrals correctly delegated.

### Blast radius added

| New failure mode | Status | Guard |
| --- | --- | --- |
| Claude/Codex JSONL drift | Acknowledged §12 | v1 fixtures + fail-closed exit `15` on malformed records |
| Codex compaction not live-state aware in v1 | Acknowledged §12 | Codex emits full transcript; explicit residual; revisable in Phase 5 |
| Memory cost proportional to transcript size | Acknowledged §12 | Required by "no partial stdout" guarantee; bounded by the same upstream constraints `migrate-db`/migration already accept |
| Timestamp regression fails closed | Acknowledged §12, pinned by §9 | Benign clock skew rejected with exit `15`; documented in §10 README updates |
| `SessionStorageType::Other` rejected even when path exists | Acknowledged §12 | Exit `12` with stderr JSON; aligned with locate's `unsupported-storage` semantics |
| `sha2` becomes a direct dependency | Acknowledged §12 | Already transitively present; minimal Cargo.toml diff |
| Native byte-tracking scanner is new code | Inherent | Confined to `src-tauri/src/session_export/jsonl.rs`; testable independently per §9 D1 row |

All seven items are bounded to the new module or fail-closed at exit
sites; none mutate state or change adjacent path output. Net value is
clearly positive — seven retired §6 entries vs seven small,
fail-closed, additive failure modes confined to a new read-only
subcommand.

**Termination signal #2 (`non-positive-value`) does not fire.**

## Concern 3 — Adjacent supported-path continuity

| Path | Verdict | Evidence |
| --- | --- | --- |
| `agents session locate` | PRESERVED | §11.1 explicitly: "`session locate` remains metadata-only". Export consumes `locate_session_metadata` as a function call (§4 step 5); no schema or behavioral change to locate. Locate Phase 8 LOW already pinned this surface. |
| `agents trace --json` | PRESERVED | §11.1 explicitly: "`trace --json` remains invocation-tree scoped and placeholder-only for inline transcripts". §11.1 also clarifies in §10 README that export is the supported transcript reader, leaving trace's contract unchanged. |
| `agents resume` | PRESERVED | §7 anti-scope and §8 side-effect contract forbid provider spawn; problem map §3.6 lists resume as adjacent only because it consumes the same `ResolvedResume`. Export does not touch `run_resume`. |
| `agents repl --resume` | PRESERVED | Same as resume — §7/§8 forbid provider launch. |
| Top-level `--resume` | PRESERVED | Same as resume. |
| Hidden `agents resume-list` | PRESERVED | Not referenced or modified by the proposal. §7 anti-scope reaffirms. |
| `agents migrate-db` | UNCOUPLED | §11.1: "`migrate-db` and `migrate-config` are not called or coupled." §13 cross-feature constraints reinforce. |
| `agents migrate-config` | UNCOUPLED | Same. |
| Direct CLI ingestion (`scan_provider`, adapter scripts) | PRESERVED | §11.1: "direct CLI ingestion remains adapter-script based". §7 anti-scope explicitly forbids running scans, turn scripts, or refreshing cursors from export. |
| Future 06-import-replace | FORWARD-COMPAT | §6 D7 + §11.1 expose `read_canonical_transcript`, `CanonicalRecord`, `RecordSource`, `ExportError` as the round-trip reader for import-replace. |

No path is BROKEN or DEGRADED.

## Concern 4 — Migration / rollback / observability concreteness

§11.1 makes three load-bearing claims; all hold under the proposal:

- **No user state migration**: VERIFIED. Proposal §11.1 names this; §3
  defines no schema changes; §6 is a new module only; §13 affirms no
  coupling to `migrate-db` or `migrate-config`. Existing sessions are
  exportable iff locate can resolve them and their JSONL matches a v1
  parser; failures are fail-closed at known exit codes.
- **Rollback by uninstall/revert**: VERIFIED. The subcommand is
  additive; §8 forbids any durable write (DB, transcript, temp,
  cursors); removing the binary or skipping the subcommand has no
  cleanup step. The new `session_export` module lives under
  `src-tauri/src/`; deleting it leaves the rest of the binary
  unchanged.
- **Observability surface = success JSONL + stderr JSON errors**:
  VERIFIED. §8 forbids telemetry, invocation rows, trace records,
  durable warnings, and JSONL writes; §5 pins exit codes to a closed
  set; §10 README cites the same exit codes; the stderr JSON shape
  matches locate's pattern (`code` + `message`).

The "no partial stdout on error" claim is mechanized in §4 step 10
(validate the full `Vec<CanonicalRecord>` before writing) and pinned
by the §9 end-to-end test row that asserts zero stdout bytes when a
later line is malformed. Concrete and testable.

## Concern 5 — Harness acceptance criteria coverage

Cross-check against `02-session-export.md` §"Acceptance criteria":

| Harness AC | Coverage |
| --- | --- |
| `agents session export <known-session>` emits valid JSONL, exit `0` | §2 + §3 + §9 row "Locate reuse and resolver pass-through" |
| Export order stable and chronological within canonical transcript | §4 step 9 D5 + §9 ordering row |
| Source metadata includes offsets and content hashes for harness audit | §3 D1 + §9 "D1 source offsets and SHA-256" row |
| Claude Code and Codex fixtures export without call-site native-shape knowledge | §6 (`session_export` module hides parsers) + §9 "D2 text/system/tool shape" row |
| Unsupported records → explicit `unsupported_record` when safe; exit `15` otherwise | §3 5-condition gate + §5 + §9 "Unsupported native record policy" row |
| Missing/ambiguous/unsupported sessions → stable error codes, no partial stdout | §5 + §9 "Not found / ambiguous / unsupported storage mapping" + "No partial stdout on parser error" |
| Tests prove read-only against state DB and transcript files | §9 "Read-only behavior" row (DB rows / file mtimes / dirs snapshot) |

All seven harness acceptance bullets are covered by the proposal's
test-intent track. Coverage verdict: **complete.**

## Concern 6 — Initiative-06 sequencing forward-compat

Downstream consumers in initiative 06:

- **06-import-replace** consumes `CanonicalRecord`, `RecordSource`,
  `ExportError`, `read_canonical_transcript` (proposal §6 D7 +
  §11.1). Round-trip use is named in
  `06-session-override-contract.md:48-56`. Forward-compat preserved.
- **06-pause-handshake** is independent of export's read-only path
  (§7/§8 forbid lock observation in v1 because export does not
  mutate). Forward-compat preserved.
- **06-schema-probe** is upstream of export, not downstream;
  schema-probe's read-only `StateDb` open is consumed by export
  (§4 step 3). Already merged through Phase 8 per local git log.

Initiative-wide error namespace (§13) reuses `10`, `11`, `12`, `15`
exactly as `06-session-override-contract.md:106-111` specifies. No
collision with reserved siblings 13/14/16/17.

## Concern 7 — Side-effect contract vs `locate_transcript` `STATE_DIR` creation

Problem map §1.5, §1.29, §2.5 document that today's `locate_transcript`
helper creates `STATE_DIR` before invoking the locator script
(`src-tauri/src/sessions/mod.rs:183-187`). The harness request at
`02-session-export.md:54-64` requires export to be read-only including
"no temp files." This is the only explicit gap between the proposal's
declared read-only contract and current code.

The proposal handles it correctly:

- §8 names the gap directly: "Unlike locate Rev 3's caveat, export
  depends on the 06-schema-probe read-only state open. If the current
  locator helper still creates `STATE_DIR`, Phase 5 must either
  identify a read-only locator path or revise this proposal; export's
  side-effect contract is stricter than locate's."
- §13 cross-feature constraint table confirms the dependency on
  schema-probe's read-only open.
- §11.1 declares observability post-condition as success JSONL +
  stderr JSON only; this is consistent with the eventual read-only
  path.

This is conditioned, not hand-waved. Phase 5 (hookpoint research) is
the correct place to either (a) thread an explicit read-only locator
variant, (b) accept `STATE_DIR` creation as a side-effect carve-out
matching locate's existing carve-out (in which case §8 must be
revised in a follow-up Rev), or (c) revise the harness contract.
Recorded as **F01** below.

## Findings

- **F01 (advisory; non-blocking)** — `locate_transcript`'s
  `STATE_DIR` directory creation is the one current behavior that
  the proposal's stricter side-effect contract does not yet
  reconcile. §8 explicitly conditions on Phase 5 resolution. Phase
  5 must either (a) introduce a read-only locator variant that
  skips the directory creation when it already exists / when no
  script is configured, (b) revise §8 to inherit locate Rev 3's
  carve-out for `STATE_DIR` creation as a non-mutating
  bootstrap step, or (c) document the deviation in
  `risk/06-export-test-residuals.md` if it survives to Phase 6b.
  Not a termination signal because §8 explicitly conditions on
  Phase 5 resolving it; the "Read-only behavior" §9 test row will
  catch any unconditioned deviation by snapshotting directory
  listings before/after export.

- **F02 (cosmetic)** — Problem-map §6 #7 ("storage-type support
  only indirectly observable") is partially retired by §11.1 +
  §10 README documenting the supported v1 storage set. A
  programmatic feature-flag surface for "which storage types
  export supports" lives in `06-schema-probe`'s schema-probe
  output (per `06-session-override-contract.md:44-50`), not in
  export. This is correct sequencing, not a finding against the
  proposal. Recorded for completeness so reviewers do not expect
  export to expose a parser-feature list.

- **F03 (cosmetic)** — A6's "real transcripts with benign clock
  skew would be rejected" residual (acknowledged in §9 ordering
  row and §12) is a legitimate trade — it preserves the strict
  fail-closed contract the harness asked for. Worth surfacing in
  §10 README so harness consumers know that timestamp regression
  is a definitive failure rather than a normalization opportunity.
  README §10 already lists this in spirit ("compaction behavior
  ... live transcript from latest supported boundary") but does
  not explicitly say "regressing timestamps fail." Optional README
  copy-edit; not a contract problem.

## Verdict rationale

**Termination signal #1** does not fire — A1-A8 hold against problem
map evidence, locate's already-merged Phase 8 LOW verdict, and
schema-probe's already-merged Phase 8 surface. No proposal section
depends on a falsified assumption.

**Termination signal #2** does not fire — seven §6 entries retired
(bringing canonical transcripts, source-preimage audit, structured
unsupported-record vocabulary, "no partial stdout" guarantee, and
explicit compaction semantics to the supported surface for the first
time). Blast radius is seven bounded, fail-closed, additive failure
modes confined to one new read-only module. Net value is clearly
positive on the current supported surface.

**Standard verdict: LOW.** Adjacent supported-path continuity is
preserved across all ten enumerated paths (concern 3); migration
burden is zero on user state and rollback is uninstall-or-avoid
(concern 4); harness acceptance bullets are completely covered by
the proposal's test-intent track (concern 5); initiative-06
sequencing forward-compat is preserved for `06-import-replace`,
`06-pause-handshake`, and `06-schema-probe` (concern 6); the one
known side-effect gap (`STATE_DIR` creation by `locate_transcript`)
is correctly conditioned on Phase 5 resolution (concern 7).

**Recommendation:** Phase 5 (hookpoint research) may proceed.
Phase 5 must record F01 explicitly so the read-only contract is
either mechanized or carve-out-documented before Phase 6b emits
the read-only-behavior test fixture.

**Final verdict: LOW. Termination signal: none.**
