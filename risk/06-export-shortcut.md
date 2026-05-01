# 06-export — Phase 4 Shortcut Risk Assessment (Rev 1)

## Verdict: LOW

The Rev 1 proposal does not contain shortcuts that defeat the
underlying purpose of `agents session export`. The five purpose
threads — (a) replace harness-side direct provider parsing with one
canonical CLI surface; (b) auditable per-record source preimage
metadata (line/byte/sha); (c) reusable canonical reader for
06-import-replace round-trip; (d) reuse of 06-locate ownership/path;
(e) read-only side-effect contract — are each carried by a
non-trivially-costed design choice, not by a corner-cut. Every
identified tradeoff (compaction asymmetry, timestamp-regression fail-
closed, in-memory `Vec` buffering, deferred read-only `StateDb`
dependency, deferred locator side-effect resolution) is documented
as a residual with a concrete falsifiable invalidator and a Phase 5
or Phase 6 disposition rule. No purpose thread is silently degraded.

No `MEDIUM` or `HIGH` shortcut findings. Four sub-LOW watchpoints
documented below for Phase 5 / Phase 6 awareness.

## Purpose-thread shortcut analysis

| Purpose thread | Could a shortcut here defeat purpose? | Proposal's choice | Verdict |
| --- | --- | --- | --- |
| (a) Stable canonical surface that replaces harness direct parsing | A canonical schema that flattens tool calls into prose, or that drops `unsupported_record`, would force the harness to keep parsing private formats. | D2 keeps text/tool_call/tool_result as typed chunks (`proposals/06-export.md:118-127`); `unsupported_record` is mandatory on every record (`:115-116`); `canonical-jsonl` is the only v1 format with a clap-typed enum so format drift is fail-closed (`:88-94`). Purpose-fit, not a shortcut. | LOW |
| (b) Auditable source preimage on every record | A "best-effort" or "sometimes-omit" source object would defeat preimage audit — the harness is the consumer of these fields. | D1 makes all six source fields mandatory on every emitted record (`:146-156`); SHA-256 is over the exact native record byte slice excluding terminator, with explicit CRLF/LF rules (`:158-162`); the byte-preserving JSONL scanner is named as a non-trivial implementation cost (`:164-168`). Purpose-fit, not a shortcut. | LOW |
| (c) Reusable canonical reader for import-replace round-trip | Returning a streaming iterator would have been cheaper but would defeat both the no-partial-stdout invariant *and* the round-trip-from-buffer reuse pattern import-replace expects. | D7 returns `Vec<CanonicalRecord>` and §6 exposes the public types `CanonicalRecord` / `ContentChunk` / `RecordSource` / `ExportError` from `src-tauri/src/session_export/` (`:236-300`). Public types are explicit so import-replace can parse replacement input and compare post-replace export output (`:298-300`). The memory cost is documented as residual (`:428-429`). Purpose-fit. | LOW |
| (d) Reuse of 06-locate ownership/path | Re-implementing ownership inside export, or bypassing `resolve_resume`, would create a second ownership path. | §4 step 5 calls `locate_session_metadata`, inheriting ownership/ambiguity/storage vocabulary/canonicalization/workspace-root validation from locate (`:181-187`); §13 cross-feature constraints row confirms reuse (`:441-442`). No second ownership path exists. Purpose-fit. | LOW |
| (e) Read-only side-effect contract | A "we'll use the existing mutating `StateDb::open_default`" or "we'll call `locate_transcript` and accept its STATE_DIR mkdir" would silently violate the harness's strict no-side-effect rule (`02-session-export.md:54-64`). | §4 step 3 explicitly forbids today's mutating `StateDb::open_default` and depends on 06-schema-probe's read-only variant (`:177-180`). §8 names the `STATE_DIR`-mkdir residue inside `locate_transcript` as a Phase 5 must-resolve-or-revise (`:336-339`). Phase 5 either supplies a read-only locator path or this proposal is revised — fail-closed deferral, not silent acceptance. | LOW |

## Specific shortcut surfaces evaluated

### Sh-1 D5 ordering: file order with regression-as-error

§4 step 9 emits records in JSONL file order after the compaction
cutoff and exits `15` on timestamp regression rather than re-sorting
(`proposals/06-export.md:203-208`). §12 residual names "Real
transcripts with benign clock skew would be rejected" (`:427`). The
alternative (timestamp-sort) would silently re-order records that
the provider wrote in a specific causal order — that is the larger
shortcut against the canonical-replay purpose. Refuse-rather-than-
corrupt is the correct shortcut-avoidant choice for v1. Watchpoint
Sh-W2 below covers Phase 6 fixture coverage.

### Sh-2 D4 compaction: Claude live, Codex full

