# 06-export — Phase 4 Scope Risk Assessment (Rev 1)

**Assessor:** `claude-opus` (scope)
**Verdict:** **LOW** — Rev 1 ships an additive, well-bounded scope: one
new `agents session export` subcommand attached to the locate-introduced
`SessionSubcommands` enum, plus one new read-only Rust module
`src-tauri/src/session_export/`. No existing surface (resume, repl,
trace, migration, locate, sessions adapter, GUI) is mutated. Anti-scope
(§7) is exhaustive and consistent with the side-effect contract (§8).
All ten cross-feature constraints in §13 hold under the proposed
mechanics. The §1.1 register replaces the problem-map draft register
correctly; the two register expansions (A6 ordering, A8 `sha2`) are
surfaced honestly with invalidators rather than being slipped in. No
finding rises to MEDIUM. Three LOW drafting nits captured in §7
(boundary-summary canonical shape, in-memory Vec residual phrasing,
Other-storage in `RecordSource` enum).

---

## 1. Scope statement audit

| Aspect | Result | Evidence |
| --- | --- | --- |
| One CLI surface added | confirmed | §1, §2: `session export <session-id> [--format canonical-jsonl]` extends locate's `SessionSubcommands` enum; no second top-level command. |
| One Rust module added | confirmed | §1, §6: `src-tauri/src/session_export/` with `mod.rs`, `jsonl.rs`, `claude_code.rs`, `codex_session.rs`; consistent placement with locate's `session_metadata/` precedent. |
| Existing surfaces unchanged | confirmed | §1: "Existing resume/repl/trace/migration/locate behavior remains unchanged." Cross-checked against §7 anti-scope (no trace JSON edits, no `--inline-transcript` change, no migrate-db coupling) and §11.1 (no GUI, no daemon, no server). |
| One direct dependency added | confirmed, justified | §1.1 A8 + §12: `sha2` direct dep; already transitive in `Cargo.lock`; flagged as residual subject to Phase 5 dep-policy validation. |
| Register hygiene | confirmed | §1.1: "consumes the approved current-state map at `research/06-export-problem-map.md`; this proposal's §1.1 register replaces the draft register in that map." No competing register kept. |

Scope statement holds. The only design surface introduced is contained
inside the new `session_export` module; the only external touch points
are the locate `SessionMetadata` API (read), the schema-probe read-only
`StateDb` open (read), and the `SessionSubcommands` enum (one new
variant).

## 2. Anti-scope integrity check

§7 anti-scope clauses walked against §3–§8 mechanics:

| Anti-scope clause | Verified by | Note |
| --- | --- | --- |
| No provider spawn / auto-resume / login / quota refresh / model discovery | §4 step-list (no launch step), §8 second bullet | resolution flow ends at validated `Vec<CanonicalRecord>` + stdout write. |
| No DB writes / cursor writes / transcript writes / temp files / state repair / migrations / lock commands | §4 step 3 (read-only DB open from schema-probe), §8 first/second bullets | strict superset of locate's read-only contract. |
| No `session_turns` fallback for content / source / ordering / parser dispatch | §4 step 7 (D3) + closing paragraph after §4 step 10 | "raw JSONL file is the transcript source of truth"; §13 row 2 also pins this. |
| No `SessionStorageType::Other` parser in v1 | §4 step 6, §5 (exit `12`), §3 schema enum row | failure path is fail-closed exit `12`; not a silent skip. |
| No alternate formats (pretty JSON / Markdown / native JSONL / archives) | §2 `ExportFormat::CanonicalJsonl` only; clap rejects others as exit `2` | no `--pretty`, `--inline`, or archive flags introduced. |
| No byte-for-byte provider-native promise | §3 schema (canonical chunk variants) + §7 bullet 5 | preserves preimage via `source.sha256` of native bytes, but emitted JSON is canonical. |
| No import / replace / append / truncate / rewrite | §6 declares `read_canonical_transcript` (read-only) only | reusable types defined for `06-import-replace` to consume, but no write API. |
| No GUI / Tauri frontend surface | §11.1 first paragraph | no Tauri command, no `lib.rs` invoke wiring beyond `pub mod session_export;`. |
| No preservation of provider-private metadata beyond canonical schema | §3 chunk variants, §7 bullet 7 | unsupported records carry `unsupported_record: true` placeholder, not native payload passthrough. |

