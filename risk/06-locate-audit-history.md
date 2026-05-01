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

### Round 3 — Phase 5 fired A4 invalidator; Rev 3 setup

- Phase 5 hookpoint research at `research/06-locate-hookpoints.md` §I.WS1: real Codex rollout JSONL exists under `/home/nes/.codex/sessions/` (5,739 files). Sample of 25 files across Codex 0.46.0 and 0.58.0 found `session_meta.payload.cwd` present in every sampled file. `payload.workspace_root` absent (but `payload.cwd` is the field needed for derivation).
- Pipeline rule (Phase 5): "if hookpoint research shows the approved problem map or assumption register is wrong, stop and return to research; resume at Phase 2.5 with an updated problem map before implementation continues."
- A4 invalidator literal: "Phase 5 proves a stable Codex workspace-root field and risk gates require folding it into v1 rather than a follow-up." Phase 5 has provided the empirical evidence; risk gates will decide whether folding is required.
- Workflow path: update problem map §7 A4 (light Phase 2.5 edit), dispatch Rev 3 proposal-revision agent to fold Codex `payload.cwd` derivation into v1, re-run all four risk gates against Rev 3.
- Round 3 verdict pending Round 3 risk-gate completion.

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

#### Round 3 (pending)
- Input to read: Phase 5 hookpoint research, Rev 2 proposal, problem map (with §7 A4 updated), initiative
- Role decision: pending — emit Rev 3
- Reason: Phase 5 fired A4 invalidator. Rev 3 must (a) drop the Codex deferral; (b) update §1.1 A4 to record Phase 5 finding and rephrase invalidator to "real Codex schema diverges from sampled `payload.cwd`"; (c) replace §4 step 8 Codex branch with `payload.cwd` derivation (parse JSONL, canonicalize, exists, UTF-8); (d) update §9.1 D7 row to cover Codex success path; (e) drop the Codex-deferral residual from §12; (f) add Rev 3 changes block.
- Self-oscillation watch: Rev 3 must NOT introduce design surface beyond the WS1 closure. The R2-F01 path-hash prose ambiguity is fair game for tightening if §4 step 8 prose is being edited anyway.

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

## Decision register

| Round | Decision | Deciding inputs | Reason | Dissent | Next action |
| --- | --- | --- | --- | --- | --- |
| 1 | continue | audit HIGH; pipeline rule "any MEDIUM or HIGH report means revise the proposal and re-run all four" | Audit's two HIGH findings (B1, F4) are surgical and closable without redesign; supported-surface, scope, and shortcut all LOW | none | dispatch Rev 2 proposal-revision agent on `worktrees/06-locate`; re-run all four risk gates against Rev 2 |
| 2 | apply | audit LOW, scope LOW, shortcut LOW, supported-surface LOW (termination none); pipeline rule "all four reports must return LOW" | All nine R1 findings closed at original severities; no R1 finding regressed; no oscillation classified; two R2 LOW nits accepted as residual (path-hash prose pinned by §9.1; resume-parity malformed-config inherited limitation) | none | advance to Phase 5 (hookpoint research) |
| 3 | continue | Phase 5 sampled real Codex rollout JSONL and found `session_meta.payload.cwd` present in every sampled file (25 files, Codex 0.46.0 and 0.58.0); A4 invalidator's "Phase 5 proves a stable Codex workspace-root field" trigger fires; pipeline rule "if hookpoint research shows the approved problem map or assumption register is wrong, stop and return to research; resume at Phase 2.5 with an updated problem map before implementation continues" | Codex deferral was deliberately drafted as a Phase-5-conditional branch; Phase 5 evidence has now flipped that branch. Folding Codex into v1 via Rev 3 is the workflow's prescribed path, costs ~30 LOC of `payload.cwd` parsing in the proposal, and avoids a follow-up PR | none | update problem map §7 A4; dispatch Rev 3 proposal-revision agent on `worktrees/06-locate`; re-run all four risk gates against Rev 3 |

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

**Phase 5 fired A4 invalidator.** Real Codex rollout JSONL sampling found `session_meta.payload.cwd` present in every sampled file (`/home/nes/.codex/sessions/`, 25 files across Codex 0.46.0 and 0.58.0). A4's invalidator condition met. Per `~/ai/workflows/implementation-pipeline.md` Phase 5 rule "if hookpoint research shows the assumption register is wrong, return to research; resume at Phase 2.5", advancing to Round 3:

- Problem map §7 A4 updated in place to record the empirical finding.
- Round 3 will revise the proposal as Rev 3 to fold Codex `payload.cwd` derivation into v1 (drop the deferral).
- Round 3 will re-run all four risk gates against Rev 3.

WS1 closure: empirical evidence found; Rev 3 incorporates it.

WS4, WS5: still active for Phase 6, unchanged by Phase 5 / Round 3.
