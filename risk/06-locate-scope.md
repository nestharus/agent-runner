# 06-locate — Phase 4 Scope Risk Assessment (Rev 1)

**Assessor:** `claude-opus` (scope)
**Verdict:** **LOW** — proposal stays within "one read-only CLI command + the
reusable `SessionMetadata` API the initiative explicitly assigned to 06-locate";
all seven D-decisions track the harness contract; anti-scope is complete.

The single borderline item is the `TranscriptState` move from `trace` into the
new module (§6 line 178), but it is gated by an explicit "stop and revise if
Phase 5 hookpoint research shows the move materially changes trace behavior"
condition and would otherwise force a duplicate enum — the gate is sufficient.
Net direction relative to the harness ask is neutral with two intentional
reductions (D5 no `--state-db`, D2 internal-enum unchanged) and one in-scope
extraction (`SessionMetadata` API at `initiatives/06-session-override-contract.md:41-43`).

---

## 1. Scope-direction analysis

| Question | Direction | Justification |
| --- | --- | --- |
| S1. New `session_metadata/` module | neutral | Initiative line 41-43 and 83-84 explicitly assigns "factor reusable `SessionMetadata` API" to 06-locate. Not creep. |
| S1. `TranscriptState` move out of `trace` | neutral (gated) | Alternative is a silently duplicated enum; §6 line 178 stops-and-revises if hookpoints show trace-behavior risk. The shared enum keeps existing serde snake-case (`src-tauri/src/trace/mod.rs:73-80`). |
| S2.D1 ambiguity = resolver `Ambiguous` | neutral | Harness ask routes through the same resolver as `agents resume`; D1a mirrors that. Strict multi-row would be a second ownership path, forbidden by `initiatives/06-session-override-contract.md:112-113`. |
| S2.D2 emit `codex_session`, keep internal `codex` | reduction | Avoids dragging config-file migration into this PR; output vocabulary matches harness §Output. |
| S2.D3 mutable composite, excludes quota | neutral | Harness asks for `mutable: true`; quota is provider-account global per `src-tauri/src/state/db.rs:455-463`. Excluding it keeps locate metadata orthogonal to routing policy. |
| S2.D4 segmentless turns → `session-not-found` | neutral | `session_turns` fallback would be a second ownership path; rejected for the same reason as D1b. |
| S2.D5 no `--state-db <path>` | reduction | Drops a public knob; GUI path divergence (`src-tauri/src/lib.rs:525-533`) is a known pre-existing issue not owned here. |
| S2.D6 four-state internal, `available`-only success | neutral | Harness explicitly asks for `unsupported-storage` over partial location (`01-session-locate.md:35`). |
| S2.D7 derive `workspace_root` from JSONL/path provenance | neutral | Harness requires the field but does not specify derivation; Phase 3 must commit, and D7b/c are evidence-rejected (invocations don't store `-p/--project`, `src-tauri/src/state/db.rs:205-233`). Residual logged at §12. |
| S3. Test-intent track | neutral | Twelve rows pin boundary behavior (resolver pass-through, mapping, partial-DB visibility, JSON shape, read-only contract). No row re-proves `resolve_resume` or `locate_transcript` internals. |
| S4. `mutable` deferral vs pause-handshake | neutral | §7 explicitly states "No attempt to make `mutable` a hard import/replace safety lock; 06-pause-handshake owns locks later." Future lock state is an additive `mutable: false` clause, not a contract collision. |
| S4. No physical read-only DB open | neutral | Initiative line 118-120 explicitly assigns the read-only variant to 06-schema-probe; logged at §12 residual. |
| S5. README updates | neutral | §10 stays at command synopsis + new "Locating a Session" section + clarifying notes against existing trace/SQL paragraphs. Paragraph-scale, not section rewrite. |
| S6. Anti-scope completeness | neutral | §7 covers all five harness anti-scope items and all five initiative anti-scope items; cross-checked at §13. |

## 2. Findings (severity >= MEDIUM)

None.

## 3. Nits (severity LOW)

### #3.A — `mutable: false` under future lock state not flagged in §12 residuals

§7 acknowledges that 06-pause-handshake will own locks; §13 marks lock
observation as "Not applicable to locate v1." Neither §12 nor the
checklist record the forward-extension contract that, once pause-handshake
lands, `mutable` is expected to become `false` while a lock is held even
when the §3 D3 conditions are otherwise met. This is not a scoping issue —
it is an additive future amendment — but recording it in §12 residuals
("`mutable` will gain a sixth condition once pause-handshake lands") would
prevent the next proposal from rediscovering it as a surprise. One-line
fix; non-blocking.

### #3.B — D2 `other` storage success path widens harness `storage_type` enum implicitly

The harness ask says `storage_type` distinguishes "at least `claude_code`,
`codex_session`, and `other`" (`01-session-locate.md:52`), so `other` is
within the harness contract. The proposal extends `other` to a
success-emitting state when a configured locator returns a canonical
absolute UTF-8 path AND `workspace_root` derives. That is a
non-obvious commitment — most `other` providers will lack invertible
workspace provenance and will get exit `12` per §3 footnote. The §12
residual at "`other` storage is success-capable only when..." records
this, so scope is bounded; the nit is purely about the schema field's
documentation in §10 making the `other`-success branch visible to README
readers. Non-blocking.

### #3.C — §6 module path proposed, not finalized

§1 line 16 reads "proposed as `src-tauri/src/session_metadata/`" and §6 line
146 reads "proposed path." Phase 3 normally commits a path; the soft
wording leaves two readers (Phase 5 hookpoints, Phase 6 implementation)
to re-decide. Not a scoping concern — the module's existence and shape
are committed; only the directory name is soft. One-line tightening.

### #3.D — Initiative file uses harness-numbering "1 → 5 → 2 → 4 → 3" while feature labels are 06-locate / 06-schema-probe / etc.

`initiatives/06-session-override-contract.md:80` renders sequencing
in harness-numbered form. The proposal correctly follows the
technical-dependency order in scope statement §1 line 9. No proposal
change needed; flagging only because future scope reviewers reading
the initiative may re-derive the same translation. Non-blocking; not
a 06-locate proposal issue.