All clauses hold. No drift between anti-scope text and mechanics
elsewhere in the proposal.

## 3. Cross-feature constraint compliance

§13 row-by-row verification against
`initiatives/06-session-override-contract.md:106-122`:

| Constraint | Compliance | Verification anchor |
| --- | --- | --- |
| Shared error namespace (`10`/`11`/`12`/`15` for export-relevant cases) | yes | §5 table; export-specific `15` is `malformed-transcript` / `unsupported-record`, not initiative-wide `invalid-input`. Naming is harness-aligned (`02-session-export.md:44-53`). |
| Single ownership via `StateDb::resolve_resume`; no second ownership path | yes | §4 steps 5 + 10 invoke `locate_session_metadata`, which proxies to `resolve_resume`; D3 explicitly forbids ownership reads from `session_turns`. |
| Read-only `StateDb` open variant (06-schema-probe) | yes | §4 step 3 + §8 third paragraph; A2 invalidator names "export starts from today's mutating `StateDb::open_default()` without an accepted exception" as the explicit failure mode if schema-probe slips. |
| Lock observation for import-replace / migration / repl / resume / one-shot once 06-pause-handshake lands | N/A for read-only export | §11.1 reflects that 06-export is locker-free; future pause-handshake observation lands in import-replace, not here. |
| No auto-resume | yes | §7 bullet 1, §8 second bullet; resolution flow has no resume call site. |
| No provider spawn | yes | §7 bullet 1, §8 second bullet. |
| No quota refresh | yes | §7 bullet 1; no scan/refresh job step in §4. |
| No config edits | yes | §7 + §8; config is read via locate, never mutated. |
| No coupling to `migrate-config` | yes | §7 bullet 2, §11.1 third paragraph: "`migrate-db` and `migrate-config` are not called or coupled." |
| Reusable canonical reader for import-replace round-trip | yes | §6: `CanonicalRecord`, `RecordSource`, `ContentChunk`, `ExportError`, `read_canonical_transcript` are public; D7 names import-replace as direct consumer. |
| Harness receives canonical JSONL, not provider-native | yes | §3 schema is canonical; §7 bullet 5 forbids byte-for-byte promise. |

All ten constraint rows hold. No row composition changed; all citations
resolve.

## 4. Net-value and blast-radius framing audit

§1.2 claim is honest:

