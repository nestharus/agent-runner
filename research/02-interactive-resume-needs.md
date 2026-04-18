# Synthesis: Interactive Launch + Cross-Provider Resume

This document maps `research/02-interactive-resume-problem.md` to
`oulipoly-agent-runner`'s specific situation and the user's stated
intent. The output is a scoped need-set the proposal phase will
design against, plus an explicit list of open questions the proposal
must resolve.

This is the user's last lightweight gate before the proposal phase.
After approval the 3x risk gate becomes the next checkpoint.

---

## 1. The user's stated intent (verbatim recap)

From `initiatives/02-interactive-and-resume.md`:

> `agents modelid` will start an interactive session with the model.
> ... the user also doesn't need to pass in flags every time like
> `--dangerously-skip-permissions`.
>
> `agents modelid --resume sessionid` ... will automatically figure
> out which provider has the sessionid. It will also automatically
> restore the most recent sessionid (in case there are conflicts).

Decomposed:

- **Interactive launch**: drop into a real REPL, runner picks the
  account, runner applies provider startup args automatically.
- **Cross-provider resume**: user supplies session UUID, runner
  finds the provider, conflict-resolves to most-recent on duplicates.
- **"That will work"** hasn't been said again, but the brief is the
  user's spec.

---

## 2. Scope decisions

### In scope

1. **Interactive launch** — `agents <model-or-flag>` enters a TTY
   REPL with the wrapped CLI, runner having quota-balanced and
   applied per-provider interactive args.
2. **Cross-provider `--resume`** — runner finds the provider
   hosting `<session-id>` from `session_turns`; if multiple, picks
   the most recent by `MAX(timestamp)`; if none, errors with a
   useful hint.
3. **Stable trace correlation when stderr is not a TTY** — emit
   `OULIPOLY_INVOCATION` per Init-01 contract; suppress on TTY to
   avoid screen pollution.
4. **Per-provider interactive command surface** — extend
   `ProviderConfig` so each provider can declare both its one-shot
   args (existing) and its interactive launch args (new). Same goes
   for resume, since Codex's resume is a subcommand and Claude's is
   a flag.

### Out of scope

- **Mid-session quota rebalance** — once attached to a TTY, you're
  committed. Density scoring runs only at provider-pick time. (The
  research confirmed this matches current architecture.)
- **OpenCode/GLM interactive + resume** — only verified for Claude
  and Codex; other CLIs' interactive mode and resume contract are
  out-of-scope until per-CLI session_capture matrix expands.
- **Fork semantics** — `claude --fork-session` and `codex fork` are
  distinct from `--resume`. The user asked for resume, not fork.
- **Multi-machine session migration** — if a session_id is in a
  user's DB but the wrapped CLI's local store doesn't have it, we
  can't actually resume. Document the failure mode; don't try to
  smuggle session state across machines.
- **Validating model continuity** — both Claude and Codex allow
  model switching on resume. The runner doesn't second-guess.
- **Print-mode forced-id resumability** — the research surfaced
  that Claude `--session-id` in print mode may produce
  non-resumable sessions. That's an Init-01 PR-C concern, not
  Init-02. Document as an Init-01 followup, don't fix here.

---

## 3. The four schema/contract gaps that block in-scope use cases

Direct from the research:

1. **Interactive command shape per provider not declared.** Today
   `ProviderConfig.args` is one-shot-implicit (`-p` for Claude,
   `exec` for Codex). Need a way to express "use these args for
   interactive launch" vs "use these args for one-shot."
2. **Resume command shape per provider not declared.** Claude
   uses a flag (`--resume <id>`); Codex uses a subcommand
   (`codex resume <id>` interactive, `codex exec resume <id>`
   non-interactive). Need declarative resume strategy per
   provider, mirroring the `session_capture` pattern from PR-C.
3. **No way to look up "which provider hosts session X."** The
   data exists in `session_turns(provider_name, session_id, ...)`
   but no method on `StateDb` returns
   `(provider_name, latest_timestamp)` for a given session_id.
4. **CLI parser doesn't distinguish "no prompt = interactive"
   from "no prompt = error."** Today
   `resolve_prompt(cli, ...)` errors when there's no positional
   arg, no `-f`, and stdin is a TTY. The new flow needs a new
   entry path that doesn't run that check.

Other research-flagged gaps that are NOT blocking and stay
deferred:

