# 06-locate — Phase 4 Shortcut Risk Assessment (Rev 2)

## Verdict: LOW

The Rev 2 closure changes do not import a shortcut. Each
controversial change (Codex fail-closed, longest-prefix-existing
tiebreaker, `unwrap_or_default` config load, `mutable` future-
extension residual) is a purpose-fit narrowing of contract surface
that keeps "refuse rather than corrupt" intact. None matches the
deferred-stub shape (`~/ai/conventions/no-deferred-stubs.md`):
there are no functions declared without raise sites, no silent
`None`/`{}` placeholders, no unreferenced TODOs. The Rev 1 LOW
observations close cleanly: L2 is now explicit in §10, and L1
remains the audit pin it was always framed as. Two new LOW
observations recorded (R2-F01 prose ambiguity in §4 step 8;
R2-F02 malformed-config / unsupported-storage indistinguishability).

## R1 closure

| L# | Status | Evidence |
| --- | --- | --- |
| L1 | not closed (intentional) | Rev 2 leaves §6's "siblings should consume `SessionMetadata`" recommendation unchanged (line 204). L1 was framed in Rev 1 as "leave as a LOW audit nit; not a shortcut" — closure was not required this round. The vocabulary boundary still depends on sibling discipline rather than enforcement; same posture as Rev 1. |
| L2 | closed | §10 line 267 explicitly documents `mutable: true` as a "read-time eligibility hint... not a safety lock" with the warning "Consumers should not treat `mutable: true` as a permission to mutate." Reinforced by §7 anti-scope and §12 residual. |

## Rev 2 watchpoint judgments

### W1 Codex deferral
**Purpose-fit hand-off.** The Codex fail-closed branch in §4 step 8
is not a deferred stub by the convention's definition: there is no
function declared-but-not-implemented; the §6 API returns a real
`MetadataError::UnsupportedStorage` raised at a concrete site. The
harness contract (`01-session-locate.md:35`) explicitly accepts
`unsupported-storage` as the response when canonical file-backed
metadata cannot be resolved. The Phase 5 hookpoint research is
named with a concrete trigger ("sample real Codex rollout JSONL...
verify a stable root field"), the bad fallback ("commit to
`payload.cwd` without evidence") is forbidden by R1-F02's
fix, and the residual is recorded in §12 and the A4 invalidator.

The harness "still has no answer for Codex" critique is real but
not a purpose violation: the harness's contract is *stable refusal*,
not *universal success*. Exit 12 for all Codex sessions is a stable
contract the harness can pin against and route around (or keep its
v1 direct-read path for Codex specifically). Honest scope.

### W2 path-hash tiebreaker
**Purpose-fit.** The §4 step 8 algorithm — enumerate decompositions
in longest-prefix-of-existing-path-first order, succeed only when
exactly one decoded path exists, exit 12 when two or more exist —
is deterministic and falsifiable. The §9.1 D7-ambiguity test row
covers zero, one, and multiple-existing decompositions, so the
exit-12 fallback is enforced by test intent. The heuristic does
not "mask a deeper problem"; §12 explicitly records that workspace-
root derivation may reject valid sessions whose provider transcript
does not expose an invertible path. Refusal is preferred over
guessing — the very purpose the gate exists to protect.

(Prose nit recorded as R2-F01 below: the §4 prose reads as if it
short-circuits at the first existing decomposition; only §9.1
clarifies that the rule is "exactly one existing decomposition or
exit 12." Clarity issue, not a shortcut.)

### W3 unwrap_or_default
**Purpose-fit citation fix.** R1-F04 was a citation error (Rev 1
implied resume used strict load semantics; resume actually uses
`unwrap_or_default`). Rev 2 §4 step 3 corrects the citation and
commits locate to the same lenient behavior, with downstream
fail-closed (storage_type → exit 12) catching malformed config
naturally. This is the right side of the no-second-ownership-path
rule: locate must not invent a stricter config-load contract than
resume does.

The cost — a typo in `providers.toml` degrades to "unsupported-
storage" rather than "config malformed" — is real but bounded by
resume parity. The harness experiences exactly the same ambiguity
when running `agents resume` today; locate adding a parallel
command does not worsen the user-facing diagnosis path. Recorded
as R2-F02 LOW observation (not a shortcut, an inherited
limitation).

### W4 mutable forward-extension
**Purpose-fit forward-extension note.** The §12 residual ("Once
06-pause-handshake lands, `mutable` will gain a sixth condition")
is the right shape: it explicitly documents contract evolution so
no consumer bakes in `mutable: true` → "safe to write" semantics
in cross-process contexts. The harness misuse risk is mitigated
through three coordinated places: §10 README ("read-time
eligibility hint... not a safety lock"), §7 anti-scope ("No
attempt to make `mutable` a hard import/replace safety lock;
06-pause-handshake owns locks later"), and §13 cross-feature
checklist row.

Refusing to set `mutable: true` until 06-pause-handshake ships
would defeat the field's legitimate read-time purpose (UI display,
eligibility filtering, harness pre-flight checks). The current
residual + documentation strategy is honest about the bound and
keeps the field useful. Not problem-shifting; the lock observation
is a sibling feature with its own surface and its own future
work.

## Findings (severity >= MEDIUM)

None.

## LOW-severity observations / nits

**R2-F01. §4 step 8 path-hash tiebreaker prose is ambiguous about
when the algorithm short-circuits.**
The text reads "generate candidate decompositions in longest-
prefix-of-existing-path-first order and pick the first
interpretation whose decoded path exists on the filesystem. If
two or more decompositions both yield existing paths, treat it as
exit `12`." The first sentence describes a short-circuit ("pick
the first"); the second sentence requires enumerating all matches
("two or more... both yield"). §9.1's D7-ambiguity row clarifies
the rule as "deterministic only when it yields a single existing
decoded path" — i.e., enumerate all, exit 12 if >1. Phase 6 will
need the §9.1 reading; the §4 prose should be tightened
("enumerate all decompositions; succeed if exactly one decoded
path exists; exit 12 otherwise"). Not a shortcut — the §9.1 row
forces correct implementation via test intent.

**R2-F02. Inherited resume-parity malformed-config /
unsupported-storage indistinguishability.**
With Rev 2's `unwrap_or_default` config load (R1-F04 closure), a
malformed `providers.toml` silently degrades to a default empty
config and the harness sees exit 12 `unsupported-storage` instead
of an operational error. This is a real ambiguity — but it is
inherited from resume, not introduced by locate. Resume parity is
the right call (the no-second-ownership-path constraint applies
to config load too in spirit). Phase 6b README review should
mention that "unsupported-storage" can also occur when provider
config is malformed; or a future cross-feature pass could tighten
both resume and locate together. Not a shortcut, an honest
inherited limitation.
