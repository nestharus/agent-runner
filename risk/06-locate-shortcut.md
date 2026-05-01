# 06-locate — Phase 4 Shortcut Risk Assessment (Rev 3)

## Verdict: LOW

Folding Codex `payload.cwd` derivation into v1 is a purpose-fit
expansion, not a shortcut. Phase 5 supplied concrete empirical
evidence (25 rollout files across Codex 0.46.0 and 0.58.0;
`session_meta.payload.cwd` present in every sampled file —
`research/06-locate-hookpoints.md:164-176`), and Rev 3 honors that
evidence with (a) updated A4 evidence text, (b) a falsifiable
forward-looking invalidator naming schema drift as the failure
mode, and (c) a fail-closed wrapper exiting `12` for any deviation
(`proposals/06-locate.md:60`, `:144`). R2-F01's path-hash short-
circuit prose closes (`proposals/06-locate.md:143`); R2-F02
(resume-parity malformed-config) is unchanged and carried as LOW.
No deferred stubs (workflow convention `no-deferred-stubs.md`); no
backwards-compatibility shims.

## R2 LOW closure

| L# / R2-F | Status | Evidence |
| --- | --- | --- |
| R2-F01 (path-hash prose) | closed | Rev 3 §4 step 8 Claude branch (`proposals/06-locate.md:143`) replaces "pick the first" with "enumerate **all** candidate decompositions… If exactly one decoded path exists, succeed… If zero or two-or-more decoded paths exist, exit `12 unsupported-storage`." Prose now agrees with §9.1 D7 (`proposals/06-locate.md:264`). |
| R2-F02 (resume-parity malformed-config) | unchanged (carried) | §4 step 3 still uses `unwrap_or_default` (`proposals/06-locate.md:137`); WS5 still names it. Rev 3 didn't touch this surface and shouldn't have — resume parity is the right shape. Re-listed below. |

## Rev 3 watchpoint judgments

### Sh-W1 Codex payload.cwd derivation
**Purpose-fit.** Three things keep this honest rather than
premature: (1) the empirical base spans two Codex versions (0.46.0
+ 0.58.0), so the field isn't a single-release accident; (2) the
A4 invalidator (`proposals/06-locate.md:60`) names "upstream Codex
schema drift removes/relocates `payload.cwd` (e.g., a Codex release
nests it differently or makes it optional)" — a concrete observable
that a future Phase-5-style sample can fire; (3) the §4 step 8
Codex branch is fail-closed for absent/non-absolute/non-existing/
non-UTF-8 — schema drift becomes stable refusal, not a wrong
answer. 25 files isn't exhaustive, but the two-version span and
the falsifiable invalidator are what the assumption-register
discipline asks for.

### Sh-W2 path-hash prose tightening
**Closed.** §4 step 8 Claude branch (`proposals/06-locate.md:143`)
replaces "pick the first" with enumerate-all / exactly-one /
else-exit-12. No other §4 step 8 sentence implies short-circuit on
path candidates; the Codex branch's "until a `session_meta` record
is found" is record-discovery first-match, addressed in Sh-W3.

### Sh-W3 Codex line-walk first-match
**Purpose-fit.** "Read the located rollout JSONL line-by-line until
a `session_meta` record is found (one per file by Codex
convention)" (`proposals/06-locate.md:144`) parallels the existing
`scripts/codex-locate-transcript` first-match pattern
(`research/06-locate-hookpoints.md:173-176`). Phase 5's 25-file
sample observed one `session_meta` per file with no exception, so
the convention claim is empirically grounded for the sample base.
Performance isn't a concern (one-shot read; no hot path). The
unprobed multi-record edge is recorded as R3-F01 below — not a
shortcut, just a Phase 6 implementer note.

### Sh-W4 R1 closures still standing
All nine intact:
- **R1-F01** (D5 test row): `proposals/06-locate.md:259` ✓
- **R1-F02** (Codex `payload.cwd` speculative): closure *evolves*
  — Rev 2 closed via fail-closed deferral; Rev 3 closes via Phase-5-
  evidence-backed derivation with falsifiable invalidator. The "no
  speculation" spirit is preserved (Phase 5 supplied the evidence
  the Rev 1 audit demanded).
- **R1-F03** (STATE_DIR mkdir): `:243` ✓
- **R1-F04** (`unwrap_or_default`): `:137` ✓
- **R1-F05** (Claude path-hash tiebreaker): tightened, not undone
  (`:143`).
- **R1-F06** (`migrate-db` overpromise): `:300`, `:315` ✓
- **R1-F07** (`mutable` sixth-condition residual): `:316` ✓
- **R1-F08** (module path): `:16`, `:169` ✓
- **R1-F09** (README `mutable` framing): `:278` ✓ — load-bearing
  for Sh-W5 below.

### Sh-W5 Codex now eligible for mutable: true
**Purpose-fit.** Rev 3 satisfies condition 5 (`workspace_root`)
for Codex; conditions 1-4 were already satisfiable. So Rev 3 does
enable Codex sessions to return `mutable: true` for the first time.
This is correct contract behavior, not a scope leak. The R1-F09
closure (`proposals/06-locate.md:278`) explicitly defends against
the misread: "read-time eligibility hint… not a safety lock.
Cross-process write safety requires the pause/resume-handshake
sibling feature once it lands. Consumers should not treat
`mutable: true` as a permission to mutate." §7 anti-scope (`:230`),
§12 residual (`:316`), and §13 lock-observation row (`:324`) all
reinforce. The four-place defense from Rev 2 carries through
unchanged. Whether 06-import-replace's MVP supports Codex
sessions is its own Phase-3 decision; locate correctly answering
the metadata question does not preempt it.

## Findings (severity >= MEDIUM)

None.

## LOW-severity observations / nits

**R2-F02 (carried).** Resume-parity malformed-config / unsupported-
storage indistinguishability. With `unwrap_or_default` config load
(`proposals/06-locate.md:137`), a malformed `providers.toml`
silently degrades to a default empty config and the harness sees
exit `12 unsupported-storage` instead of an operational error.
Inherited from resume; Rev 3 did not touch this surface. WS5
(audit history line 216) still names this for a future cross-
feature pass.

**R3-F01 (sub-LOW, Phase 6 implementer awareness).** §4 step 8
Codex branch (`proposals/06-locate.md:144`) does not specify
behavior for the unprobed edge of multiple `session_meta` records
in one rollout JSONL. Phase 5's 25-file sample saw one per file in
every case, consistent with the "one per file by Codex convention"
claim. First-match (the existing `scripts/codex-locate-transcript`
pattern) produces deterministic results, but the §9.1 D7 row does
not directly cover the multi-record case. Phase 6 should pick
first-match for parity with the existing locator and document the
choice in code; not a Phase 4 blocker.
