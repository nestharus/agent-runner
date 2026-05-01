# 06-locate — Phase 4 Shortcut Risk Assessment (Rev 1)

## Verdict: LOW

The Rev 1 proposal does not dodge the harness's "stable, refuse-
rather-than-corrupt JSON" purpose with any D-decision. Each of
D1–D7 is purpose-fit on inspection: the chosen branches push the
command toward refuse-with-stable-error rather than emit-partial-
or-guess, and the rejected branches are turned down for documented
reasons (initiative constraints, no second ownership path, or
provenance unavailable in current schema). The §6 reusable API
declares typed errors that are concretely raised in §4 step
mappings, so it does not smell like a deferred-stub. The §6
`TranscriptState` move is conditioned on Phase 5 hookpoint
research, but the conditional is structured ("stop and revise
this proposal rather than duplicating a second transcript-state
type silently"), not a "we'll figure it out later." Anti-scope
items (§7 D4b/D5 rejections) are driven by the cross-feature
constraint forbidding a second ownership path, not by problem-
shifting. The internal-`codex` / external-`codex_session`
boundary at D2b is a public-vocabulary boundary, not a backwards-
compatibility shim — there is no old shape preserved alongside
a new one, and the convention's "transitional adapter" pattern
does not apply when the two vocabularies are by-design
asymmetric (internal config TOMLs vs. harness JSON contract).

Two LOW observations recorded below.

## Findings (severity >= MEDIUM)

None.

## LOW-severity observations / nits

**L1. D2b vocabulary boundary is purpose-fit but worth a one-
line audit pin.**
The proposal commits to `internal SessionStorage::Codex →
external "codex_session"` translation only at the locate boundary
(§3, §4 step 6). This is not a backwards-compat shim — `codex`
and `codex_session` were never the same shape, and §6's
`SessionStorageType::CodexSession` is the single public
vocabulary that 06-export and 06-import-replace will inherit
through `SessionMetadata`. The risk is downstream: if a future
sibling reads provider config directly (bypassing `SessionMetadata`)
and emits its own `storage_type`, the two vocabularies will
diverge. Mitigation already in proposal: §6 instructs siblings
to consume `SessionMetadata` rather than `ProviderConfig`. Leave
as a LOW audit nit; not a shortcut.

**L2. `mutable` excludes `exhausted_at` is purpose-fit, but the
proposal could be more explicit that pause-handshake is the only
hard lock.**
D3's choice to exclude `provider_quotas.exhausted_at` from the
`mutable` boolean is sound — quota is account-global, locate is
session-scoped, and §7 / §13 explicitly state "No attempt to make
`mutable` a hard import/replace safety lock; 06-pause-handshake
owns locks later." That is the correct purpose-fit: `mutable`
is a read-time eligibility hint, not a write guard. The harness
contract (`01-session-locate.md:31`) does not promise quota-
awareness either. The minor exposure is harness consumers who
read `mutable: true` and infer "safe to write" without waiting
for pause-handshake. The proposal documents this in §7 anti-
scope and §12 residuals; the README update in §10 should make
the read-only/eligibility framing explicit so harness authors
do not over-read the field. Phase 6b/README review concern,
not a shortcut.

## Per-question verdict

### Sh1 — D-decisions vs. harness purpose

- **D1a (mirror resolver ambiguity)** — Purpose-fit. The
  resolver's recency collapse already encodes the chain that
  agent-runner would resume; the harness inheriting the same
  collapse means `locate` and `resume` agree on which chain is
  "the" chain. Strict multi-row ambiguity (D1b) would have been a
  second ownership path in violation of `initiatives/06-session-
  override-contract.md:112-113`.
- **D2b (translate at boundary)** — See L1 above. Purpose-fit,
  not a shim.
- **D3 (`mutable` excludes `exhausted_at`)** — Purpose-fit. See
  L2 above. The proposal does not let the harness confuse
  read-time eligibility with a write lock.
- **D4a (no `session_turns` fallback)** — Purpose-fit. Falling
  back to `session_turns` outside the resolver would be a second
  ownership path (forbidden by initiative). Mapping segmentless
  rows to exit `10 session-not-found` is an honest refusal; the
  user can run `agents migrate-db` to backfill, which §11.1
  documents as the migration path.
- **D5 (no `--state-db` override / no GUI state DB)** — Purpose-
  fit for v1. The harness invokes the CLI binary, so it gets
  `open_default`. GUI state divergence is documented in §12 as a
  known residual. Not a shortcut.
- **D6 (fail-closed transcript_state)** — Purpose-fit. Returning
  exit `12` when transcript is `no_locator`/`missing` is exactly
  the "refuse rather than corrupt" behavior the harness asks for
  (`01-session-locate.md:35`). `trace --json` retains its
  graceful degradation for diagnostics; locate is for action.
  This split is not problem-shifting — it is the right surface
  separation.
- **D7 (JSONL-path-derived workspace_root)** — Purpose-fit. The
  proposal acknowledges in §12 that this can reject valid
  sessions whose provenance is not invertible from path/metadata.
  Refusing is preferable to guessing; A4's invalidator names the
  exact condition under which this rejection becomes wrong.

### Sh2 — Backwards-compatibility shims

The internal-`codex` / external-`codex_session` boundary is not
a back-compat shim. `~/ai/conventions/no-backwards-compatibility.md`
forbids "code that translates between old and new data shapes
purely for compatibility" — but here the two shapes were never
the same shape. `codex` is the internal serde tag; `codex_session`
is the new public harness vocabulary. There is no "old" external
vocabulary being preserved. §6's `SessionStorageType` enum is the
single public surface; siblings that consume `SessionMetadata`
inherit it. Audit pin recorded as L1 above for the divergence
risk if a sibling bypasses the API.

### Sh3 — Deferred stubs

§6's `MetadataError::{InvalidSessionId, SessionNotFound,
AmbiguousSession, UnsupportedStorage, Operational}` variants are
each concretely mapped to a producing condition in §4 (steps 1,
4, 6-8) and to an exit code in §5. None are declared without a
raise site. The CLI wrapper's responsibilities are also pinned
(§6 second paragraph after the function shape). Phase 6 cannot
land a half-stubbed `MetadataError::AmbiguousSession` because §9.1
"D1 ambiguity mirrors resolver" pins it as a component test that
asserts the variant fires only when the resolver returns
`Ambiguous`. Same for `SessionNotFound` (D4 row), `UnsupportedStorage`
(D2 + D6 + D7 rows), and `InvalidSessionId` (Invalid UUID row).

The lone gap is `MetadataError::Operational` — §9.1 has no
explicit row pinning it. That is an audit/test-audit concern
(operational errors are awkward to pin) and not a deferred-stub
violation; the variant is wired to a concrete failure class.

### Sh4 — `TranscriptState` extraction conditional

§6 line 178's conditional is structured deferral, not "we'll
figure it out later":
1. The intent is committed (move out of trace, share the enum).
2. The unacceptable fallback is named and forbidden ("rather
   than duplicating a second transcript-state type silently").
3. The trigger is concrete (Phase 5 hookpoint research showing
   trace behavior would materially change).
4. The escape hatch is "stop and revise this proposal" — i.e.,
   block forward progress, not paper over.

This shape — commit to the move, forbid the silent-duplicate
fallback, route uncertainty back to revision — is exactly what
a clean Phase 4→5 hand-off looks like. Not a shortcut.

### Sh5 — Anti-scope hidden shortcuts

- **D4b rejection (no `session_turns` fallback).** Is "we say
  session-not-found instead of handling partial DBs" problem-
  shifting? No — it is direct compliance with the initiative's
  no-second-ownership-path constraint
  (`initiatives/06-session-override-contract.md:112-113`). The
  alternative (a second ownership path that reads `session_turns`
  directly) is what the constraint exists to forbid. The user-
  facing recourse (`agents migrate-db`) is documented in §11.1.
- **D5 rejection (no `--state-db` override / no GUI state DB).**
  Same shape: scope-bounded to v1's harness consumer (CLI),
  with GUI state divergence documented as a known residual in
  §12. Not problem-shifting; the GUI path is its own surface
  with its own future work.

### Sh6 — Test-intent track shortcuts

The "README examples remain truthful" row's fallback to "Phase
6b index maps to manual doc review residual" is an honest
limitation, not a shortcut. README example tests are notoriously
hard; flagging the gap in the Phase 6b output index is the
right hand-off. The "Read-only behavior after open" row uses row-
count and mtime snapshots as a proxy — this is a strong proxy
(mutation would change the snapshot) and §12 names the physical
read-only residual that 06-schema-probe will close. Acceptable.

The D6 row notes it "Does not prove 600s timeout behavior except
existing `locate_transcript` tests" — that is a documented
residual (the 600s is `locate_transcript`'s contract, not
`locate`'s), not a shortcut.

## Conclusion

Verdict: **LOW**.

No D-decision substitutes the harness's literal contract for its
purpose. No backwards-compatibility shim. No deferred stub. The
one structurally-deferred item (TranscriptState extraction) is
gated on a named Phase 5 finding with the bad fallback (silent
duplicate) explicitly forbidden. Two LOW observations (L1 — D2b
vocabulary boundary discipline for siblings; L2 — README framing
of `mutable` as eligibility, not lock) are nits for audit/Phase
6b README review, not shortcut compromises.
