# Audit history — 06-locate (`agents session locate`)

## Purpose

Track the multi-round revise/review loop for Phase 3 / Phase 4 of
feature 06-locate under Initiative 06 (Session Override Contract).
Round 1 returned HIGH on the audit gate; Round 2 cleared all four
gates at LOW with no oscillation. Phase 4 closed; Phase 5 next.

## Artifact lineage

- Initiative: `initiatives/06-session-override-contract.md`
- Phase 2.5 problem map: `research/06-locate-problem-map.md`
- Proposal under audit: `proposals/06-locate.md` (Rev 1 → Rev 2)
- Risk reports (overwritten per round):
  - `risk/06-locate-audit.md`
  - `risk/06-locate-scope.md`
  - `risk/06-locate-shortcut.md`
  - `risk/06-locate-supported-surface.md`

## Round summaries

### Round 1 — `proposals/06-locate.md` (Rev 1) reviewed

- Artifact under review: `proposals/06-locate.md` (Rev 1)
- Round artifacts:
  - `proposals/06-locate.md` (Rev 1, 301 lines)
  - `risk/06-locate-audit.md` (Rev 1)
  - `risk/06-locate-scope.md` (Rev 1)
  - `risk/06-locate-shortcut.md` (Rev 1)
  - `risk/06-locate-supported-surface.md` (Rev 1)
- Report artifacts:
  - report index: none
  - PDFs: none
  - uploaded artifact URL: none
  - screenshots: none
  - non-UI evidence: none
- Prior finding counters: n/a (first round)
- New findings:
  - **`R1-F01`** — blocking (HIGH); `risk/06-locate-audit.md` B1: §9.1 missing the required test-intent row for D5 (no `--state-db` override / GUI DB scope); ancestor chain: none; oscillation: none.
  - **`R1-F02`** — blocking (HIGH); `risk/06-locate-audit.md` F4: §4 step 8 / §9.1 D7 cite Codex `session_meta.payload.cwd`/`payload.workspace_root`; cited script reads only `payload.id`. Speculative against cited source. Echoed by `risk/06-locate-supported-surface.md` F1 as advisory; ancestor chain: none; oscillation: none.
  - **`R1-F03`** — non-blocking (MEDIUM); `risk/06-locate-audit.md` E2: §4 step 7 calls `locate_transcript` which performs `STATE_DIR` mkdir; §8 side-effect contract does not classify; supported-surface confirms it matches `trace --json`'s existing I/O; ancestor chain: none; oscillation: none.
  - **`R1-F04`** — non-blocking (MEDIUM); `risk/06-locate-audit.md` F2: §4 step 3 cites resume-adjacent config loading as precedent for malformed-config-as-operational-error, but resume actually uses `unwrap_or_default` and silently degrades; ancestor chain: none; oscillation: none.
  - **`R1-F05`** — non-blocking (advisory); `risk/06-locate-supported-surface.md` F2: Claude project-hash inversion has no defined tiebreaker for ambiguous decompositions (paths with `-` in components); ancestor chain: none; oscillation: none.
  - **`R1-F06`** — non-blocking (advisory); `risk/06-locate-supported-surface.md` F3: §11.1's "users can run `agents migrate-db`" overpromises — `backfill_session_chains` skips when any chain row exists; ancestor chain: none; oscillation: none.
  - **`R1-F07`** — non-blocking (cosmetic); `risk/06-locate-scope.md` #3.A: §12 residuals do not record that `mutable: false` will gain a sixth condition once 06-pause-handshake lands; ancestor chain: none; oscillation: none.
  - **`R1-F08`** — non-blocking (cosmetic); `risk/06-locate-scope.md` #3.C: module path written as "proposed `src-tauri/src/session_metadata/`"; should be committed; ancestor chain: none; oscillation: none.
  - **`R1-F09`** — non-blocking (cosmetic); `risk/06-locate-shortcut.md` L2: §10 README framing of `mutable` as eligibility (not a write lock) is implicit; should be explicit; ancestor chain: none; oscillation: none.