- **Risk reduction is concrete.** Today the harness must parse private
  Claude/Codex JSONL or read summary rows that omit content
  (problem-map §6 #1–#5). Centralizing the parser behind a stable CLI
  contract removes that obligation from the harness. The benefit is
  not speculative.
- **Blast radius is bounded.** One enum variant, one new module, one
  README section. No edits to existing CLI dispatch beyond the
  locate-introduced `Subcommands::Session` arm; no edits to existing
  state, sessions adapter, trace, or migration code paths.
- **Migration cost is correctly stated as none for user state.** No
  schema migration, no transcript rewrite, no cursor reset.
- **Rollback cost is correctly stated as low.** Additive subcommand
  with no durable state writes; revert binary or avoid the subcommand.
- **Ongoing burden is correctly framed.** Provider JSONL drift is the
  one large recurring cost (§12 first bullet). The proposal does not
  hide this — it explicitly names parser drift as the largest
  residual.

The framing does not overstate value or understate cost. No "free
lunch" claim.

## 5. Watch-flag judgments

| Watch flag | Source | Judgment |
| --- | --- | --- |
| WF1: `STATE_DIR` mkdir side effect inherited via locate | §8 final paragraph | **acceptably escalated.** The proposal explicitly states "Phase 5 must either identify a read-only locator path or revise this proposal." This is honest Phase 5 escalation, not silent scope creep. The export side-effect contract is documented as strictly stricter than locate's; the dependency is named, not hidden. Not a Phase 4 scope finding. |
| WF2: D4 compaction policy is asymmetric (Claude post-boundary suffix; Codex full transcript) | §4 step 8, §12 bullet 2, §1.1 A7 | **bounded asymmetry.** One CLI surface, two storage-type behaviors, both documented. The harness consumer can detect the regime from `source.line` of the first record (boundary line vs. line 1). A7 invalidator names "Codex compaction must be live-state accurate in v1, or Claude changes compaction marker shape" as the trigger to revise. Acceptable for v1 because no stable Codex marker is currently known (problem-map §1 #36, §2 #18); a fail-closed alternative would over-reject Codex sessions. |
| WF3: D5 strict timestamp regression → exit `15` | §4 step 9, §12 bullet 3 | **fail-closed by design.** §12 acknowledges "A real provider transcript with valid causal order but regressing timestamps would exit `15`." The alternative — sort by timestamp — would silently reorder records relative to JSONL append order, which problem-map §1 #35–#36 establishes as today's stable conversation order. Fail-closed is consistent with harness ask "no partial stdout transcript on error" (`02-session-export.md:58-64`). Not a scope finding. |
| WF4: D7 returns `Vec<CanonicalRecord>`, not streaming iterator | §6 D7, §12 bullet 4 | **honest tradeoff.** Required by harness ask "no partial transcript on error" (`02-session-export.md:54-64`); streaming would force partial stdout cleanup on late-record failure. Memory cost residual is named in §12. Internal helpers may stream; public API is buffered. Not a scope finding. |
| WF5: D3 places parsers in Rust (not `scripts/`) | §4 step 7, §6 module list | **bounded module addition.** Justified by harness's preimage requirements (byte offsets, SHA-256, no cursor writes), which existing adapter scripts cannot supply (problem-map §1 #26, §2 #2, §6 #3). Existing adapter scripts are explicitly preserved as "summary-ingestion helpers only" (§4 step 7). Not a duplicate code path: scripts feed `session_turns`; Rust parsers feed canonical export. Two separate consumers, two separate purposes. |
| WF6: New direct `sha2` dependency | §1.1 A8, §12 bullet 5 | **single, transitive-already-present dep.** `Cargo.lock:3142-3149` confirms transitive presence. A8 invalidator names "dependency policy rejects a direct hash crate" as the only failure mode. Not a scope finding; defer to Phase 5 dep-policy gate. |
| WF7: `SessionStorageType::Other` rejected even when locator returns a path | §4 step 6, §12 bullet 6 | **correct fail-closed.** No v1 parser → no contract → exit `12`. Consistent with harness exit-code map (`02-session-export.md:51`). Not a scope finding. |

No watch flag escalates to a Phase 4 finding. Each is either
controlled by an invalidator clause or by the harness contract.

## 6. Drift audit

### 6.1 vs. harness ask (`02-session-export.md`)

| Harness requirement | Proposal coverage | Gap? |
| --- | --- | --- |
| `agents session export <id> [--format canonical-jsonl]` (`:7-21`) | §2 clap shape; same id resolution as locate | none |
| Line-delimited JSON; one canonical record (not summary row) per line (`:20`) | §3 schema, §1 statement | none |
| Minimum record shape with session/provider/turn/role/timestamp/content + source.{storage_type,jsonl_path,line,byte_start,byte_end,sha256} + unsupported_record (`:22-41`) | §3 schema; D1 makes all source fields required; D2 keeps `content` as typed array | none |
| Exit codes `0`/`1`/`2`/`10`/`11`/`12`/`15` (`:44-53`) | §5 table, §13 row 1 | none |
| Read-only: no `state.db` mutation, no cursor updates, no temp files, no provider launch (`:54-64`) | §4 step 3 (read-only DB), §7, §8 | none |
| Stable, chronological export order (`:58`) | §4 step 9 (D5) | none — JSONL file order is stable+chronological for supported storage per problem-map §1 #35–#36 |
| Source metadata sufficient for harness audit/preimage checks (`:60`) | §3 source object; D1 hash-of-native-bytes precision | none |
| Claude Code and Codex fixtures export without call-site native-shape knowledge (`:61`) | §6 reusable reader API; §9 Claude/Codex fixture rows | none |
| Unsupported native records → safe placeholder OR exit `15` if unsafe (`:62`) | §3 unsupported-record 5-condition gate; §5 exit `15`; §9 row "Unsupported native record policy" | none |
| Missing/ambiguous/unsupported sessions → stable error codes, no partial stdout (`:63`) | §5 + §4 step 10 (validate-then-write) | none |
| Read-only proven by tests against state DB and transcript files (`:64`) | §9 row "Read-only behavior" | none |
| Reuse `locate` ownership/path logic; do not re-scan arbitrary files (`:66-72`) | §4 step 5; §13 row 2 | none |
| Trace placeholder `--inline-transcript` not changed (`:70`) | §11.1, §10 README clarification (trace/locate do not emit transcript content) | none |
| `agents resume` / `agents repl --resume` continue to launch providers; export must not imply resume (`:72`) | §7 + §8 | none |

No drift from harness ask; one expansion (D2 typed `content` chunks
beyond the harness's text-only example) is a refinement, not a
deviation, since it preserves text and adds structured tool/result
shapes the harness can ignore.

### 6.2 vs. problem-map draft register (§7)

| Problem-map draft | Proposal disposition |
| --- | --- |
| Draft A1 (locate before export) | A1 carried, evidence enriched with locate Rev 3 module path. |
| Draft A2 (schema-probe before export, read-only `StateDb`) | A2 carried, citation deepened. |
| Draft A3 (canonical source = JSONL, not `session_turns`) | A3 carried, used by D3. |
| Draft A4 (storage type sufficient for parser family) | renumbered to A5; substance preserved. |
| Draft A5 (per-record source metadata at read time) | renumbered to A4; substance preserved. |
| Draft A6 (compaction state sufficient) | narrowed to A7: Claude detectable via `isCompactSummary == true`; Codex deferred unless Phase 5 finds a marker. Narrowing tightens, does not expand. |
| Draft A7 (locate error vocabulary shareable) | folded into §13 row 1 + §5 table (constraint, not assumption). Acceptable migration. |
| — | A6 added: JSONL line order is stable conversation order. New assumption with cited evidence and forward-looking invalidator. |
| — | A8 added: `sha2` can become a direct dependency. New assumption with explicit invalidator. |

Net: 6 problem-map drafts → 8 proposal entries. Two additions are
distinct, narrow assumptions with invalidators; one (A6) is load-bearing
for D5 ordering policy and was not surfaced as a draft assumption — its
addition is correct, not improper. Register expansion is controlled.

### 6.3 vs. initiative scope
(`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md`)

| Initiative requirement | Proposal | Gap? |
| --- | --- | --- |
| 06-export third in technical order (`:48-50`, `:75-89`) | A1 + A2 enforce sequencing | none |
| Builds canonical-transcript reader that 06-import-replace round-trips against (`:48-50`, `:83-89`) | §6 public types defined; §13 row 9 | none |
| Cross-feature anti-scope: no auto-resume / spawn / quota / config edits / `migrate-config` coupling (`:106-122`) | §7, §8, §13 | none |
| Out-of-scope items: cross-CLI migration, `.zst` ingestion, cross-org cache, frontend visibility, alternate formats (`:64-73`) | §7 + §11.1 | none |

No initiative scope drift.

## 7. Findings

### Severity ≥ MEDIUM

None.

### Severity LOW (drafting nits)

**L1 — §4 step 8 + §3 do not pin the canonical shape of the
emitted Claude compaction-boundary record.** §4 step 8 says "the
boundary summary record is included as the first emitted record" but
does not state which `role`, which `content` chunk variant(s), or
whether `unsupported_record` is true. The §3 schema requires `role`
to be `system|user|assistant|tool|unknown` and `unknown` only when
`unsupported_record: true`; without explicit pinning, two reasonable
implementations (assistant + text chunk vs. system + text chunk vs.
unsupported placeholder) are equally consistent with the proposal.
Recommend Phase 5 hookpoints pin this concretely; or that §4 step 8
add one sentence ("emit boundary as `role: assistant` with `text`
chunk built from the summary payload, `unsupported_record: false`")
to make D4 enforceable from §3 alone. **Severity: LOW (drafting).**

**L2 — §6 `RecordSource.storage_type: SessionStorageType` includes
`Other` even though §4 step 6 fails before any record is built.**
The public `SessionStorageType` enum from locate is
`{ClaudeCode, CodexSession, Other}`. Strictly, the typestate of an
emitted `RecordSource` cannot be `Other`. This is not a bug — `Other`
is structurally reachable but dynamically unreachable on the success
path — but the public type would be marginally more honest as a
parser-supported subset (e.g. `enum CanonicalStorageType {
ClaudeCode, CodexSession }`) or §6 should add a one-line invariant
note that `RecordSource.storage_type ∈ {claude_code, codex_session}`.
Either fix is a Phase 5/6 implementation decision; the contract risk
is that a future caller could over-trust the field. **Severity: LOW
(drafting / public-type ergonomics).**

**L3 — §12 bullet 4 in-memory residual phrasing is softer than §6
D7's commitment.** §6 D7 commits the public API to a `Vec` ("Later
internal helpers may stream source lines into parser state, but the
public API returns a fully validated transcript"); §12 bullet 4
notes "Very large transcripts pay memory cost proportional to
exported records" without naming a budget or a Phase 6 mitigation.
Not a scope expansion, but the residual would be more useful with
either (a) a stated tolerable size band ("transcripts up to N records
fit in memory budget B") or (b) an explicit deferral
("post-v1 streaming variant tracked under future-residuals/…"). As
written, the residual is observable but not actionable. **Severity:
LOW (drafting).**

None of L1–L3 raises a Phase 4 scope concern; all are drafting
clarifications appropriate for Rev 2 fold-in or Phase 5 hookpoints.

## 8. Drift audit (S5)

Cross-checked the proposal against §1–§13 for surface beyond the §1
scope statement. Sections walked:

- §1, §1.1, §1.2: scope, register, net-value — bounded.
- §2: subcommand surface — one enum variant, one format enum.
- §3: per-record schema — fields match harness ask + D1/D2 refinements.
- §4: 10-step resolution flow — every step traceable to harness ask
  or constraint.
- §5: exit codes — match harness ask + namespace constraint.
- §6: reader API — public types match §3 schema; D7 returns `Vec`.
- §7: anti-scope — exhaustive, no internal contradictions.
- §8: side-effect contract — strictly read-only with explicit
  STATE_DIR Phase 5 escalation.
- §9: test-intent track — covers all six harness acceptance criteria
  plus D1–D5 decisions; no extra-scope test categories.
- §10: README updates — strictly additive, scoped to CLI sections.
- §11: supported-surface track — local CLI binary only.
- §12: residuals — seven items, all with invalidator citations or
  bounded scope.
- §13: cross-feature compliance — all ten rows hold.

No surface found outside §1's stated scope. No silent edit to
existing files beyond the additive `SessionSubcommands` enum
extension and a new top-level module declaration in `src-tauri/src/lib.rs`.
No anti-scope violation. No constraint row breakage.
