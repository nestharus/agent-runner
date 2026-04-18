# Scope Risk Assessment: proposals/02-interactive-resume.md

## Verdict: LOW

**Decomposition assessment: split-recommended.**

The proposal is the right size. It stays inside the synthesis boundary
from `research/02-interactive-resume-needs.md`: promptless interactive
launch, provider-declared interactive/resume command shape, bare-UUID
resume lookup from `session_turns`, explicit observability, and no
silent demotion to a fresh session. It does not drift into a broader
"session management" initiative.

The anti-scope discipline is solid. Section 9 explicitly declines fork
semantics, multi-machine migration/reconstruction, partial or prefix
lookup, OpenCode/GLM expansion, prompt-bearing resume, fresh interactive
session capture, mid-session rebalance, and diagnostics work for
inherited stderr. Those are exactly the temptations this proposal needed
to resist. On that point it aligns well with V16: one concern at a time,
not an omnibus cleanup of every session-related edge.

The proposed two-PR split is appropriate, and I would keep it. PR-E has
a coherent user-visible outcome on its own: `repl` launches a balanced
interactive session through the runner. `repl`, inherited stdio,
invocation lifecycle/finalization, TTY-safe stderr handling, and
`interactive_args` belong together because they are all required to make
interactive launch real rather than half-wired. Splitting PR-E further
would likely create a plumbing-only PR with weak standalone value.

PR-F is also internally cohesive. `--resume`, `[providers.resume]`,
session lookup, duplicate resolution, provider/model mismatch handling,
`"resumed"` persistence, trace wording for resumed attempts, and the
`session_id`-leading index all serve one concern: reopening the correct
existing session cheaply and observably. The index is not opportunistic
bundling; without it, the main resume lookup would knowingly ship with a
bad hot-path query, which would violate V8. Likewise, the trace update
is adjacent but still belongs: once the proposal writes `"resumed"` into
the invocation row, the read path needs to present that state
truthfully under V10.

The important separability question is answered correctly by the split.
`interactive_args` is load-bearing with `repl`; `[providers.resume]` is
not. The research showed current `args` often encode one-shot entry
points like Claude `-p` or Codex `exec`, so `repl` cannot safely ship
without a separate interactive command surface. But plain interactive
launch does not need resume syntax. Resume strategy only becomes
necessary once `--resume` exists. That makes the proposal's fault line
the right one: `interactive_args` in PR-E, `[providers.resume]` in PR-F.

I do not see anything substantial bundled that does not belong. The only
minor place to watch is PR-F's trace rendering work, because that could
expand if reviewers start pulling in broader trace UX changes. The memo
should hold the line that only `"resumed"`-specific observability is in
scope there, not generic trace cleanup. With that guard, scope risk
remains low.
