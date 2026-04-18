# Initiative 02: Interactive sessions + auto-resume

**Status:** Backlog. Picks up after Initiative 01 (trace inspection)
ships.

## What the user asked for (verbatim)

> agents model --resume sessionid
>
> `agents modelid` will start an interactive session with the model.
> We can define the command to use to start the model up as well for
> any given provider (like dangerously-skip-permissions). The purpose
> of this is so that users don't have to look up their usage. agents
> will just select the provider that is most available. The user also
> doesn't need to pass in flags every time like
> `--dangerously-skip-permissions`.
>
> The next feature is
>
>     agents modelid --resume sessionid
>
> This will automatically figure out which provider has the sessionid.
> It will also automatically restore the most recent sessionid (in
> case there are conflicts). So this allows a user to restore session
> ids without having to remember which session id was associated with
> which provider.

## Two coupled features

### A) Interactive session launch

```bash
agents <model>            # no prompt, no -p, no -m flag — interactive
agents claude-opus        # drops into claude opus interactive REPL
agents claude-haiku       # ditto, but quota-aware provider pick
```

Today `agents -m <model> "<prompt>"` is one-shot. The new mode would:

- Quota-balance to pick the best provider for `<model>` (existing
  density scoring already does this for one-shot; reuse the path)
- Invoke the wrapped CLI in **interactive** mode — no `-p`/`--print`
  flag, attach the user's terminal stdin/stdout/stderr to the
  subprocess
- Apply the provider's configured startup flags (e.g. `claude
  --dangerously-skip-permissions`) automatically. Today this would
  already work via the `args` field in the model TOML — verify this
  during the proposal.

Open question for proposal phase: do interactive sessions still emit
`OULIPOLY_INVOCATION=...` on stderr? They probably should, so the
session is still trace-able; but the user is at a TTY, so a stderr
line might land on screen. Maybe gate it on `isatty(stderr)` — emit
only when stderr is piped.

### B) Cross-provider session resume

```bash
agents <model> --resume <session-id>
agents claude-haiku --resume 9e69e8cc-616d-4640-bf1d-96f5391b1a2e
```

The user supplies just the session UUID. The runner:

1. Looks up `<session-id>` across all known providers in
   `session_turns` (already keyed by `(provider_name, session_id)`).
2. If found in exactly one provider → resume there.
3. If found in multiple providers → pick the one with the most
   recent turn timestamp for that session_id. (Conflict resolution
   the user explicitly asked for.)
4. If not found → error with a hint to run `oulipoly-agent-runner
   trace <invocation_uuid>` to find what session_ids exist.
5. Invoke the wrapped CLI with its native resume flag (Claude:
   `--resume <id>`; Codex: `--resume <id>`; per-CLI matrix needed).

Why this matters: the user's mental model is "I got an ID, I want
back into that session." They shouldn't have to remember
`agents -m claude-haiku-on-account-3 --resume X`.

## Connection to Initiative 01 (trace inspection)

Initiative 01 is producing the IDs:

- `invocation_uuid` — the agent-runner's own ID, returned on stderr,
  walkable via `trace`
- `session_id` — the wrapped CLI's session ID, captured per-CLI

`--resume` consumes the **session_id**. The trace command surfaces
both IDs per node. So the natural workflow is:

```bash
agents -m claude-opus "Refactor X" 2>run.err          # capture invocation
INV=$(sed -n 's/^OULIPOLY_INVOCATION=//p' run.err | jq -r .id)
agents trace $INV --json | jq '.root.session.id'       # find session
agents claude-opus --resume <that-session-id>          # resume
```

Initiative 02 should make step 4 trivial: the user just passes the
session ID and agent-runner figures out the rest.

## Hard questions for problem research / proposal phase

1. **Interactive mode and quota refresh** — quota refresh today is
   lazy at one-shot invocation time. An interactive session may run
   for hours, burning quota without re-checking. Does the runner
   need to re-balance mid-session? (Probably not — once attached to
   a provider's TTY, you're committed for that session. But document.)
2. **Per-CLI resume flag matrix** — Claude has `--resume`, Codex has
   `--resume`, but their semantics differ (does Claude reload
   the session into a fresh context window? Does Codex append to
   the existing rollout file?). Verify empirically the way Initiative
   01 verified `--session-id`.
3. **What if `--resume <id>` is passed with `<model>` that doesn't
   match the session's original model?** E.g. session was created
   with `claude-opus` but user runs `agents claude-haiku --resume X`.
   Reject? Allow with a warning? Ignore the model arg? The user's
   stated intent suggests "auto-figure-out the provider" which might
   imply auto-figuring out the model too.
4. **Interactive + parent invocation propagation** — if interactive
   session is itself a child invocation (rare but possible — a script
   spawns `agents claude-opus`), how does parent ID propagation
   interact with TTY attachment?
5. **Provider availability vs density** — "most available" might mean
   "highest density" (the existing balancer answer) but might also
   mean "definitely won't blow the 5h cap during a long interactive
   session." Different metric. Worth a research pass.

## When to start

After Initiative 01 ships and the trace+session_id capture is
verified working. Initiative 02 depends on:

- `session_id` being reliably stored per-invocation (Init-01 deliverable)
- `OULIPOLY_INVOCATION` stderr emission being stable (Init-01 deliverable)
- Per-CLI session capture matrix being established (Init-01 research
  artifact in `proposals/01-trace-inspection.md` section 4)

Init-02 then extends that matrix to include resume and interactive
flags per CLI.

## Suggested next step

When ready, kick off the strategy pipeline: spawn `gpt-high` with
`tmp/02-research-prompt.md` (to be written) covering:

- Use cases for interactive + resume (probably narrower than
  observability — this is a UX feature)
- Existing Claude/Codex resume mechanics and limitations
- Tradeoff axes (interactive TTY handoff, quota mid-session,
  multi-provider session_id collision policy)
- Open questions including the five above