- Oscillation:
  - same-label: 0
  - same-family: 0
  - fix-created: 0
  - two-generation: 0
  - named three-generation: 0
- Decompose trigger: not fired; reason: first round, no prior generation to recur from; findings are surgical/contract-shape, not structural.
- Watch signals for next round:
  - **WS1**: D7 Codex workspace-root derivation. R1-F02 must close — Phase 5 hookpoints will need to verify against real Codex rollout schema OR proposal commits to fail-closed for all Codex sessions.
  - **WS2**: side-effect contract completeness (§8). R1-F03 closure must classify the `STATE_DIR` mkdir; future siblings (export, import-replace, schema-probe) must inherit a consistent classification.
  - **WS3**: assumption register rephrasing discipline. A4 "rephrased" status must propagate consistently between proposal §1.1, supported-surface report, and audit history if it lands in Rev 2.
- Verdict or determination: **continue** (revise proposal as Rev 2 per pipeline rule "any MEDIUM or HIGH report means revise the proposal and re-run all four")
- Role outputs:
  - audit (`gpt-high`): HIGH; 2 HIGH findings, 2 MEDIUM, 1 LOW; `risk/06-locate-audit.md`
  - scope (`claude-opus`): LOW; 0 ≥MEDIUM, 4 nits; `risk/06-locate-scope.md`
  - shortcut (`claude-opus`): LOW; 0 ≥MEDIUM, 2 LOW observations; `risk/06-locate-shortcut.md`
  - supported-surface (`claude-opus`): LOW; termination `none`; A1–A9 all HOLD (A4 rephrased); `risk/06-locate-supported-surface.md`
- Next handoff: Rev 2 proposal-revision agent (`gpt-high`) reads:
  1. all four Round 1 risk reports above
  2. this audit-history file's Round 1 finding list
  3. the Rev 1 proposal at `proposals/06-locate.md`
  4. the problem map at `research/06-locate-problem-map.md`

  and emits Rev 2 of `proposals/06-locate.md` closing R1-F01..R1-F09. Rev 2 risk gates re-run all four roles in the same model assignments.

### Round 2 — `proposals/06-locate.md` (Rev 2) reviewed

- Artifact under review: `proposals/06-locate.md` (Rev 2)
- Round artifacts:
  - `proposals/06-locate.md` (Rev 2, 321 lines, +20 net from Rev 1)
  - `risk/06-locate-audit.md` (Rev 2)
  - `risk/06-locate-scope.md` (Rev 2)
  - `risk/06-locate-shortcut.md` (Rev 2)
  - `risk/06-locate-supported-surface.md` (Rev 2)