- Schema additions for "interactive vs one-shot" markers on the
  `invocations` row (could be useful for trace's per-node
  display; not required for resume).
- A way to invalidate the runner's view of "session exists" if
  the wrapped CLI's local store gets cleaned up (out of scope).

---

## 4. Tradeoff commitments (constraints on the proposal)

### V13 — Composite identifiers

The user said `agents <model> --resume <session-id>`. The session
id is bare. The runner's job is to **find** the provider — that's
the whole point of cross-provider resume. The CLI input does NOT
take a composite. The provider is *output* (logged to stderr or
chosen silently), not *input*.

### V8 — Lazy on use

Provider lookup happens once at resume time via a single SQL
query against the existing `(provider_name, session_id)` index. No
pre-fetch, no scan.

### V10 — Failures observable

If `--resume <id>` finds no provider: error to stderr clearly
("No session found matching <id>. Run `oulipoly-agent-runner trace
<invocation_uuid>` for context, or check that session ingestion is
configured for this provider via `sessions.toml`.") with exit 1.
Don't fall through to a fresh interactive session — that's a
silent demotion the user would never expect.

### V1, V2, V3 — Declarative, not procedural

- **No CLI-name sniffing.** No `if cli == "claude"` branches.
- Each provider declares its `interactive_args` and `resume`
  strategy in TOML (mirroring `session_capture` from PR-C).
- Resume shape is two declarative strategies:
  - `flag` — append `<flag> <session_id>` to the args
    (Claude: `--resume <id>`)
  - `subcommand` — replace the suffix args with a subcommand
    sequence that takes the session id as positional
    (Codex: `resume <id>` interactive, `exec resume <id>`
    non-interactive)
- Interactive args are a per-provider override of the args
  default; if absent, fall back to `args` (one-shot).

### V14 — No compat shims

The `Cli` struct already has subcommand dispatch from PR-B
(`Subcommands::Trace`). New entry points become new subcommand
variants — no flag-on-existing-Cli that conflicts with current
positional semantics.

### V15 — Surface choice belongs to the caller

The runner emits `OULIPOLY_INVOCATION` on stderr **only when
stderr is not a TTY**. Wrapping scripts get the line; users at a
terminal don't. Implementation: `std::io::stderr().is_terminal()`
gate, just like `resolve_prompt`'s existing TTY check.

`OULIPOLY_PARENT_INVOCATION` env var write to the spawned subprocess
**always happens** — env vars don't visually pollute the screen,
and a script-driven interactive session that itself spawns a child
agent still wants the parentage.

### V11 — Explicit propagation, not inference

When the user runs `agents <model>` with `<model>` referring to
something that's also a valid agent name (rare but possible), do
NOT infer "they probably meant interactive model" — explicit
syntax. Either:

- A new subcommand: `agents repl <model>` — analogous to
  `agents trace <uuid>`. Unambiguous.
- A new flag: `agents -m <model> --interactive` — works, but
  `-m` is currently always paired with a prompt, so adding an
  "interactive prompt-less" mode to the same path is muddy.

The proposal picks one. My recommendation is `repl` as a
subcommand — most aligned with V14 (no flag-on-existing-Cli that
conflicts).

### Provider selection during `--resume`

Density scoring is irrelevant. The session is hosted on a specific
account. We use that account, period. If quota-blocking errors
arise mid-resume, those are runtime concerns; don't pre-emptively
balance.

### Conflict resolution for duplicate session_ids

The user said "most recent." Operationalize: among providers
hosting the same `session_id`, pick the one with `MAX(timestamp)`
in `session_turns`. SQL is straightforward:

```sql
SELECT provider_name
FROM session_turns
WHERE session_id = ?
GROUP BY provider_name
ORDER BY MAX(timestamp) DESC
LIMIT 1;
```

The research confirmed this is a real (not theoretical) need.

### Partial-id (prefix) lookup

The brief says `--resume <session-id>` (full UUID). Research
showed users may paste prefixes. **Not in scope for v1** — full
UUID required. If v1 ships well, prefix lookup is an obvious
followup. The proposal can mention this as anti-scope so risk
review doesn't flag it as a gap.

---

## 5. Critical unknown the proposal must resolve

**How does the new declarative resume strategy interact with
session_capture from Init-01 PR-C?**

When a user runs `agents claude-opus --resume <session-id>`:

1. Runner picks provider via lookup (NOT density).
2. Runner needs to choose: do we still try to capture
   session_id during this run? The session_id is already known —
   we don't need to *capture* it, we're *forcing* it in.
3. With Claude's `--session-id` strategy from PR-C, forcing a
   resumed session id might do something different than
   capturing a fresh one (the research showed weird behavior
   with print-mode `--session-id` not being resumable).

The proposal must specify how `session_capture` and the new
`resume` strategy compose. Likely: when `--resume` is used, the
runner's session_id is ALREADY KNOWN, so capture is a no-op
(just record the supplied session_id). But the wrapped CLI may
still need certain capture-related args for readback verification,
or may need them suppressed.

This is the single hardest design problem in scope.

---

## 6. Other open questions the proposal must resolve

- **What does `interactive_args` look like in the TOML?** A new
  optional field `interactive_args = ["..."]` on the provider
  block, falling back to `args` when absent? Or a separate
  `[providers.interactive]` table? The proposal picks one.
- **What does `[providers.resume]` look like in the TOML?**
  Mirror `[providers.session_capture]` shape — `kind = "flag"`
  vs `kind = "subcommand"` plus per-kind fields.
- **Do `repl` invocations get a row in `invocations`?** Yes, per
  V10 observability — they should. A trace command on the
  invocation_uuid should show "interactive REPL session with
  claude2, started at X, finished at Y" even if no transcript
  was captured.
- **Stderr emission for `repl`**: `OULIPOLY_INVOCATION` only
  when `!isatty(stderr)`. Confirmed.
- **What if `--resume` is given a session_id from a provider
  that's not currently configured in sessions.toml** (so
  session_turns has no rows for it)? That's the "session not
  found" path. Error with the V10 hint.
- **Cleanup on subprocess crash**: interactive REPLs can exit
  uncleanly (SIGINT, terminal close, etc). The runner's
  invocation row needs to be finalized regardless. Use existing
  PR-A finalize-invocation pattern in a Drop guard?
- **The Init-01 PR-C followup**: Claude print-mode
  `--session-id` not being resumable. Document as a known
  limitation, defer fix. (Possibly: Claude's session_capture
  should be applied differently in interactive mode vs
  print mode — but that's PR-C territory.)

---

## 7. What the proposal will look like

The proposal author should produce
`proposals/02-interactive-resume.md` covering:

- **CLI shape** — `agents repl <model> [--resume <session-id>]`
  as a subcommand. (Or whatever the proposal picks; must be
  unambiguous and not conflict with current positional
  semantics.)
- **Provider config additions** — `interactive_args`,
  `[providers.resume]` block with `kind = "flag" | "subcommand"`
  and per-kind fields.
- **Provider lookup query** — `StateDb::find_provider_for_session`
  method that returns `Option<String>` (provider_name) ordered
  by most-recent timestamp.
- **TTY handoff in executor** — `Stdio::inherit()` for stdin/
  stdout/stderr; new `execute_interactive` entry point parallel
  to `execute_with_inputs`.
- **Stderr emission gating** — `is_terminal()` check, suppress
  on TTY.
- **Lifecycle integration** — repl invocations get
  start/finalize rows; finalize on child exit; finalize on
  Drop for crash safety.
- **Composition with session_capture** — explicit decision on
  whether/how capture runs during resume.
- **Anti-scope** — fork, multi-machine resume, prefix lookup,
  print-mode forced-id resumability.
- **Tradeoff justification recap** referencing V1/V8/V10/V11/
  V13/V14/V15.
- **Open questions for the risk gate** — Codex's
  `--skip-git-repo-check`-style required flags in resume mode,
  Windows TTY handoff (probably out-of-scope but worth flagging),
  the PR-C composition.
- **PR decomposition** — likely 2 PRs:
  - PR-A: `repl` subcommand + `interactive_args` + TTY handoff +
    lifecycle wiring (no resume, just interactive launch)
  - PR-B: `--resume <session-id>` + provider lookup +
    `[providers.resume]` declarative strategies + conflict
    resolution

---

## 8. Confidence note

The user's design ("just give me the session id, you figure out
the provider") was already empirically supported by the data
(13,891 distinct sessions persisted, including a real
cross-provider duplicate). The architecture from Init-01 (declarative
per-provider config, lazy lookup, V10 observability) maps cleanly
onto this. The proposal should be more straightforward than
Init-01's, primarily because the storage layer already has what's
needed.

The one hard part is the TTY handoff + composition with PR-C's
session_capture. Everything else is mechanical.