§4 step 8 emits live canonical transcript starting at the latest
supported boundary for Claude (`isCompactSummary == true`) and emits
the full transcript for Codex because no stable raw marker is
currently known (`:198-202`). A7 names this asymmetry with a
falsifiable invalidator ("Codex compaction must be live-state
accurate in v1, or Claude changes compaction marker shape" — `:48`)
and §12 residual reinforces (`:424-425`). The harness spec
(`02-session-export.md:56-64`) does not require live-state
compaction for either provider; export's choice to apply it where a
marker exists *exceeds* the harness ask, not falls short of it. Not
a shortcut. Watchpoint Sh-W1 below covers the Codex marker hunt.

### Sh-3 In-memory `Vec<CanonicalRecord>` buffer

§4 step 10 builds the complete `Vec` and validates every record /
every source hash before any stdout write (`:209-211`). §3
reinforces the no-partial-stdout invariant at the CLI seam
(`:99-101`). §12 names the proportional-memory residual (`:428-429`).
The harness spec line 64 explicitly forbids partial transcript on
error; in-memory buffering is the *minimum* mechanism that achieves
that invariant for a non-streaming validator. A streaming
implementation would either defeat the invariant or require a
staging temp file (which §7 anti-scope forbids). Purpose-fit. Sh-W4
below carries the Phase 6 memory ceiling as a watchpoint.

### Sh-4 `SessionStorageType::Other` rejected with exit `12`

§4 step 6 fail-closes on `Other` storage (`:188-189`). The
alternative — a generic line-by-line emitter over unknown JSONL —
would defeat purpose (a) by handing the harness records that the
agent-runner side has not actually parsed into canonical content.
Better to refuse than to launder unparsed bytes through a
"canonical" envelope. Purpose-fit.

### Sh-5 Public reader API surface (D7)

§6 exposes `read_canonical_transcript(metadata: &SessionMetadata) ->
Result<Vec<CanonicalRecord>, ExportError>` plus the canonical
record/content/source/error types directly from
`src-tauri/src/session_export/` (`:288-300`). This is the
load-bearing public surface for 06-import-replace's round-trip
reader. Returning a `Vec` instead of an iterator is a deliberate
cost paid for the no-partial-stdout invariant and for round-trip
buffering at the import side. Documented in `proposals/06-export.md:296-300`.
Purpose-fit.

### Sh-6 Sha-2 direct dependency (A8)

A8 carries the cost of adding `sha2` as a direct dep instead of
hand-rolling SHA-256 (`:49`). Hand-rolling would be the shortcut;
declaring the dep is the right call. §12 names the dep policy
escape hatch (`:430-431`). Not a shortcut.

### Sh-7 No `session_turns` content/ordering/source fallback

§4 final paragraph and §13 reinforce that `session_turns` is not
used for content, source metadata, ordering, or compaction cutoff
(`:213-216`, `:441-442`). The shortcut would have been "reconstruct
content from `session_turns` rows" — that is impossible because
`session_turns` stores no content (problem map §1 #23, db.rs:559-572).
Anti-scope §7 forbids it explicitly (`:312-313`). Purpose-fit refusal,
not a shortcut.

### Sh-8 Provider-native bookkeeping skipped

§3 skips thinking/reasoning/event/session-metadata/usage records
when they do not represent transcript turns (`:130-134`). The
harness spec line 78 explicitly disclaims byte-for-byte provider-
native output and asks for canonical transcript JSONL — this skip
is what *makes* it canonical. The five-condition gate for
`unsupported_record` emission (`:135-141`) prevents the alternate
shortcut (silently dropping ambiguously-bookkeeping records) by
requiring the parser to prove safe placeholder emission or fail
closed at exit `15`. Both directions covered. Purpose-fit.

## Watchpoint signals (sub-LOW; Phase 5 / Phase 6 awareness)

### Sh-W1 Codex compaction marker hunt

§4 step 8 and A7 both name a Phase 5 hunt for a stable Codex
compaction marker (`:200-202`, `:48`). If Phase 5 finds one and the
proposal is not revised, Codex export silently misses it. If Phase
5 does *not* find one, the v1 asymmetry persists as documented. Phase
5 must record the result either way. The asymmetry itself is not a
shortcut today, but Phase 5 silence on the question would be one.

### Sh-W2 Real-transcript timestamp regression coverage

§9 names a "regressing timestamps" component-level fixture (`:357`),
but Phase 5/6 fixtures should sample real Claude and Codex
transcripts — including sidechain branches and any retry paths —
to confirm they do not regress in practice. If real transcripts
*do* regress, D5's fail-closed behavior (`:206-208`) blocks
legitimate exports and the proposal needs revision toward a
boundary-aware ordering policy. The Phase 6 implementer should
treat unexpected regressions in real fixtures as a Phase 2.5 re-
entry signal, not a fixture-skipping signal.

### Sh-W3 `locate_transcript` STATE_DIR mkdir residue

§8 acknowledges that the current `locate_transcript` helper creates
`STATE_DIR` (`:336-339`, problem map §2 #5). Export's side-effect
contract is stricter than locate's; 06-locate Rev 3 accepted this
mkdir as carried-residual (R1-F03 closure), but locate's contract
permits the side effect. Export's does not. Phase 5 must either
identify a read-only locator path, lift the helper into a read-
only mode, or revise §8. If Phase 5 does not resolve this, export
silently violates the harness's no-temp-files / no-state rule. The
proposal correctly defers without silently accepting; the watchpoint
is the Phase 5 evidence requirement.

### Sh-W4 Memory ceiling for very large transcripts

§12 names proportional-memory as a residual (`:428-429`). Phase 6
fixtures should include at least one large-transcript case (order-
of-magnitude estimate from the largest real Claude/Codex transcripts
the implementer can sample) to confirm the in-memory buffer fits a
realistic upper bound. If a real session OOMs the export binary
under default platform limits, the no-partial-stdout invariant and
the single-pass `Vec` cost would need to be reconciled (likely via
a staging temp file with atomic-rename emission, which would re-open
§7 anti-scope and require Phase 2.5 re-entry). Treat this as a
Phase 6 fixture-coverage gate, not a Phase 4 blocker.

## Findings (severity >= MEDIUM)

None.

## LOW-severity observations / nits

None beyond the four sub-LOW watchpoints above.

## What this report does not cover

Per Phase 4 role separation, this report only evaluates whether
proposed shortcuts defeat the underlying purpose. It does not
evaluate audit completeness (`risk/06-export-audit.md`), scope
adherence (`risk/06-export-scope.md`), or net-value on the supported
surface (`risk/06-export-supported-surface.md`).