- Report artifacts: none
- Prior finding counters:
  - closed: 9 (R1-F01..R1-F09 all closed at original severities)
  - intact: 0
  - weakened: 0
  - regressed: 0
  - not closed: 0 (L1 carried as audit pin per Rev 1's framing — not a closure failure)
- New findings:
  - **`R2-F01`** — non-blocking (LOW); §4 step 8 path-hash tiebreaker prose appears to short-circuit ("pick the first") while §9.1 enforces "exactly one or exit 12"; shortcut + supported-surface both raised; ancestor chain: R1-F05 -> R2-F01 (fix-clarity, not regression); oscillation: same-family (path-hash rule clarified in Rev 2; prose still ambiguous in Rev 2 — but contract pinned by test obligation, so `apply` not `decompose`).
  - **`R2-F02`** — non-blocking (cosmetic); shortcut: `unwrap_or_default` config load (R1-F04 closure) silently degrades malformed `providers.toml` to "unsupported-storage"; inherited from resume parity; not introduced by locate; ancestor chain: none; oscillation: none.
- Oscillation:
  - same-label: 0
  - same-family: 1 (R2-F01 prose-clarity ancestor of R1-F05; bounded by §9.1 test obligation)
  - fix-created: 0
  - two-generation: 0
  - named three-generation: 0
- Decompose trigger: not fired; reason: same-family R2-F01 is bounded by §9.1 test pin (Phase 6 implementer driven by test, not prose); R2-F02 is inherited resume parity, not a locate-introduced concern.
- Watch signals for Phase 5/6:
  - **WS1** (forwarded): Phase 5 must sample real Codex rollout JSONL.
  - **WS4** (new): Phase 6 implementer treats §9.1 D7 ambiguity row as authoritative for path-hash rule.
  - **WS5** (new): Phase 6b README review may add a "unsupported-storage can also occur for malformed config" note; or a future cross-feature pass tightens both resume and locate together.
- Verdict or determination: **apply** (advance to Phase 5).
- Role outputs:
  - audit (`gpt-high`): LOW; 0 findings ≥MEDIUM; all R1 findings closed; `risk/06-locate-audit.md`
  - scope (`claude-opus`): LOW; 0 findings ≥MEDIUM; net Rev 2 direction = reduction; `risk/06-locate-scope.md`
  - shortcut (`claude-opus`): LOW; 2 LOW observations (R2-F01, R2-F02); `risk/06-locate-shortcut.md`
  - supported-surface (`claude-opus`): LOW; termination `none`; A1–A9 HOLD; `risk/06-locate-supported-surface.md`
- Next handoff: Phase 5 hookpoint research agent (`gpt-high`) reads:
  1. `proposals/06-locate.md` (Rev 2) §6 reusable API, §4 resolution flow, §13 cross-feature checklist
  2. `research/06-locate-problem-map.md`
  3. WS1 (forwarded — sample real Codex rollout JSONL)
  4. existing source for `StateDb::resolve_resume`, `locate_transcript`, `[providers.session_storage]`, `compose_resume_args`, trace transcript-state code

  and emits `research/06-locate-hookpoints.md` mapping every Rev 2 proposal action to file:line code sites with reuse, deletion, and conflict notes. Phase 5 has a human gate.

### Round 3 — `proposals/06-locate.md` (Rev 3) reviewed

- Artifact under review: `proposals/06-locate.md` (Rev 3, 331 lines)
- Round artifacts:
  - `proposals/06-locate.md` (Rev 3)
  - `risk/06-locate-audit.md` (Rev 3)
  - `risk/06-locate-scope.md` (Rev 3)
  - `risk/06-locate-shortcut.md` (Rev 3)
  - `risk/06-locate-supported-surface.md` (Rev 3)
  - `research/06-locate-hookpoints.md` (Phase 5; consumed)
  - `research/06-locate-problem-map.md` (§7 A4 updated)
- Report artifacts: none
- Prior finding counters:
  - closed: 11 (R1-F01..R1-F09 still standing; R2-F01 closed by Rev 3 prose tightening; R2-F02 unchanged inherited)
  - intact: 1 (R2-F02 carried as accepted residual)
  - weakened: 0
  - regressed: 0
  - not closed: 0
- New findings:
  - **`R3-F01`** — non-blocking (cosmetic); shortcut + supported-surface: §4 step 8 Codex branch does not specify behavior for multiple `session_meta` records in one rollout JSONL, nor the line-record-type discriminator (e.g., `type == "session_meta"`); Phase 5's 25-file sample saw one per file; Phase 6 implementer follows existing `scripts/codex-locate-transcript` line-walk precedent; `§9.1 D7 row pins behavior via tests; ancestor chain: none; oscillation: none.
- Oscillation:
  - same-label: 0
  - same-family: 0
  - fix-created: 0
  - two-generation: 0
  - named three-generation: 0
- Decompose trigger: not fired; reason: R3-F01 is bounded by the existing locator-script precedent and §9.1 D7 test obligations; no MEDIUM+ surface.
- Watch signals for Phase 6:
  - **WS1** (closed for v1): A4 invalidator names "upstream Codex schema drift" as future trigger.
  - **WS4** (closed by Rev 3): path-hash prose tightening resolves R2-F01.
  - **WS5** (still active): resume parity malformed-config — Phase 6b README or future cross-feature pass.
  - **WS6 (new)**: Phase 6 Codex parser follows `scripts/codex-locate-transcript` for `type == "session_meta"` discrimination; first-match for multi-record edge.
- Verdict or determination: **apply** (advance to Phase 6).
- Role outputs:
  - audit (`gpt-high`): LOW; 0 findings; all R1/R2 closures standing; `risk/06-locate-audit.md` (Rev 3)
  - scope (`claude-opus`): LOW; 0 findings; net direction = controlled expansion (one reduction action D, one clarification action E); no drift beyond actions A-F; `risk/06-locate-scope.md` (Rev 3)
  - shortcut (`claude-opus`): LOW; 0 findings ≥MEDIUM; 1 sub-LOW R3-F01 (multi-record edge); `risk/06-locate-shortcut.md` (Rev 3)
  - supported-surface (`claude-opus`): LOW; termination `none`; A1-A9 HOLD (A4 in Rev 3 form); §6 #8 retired for both providers; harness Codex coverage flips from partial-by-design to covered; `risk/06-locate-supported-surface.md` (Rev 3)
- Next handoff: Phase 6 implementation.
  - **6a Contract**: orchestrator-owned. Define schemas, signatures, fixture-application points, test-intent handoff. Consume proposal §6 SessionMetadata API + §3 JSON schema + §5 exit codes + §9.1 test-intent track.
  - **6b Tests-first**: separate agent invocation. Test writer must NOT see Step 6c implementation. Encodes intended behavior from contract + proposal test-intent track + hookpoints.
  - **6c Code**: separate agent invocation. Reads tests + contract + Step 6b output index. Makes tests pass while respecting approved design.
  - Process-tree-auditor runs after 6c.

## Role histories

### Writer

#### Round 1
- Input read: harness spec, problem map, initiative file, prior-art proposal, pipeline doctrine
- Role decision: emitted Rev 1 (n/a determination — first round)
- Reason: Phase 3 first-pass synthesis
- Self-oscillation signal: none (first round)
- Next role-local watch: address every R1-F0N in Rev 2; do not introduce new design surface

#### Round 2
- Input read: all four Rev 1 risk reports, audit history Round 1, Rev 1 proposal, problem map, initiative
- Role decision: emitted Rev 2 (apply)
- Reason: closed all nine R1 findings at original severities; +20 net lines; no fresh design surface; A4 rephrased; Codex deferred fail-closed
- Self-oscillation signal: none — Rev 2 changes block traces 1:1 to R1-F0N
- Next role-local watch: Phase 5 hookpoint research carries WS1 (Codex schema sampling); R2-F01 (path-hash prose) is a candidate one-line tightening if a future pass touches §4 step 8

#### Round 3
- Input read: Phase 5 hookpoint research, all Rev 2 risk reports, audit history Rounds 1/2, Rev 2 proposal, problem map (§7 A4 updated), initiative
- Role decision: emitted Rev 3 (apply)
- Reason: closed R2-F01 (path-hash prose); folded Codex `payload.cwd` derivation into v1 cleanly; A4 evidence cites Phase 5 sample; new invalidator forward-looking
- Self-oscillation signal: none — Rev 3 changes block traces 1:1 to the six authorized actions A-F; no drift detected by scope gate
- Next role-local watch: Phase 6 implementation. WS6 (multi-record `session_meta` edge) is a Phase 6 implementer note, not a writer concern

### Reviewer

#### Round 1
- audit (`gpt-high`):
  - Input read: Rev 1 proposal, problem map, initiative, pipeline doctrine, Rev 1 spot-check of source
  - Role decision: HIGH
  - Reason: B1 (missing D5 test row) + F4 (Codex `payload.cwd` speculative) blocking; E2 + F2 MEDIUM
  - Self-oscillation signal: none (first round)
  - Next role-local watch: WS1, WS2 above; verify Rev 2 closes both HIGH findings without introducing new contracts
- scope (`claude-opus`):
  - Role decision: LOW
  - Reason: every D-decision tracks the harness contract; one borderline (TranscriptState extraction) is gated
  - Self-oscillation signal: none
  - Next role-local watch: nits #3.A and #3.C closure
- shortcut (`claude-opus`):
  - Role decision: LOW
  - Reason: no D-decision dodges purpose; no shim; no deferred stub
  - Self-oscillation signal: none
  - Next role-local watch: L2 README framing
- supported-surface (`claude-opus`):
  - Role decision: LOW; termination `none`
  - Reason: A1–A9 all HOLD (A4 rephrased to fail-closed); 11/11 problem-map §6 entries retired
  - Self-oscillation signal: none
  - Next role-local watch: WS1; verify Rev 2's A4 rephrasing is reflected explicitly in §1.1

#### Round 2
- audit (`gpt-high`):
  - Input read: audit history Round 1, Rev 1 audit report, Rev 2 proposal, problem map, initiative, harness spec
  - Role decision: LOW
  - Reason: all nine R1 findings closed at original severities; Rev 2 changes block truthful; A4 rephrasing falsifiable; D5 test row complete; Codex fail-closed branch closes path against Phase-6 misimplementation; STATE_DIR mkdir clause restrictive enough; migration-path overpromise removed; A-K checklist re-walked clean
  - Self-oscillation signal: none — no R1 finding regressed; no new MEDIUM+ surface
  - Next role-local watch: WS1 forwarded to Phase 5
- scope (`claude-opus`):
  - Role decision: LOW
  - Reason: net direction = reduction (Codex deferral + migrate-db narrowing); seven other Rev 2 changes are clarifications/doc-only/test-only; no Rev 2 change extends past the R1 finding it closes; cross-feature constraints unchanged
  - Self-oscillation signal: none — no Rev 1 nit regressed
  - Next role-local watch: none — all Rev 1 nits closed (#3.D was n/a)
- shortcut (`claude-opus`):
  - Role decision: LOW
  - Reason: W1 Codex deferral = purpose-fit hand-off (typed UnsupportedStorage error, concrete Phase 5 trigger, no deferred stub by convention's definition); W2 path-hash tiebreaker = purpose-fit deterministic; W3 unwrap_or_default = purpose-fit citation fix matching resume parity; W4 mutable forward-extension = purpose-fit (residual + §10 README + §7 anti-scope + §13 checklist coordinated)
  - Self-oscillation signal: same-family R2-F01 path-hash prose ambiguity (ancestor R1-F05) — bounded by §9.1 test pin, not promoted to MEDIUM
  - Next role-local watch: WS4 (Phase 6 implementer treats §9.1 as authoritative)
- supported-surface (`claude-opus`):
  - Role decision: LOW; termination `none`
  - Reason: A1–A9 still HOLD (A4 keeps Rev 1 rephrasing; Rev 2 makes Codex side explicit); termination signals do not fire; problem-map §6 #1–11 still retired (#8 retired-for-Claude / informationally-equivalent for Codex); blast radius unchanged from Rev 1 except F3 (`migrate-db` claim) now resolved
  - Self-oscillation signal: none
  - Next role-local watch: WS1 — Phase 5 hookpoint research carries the Codex schema-sampling obligation

#### Round 3
- audit (`gpt-high`):
  - Input read: audit history Rounds 1/2 + Round 3 setup, Rev 2 audit report, Rev 3 proposal, problem map (with §7 A4 updated), Phase 5 hookpoint research, initiative, harness spec, `scripts/codex-locate-transcript`
  - Role decision: LOW
  - Reason: all R1/R2 closures intact; R2-F01 closed by Rev 3 prose tightening; A4 Rev 3 invalidator forward-looking and falsifiable; §4 step 8 Codex derivation spec complete (line-walk + canonicalize + exists + UTF-8 + fail-closed for missing/absent/invalid); spot-check of `scripts/codex-locate-transcript` confirms line-walk precedent; Rev 3 changes block truthful; no fresh R3 finding from audit
  - Self-oscillation signal: none
  - Next role-local watch: Phase 6 implementer treats existing `scripts/codex-locate-transcript` as the line-walk precedent for the new `payload.cwd` parser
- scope (`claude-opus`):
  - Role decision: LOW
  - Reason: Rev 3 net direction = controlled expansion (action B) + reduction (action D) + clarification (action E); expansion is exactly what A4 invalidator authorized; drift audit found no edits outside the six authorized actions A-F; §13 cross-feature checklist unchanged; §7 anti-scope unchanged; §11.1 supported-surface track left untouched (correctly — its provider-agnostic framing already covered Codex)
  - Self-oscillation signal: none — no R1/R2 nit regressed
  - Next role-local watch: none for scope; Phase 6 implementation does not raise scope concerns
- shortcut (`claude-opus`):
  - Role decision: LOW
  - Reason: Codex `payload.cwd` derivation is purpose-fit (two Codex versions sampled, falsifiable forward-looking invalidator, fail-closed wrapper); R2-F01 path-hash tightening complete (no §4 step 8 sentence still implies short-circuit on path candidates); Codex line-walk first-match parallels existing locator pattern; R1 closures all standing; R1-F09 README mutable framing intact (load-bearing for Codex sessions now potentially returning `mutable: true`)
  - Self-oscillation signal: none
  - Next role-local watch: WS6 (Phase 6 implementer awareness for multi-record `session_meta` edge) recorded as R3-F01
- supported-surface (`claude-opus`):
  - Role decision: LOW; termination `none`
  - Reason: A1-A9 HOLD (A4 Rev 3 form retains fail-closed shape); net value strictly increased over Rev 2 (#8 now retired for BOTH providers); harness "Codex storage" coverage flips from partial-by-design to covered; adjacent path blast radius unchanged; migration / rollback / observability claims unchanged; §11.1 provider-agnostic framing remains accurate; `SessionMetadata` field set unchanged (forward-compat preserved)
  - Self-oscillation signal: none
  - Next role-local watch: future Codex schema drift fires the new A4 invalidator clause; locate's fail-closed shape preserves stable refusal behavior

## Decision register

| Round | Decision | Deciding inputs | Reason | Dissent | Next action |
| --- | --- | --- | --- | --- | --- |
| 1 | continue | audit HIGH; pipeline rule "any MEDIUM or HIGH report means revise the proposal and re-run all four" | Audit's two HIGH findings (B1, F4) are surgical and closable without redesign; supported-surface, scope, and shortcut all LOW | none | dispatch Rev 2 proposal-revision agent on `worktrees/06-locate`; re-run all four risk gates against Rev 2 |
| 2 | apply | audit LOW, scope LOW, shortcut LOW, supported-surface LOW (termination none); pipeline rule "all four reports must return LOW" | All nine R1 findings closed at original severities; no R1 finding regressed; no oscillation classified; two R2 LOW nits accepted as residual (path-hash prose pinned by §9.1; resume-parity malformed-config inherited limitation) | none | advance to Phase 5 (hookpoint research) |
| 3 | continue | Phase 5 sampled real Codex rollout JSONL and found `session_meta.payload.cwd` present in every sampled file (25 files, Codex 0.46.0 and 0.58.0); A4 invalidator's "Phase 5 proves a stable Codex workspace-root field" trigger fires; pipeline rule "if hookpoint research shows the approved problem map or assumption register is wrong, stop and return to research; resume at Phase 2.5 with an updated problem map before implementation continues" | Codex deferral was deliberately drafted as a Phase-5-conditional branch; Phase 5 evidence has now flipped that branch. Folding Codex into v1 via Rev 3 is the workflow's prescribed path, costs ~30 LOC of `payload.cwd` parsing in the proposal, and avoids a follow-up PR | none | update problem map §7 A4; dispatch Rev 3 proposal-revision agent on `worktrees/06-locate`; re-run all four risk gates against Rev 3 |
| 3 (post-gates) | apply | audit LOW, scope LOW, shortcut LOW, supported-surface LOW (termination none); pipeline rule "all four reports must return LOW" | Rev 3 closes R2-F01 by tightening §4 step 8 Claude prose; Codex `payload.cwd` derivation folded in cleanly with fail-closed wrapper and falsifiable forward-looking invalidator; A1-A9 all HOLD (A4 in Rev 3 form); §6 problem-map #8 retired for both providers; harness "Codex storage" coverage flips from partial-by-design to covered; no R1/R2 finding regressed; two R3 cosmetic findings (multi-record `session_meta` edge case; `type` discriminator unspecified) bounded by Phase 6 implementer following existing `scripts/codex-locate-transcript` precedent | none | advance to Phase 6 (implementation: 6a contract → 6b tests → 6c code) |

## User Q&A Inputs

None for Round 1.

## Watch signals

- **WS1**: D7 Codex workspace_root derivation — **forwarded to Phase 5**. Round 2 closed by deferring v1 Codex success entirely (fail-closed exit `12`). Phase 5 hookpoint research must sample real Codex rollout JSONL and decide whether `session_meta.payload.cwd` (or another stable field) is present and reliable; if so, a follow-up adds the derivation. A4 invalidator names this trigger.
- **WS2**: §8 side-effect contract completeness — **closed by R1-F03**. The `STATE_DIR` mkdir is explicitly classified; siblings (06-export, 06-import-replace, 06-schema-probe) inherit a consistent classification framework.
- **WS3**: assumption register rephrasing discipline for A4 — **closed**. Rev 2 A4 is consistent across §1.1, §4 step 8, §9.1 D7 row, §12 residuals, and supported-surface report.
- **WS4 (new)**: §4 step 8 path-hash tiebreaker prose ambiguity (R2-F01 from shortcut + supported-surface). §9.1 D7 ambiguity row pins the correct rule via test obligation. Phase 6 implementer treats §9.1 as authoritative when the §4 prose appears to short-circuit.
- **WS5 (new)**: resume-parity malformed-config / unsupported-storage indistinguishability (R2-F02 from shortcut). Inherited from resume's `unwrap_or_default`; not a 06-locate concern. Phase 6b README review or a future cross-feature pass may tighten both resume and locate together.

## Summarization tail

Two rounds; full detail retained for both per `~/ai/conventions/audit-history.md` summarization rules ("keeps the current round and two prior rounds in full"). No summarization needed.

## Final state

**Phase 4 closed (Round 3, Rev 3).** All four gates LOW; no termination signal; no R1/R2 regression. Codex `payload.cwd` derivation folded into v1 per Phase 5 evidence. Two cosmetic R3 findings (multi-record `session_meta` edge; `type` discriminator unspecified) bounded by existing locator pattern; Phase 6 implementer notes only.

Watch signals carrying into Phase 6:

- **WS1 (Codex schema)**: closed for v1; A4 invalidator names "upstream Codex schema drift" as the future falsification trigger.
- **WS4 (path-hash prose)**: closed; R2-F01 resolved by Rev 3 §4 step 8 prose tightening.
- **WS5 (resume parity malformed config)**: still active; not a 06-locate concern; Phase 6b README review or future cross-feature pass.
- **WS6 (new — `session_meta` discriminator)**: Phase 6 implementer follows `scripts/codex-locate-transcript` line-walk precedent for line-record-type discrimination; documents the choice in code.

Next phase: Phase 6 (implementation).
- 6a contract (orchestrator)
- 6b test writer (separate agent invocation; must not see implementation)
- 6c code writer (separate agent invocation; reads tests + contract; makes tests pass)
- Process-tree audit after 6c.
