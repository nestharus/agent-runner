LOW

This proposal no longer reads like a shortcut-heavy patch. The earlier
high-risk areas have mostly been converted into explicit contracts,
tests, or narrowly-scoped deferrals. On shortcut risk specifically, the
proposal's strongest move is that it removes the temptation to add new
parallel mechanisms. The `mark_resumed_session()` idea was dropped in
favor of reusing `update_session_capture(id, Some(session_id),
"resumed")`, which is the clean V5 answer: one writer for one column
pair, with semantic distinction carried by the persisted method string
rather than a second API that could drift.

Using `session_turns` rather than `invocations.session_id` for provider
lookup is also not a shortcut. The research establishes that
`session_turns` is the populated corpus and `invocations.session_id` is
currently sparse on the live machine. Given V10 and V11, lookup should
use the data source that actually records explicit session ownership,
not a thinner table just because it looks architecturally tidier. That
does leave the broader "capture is not running broadly enough" issue
unsolved, but this proposal is not masking it; it is explicitly relying
on the one table that already holds the needed fact.

The index deferral problem was corrected. Adding
`idx_session_turns_session_lookup` avoids turning "lazy on use" into
"full-scan on every use," so the proposal is no longer load-bearing on
accidental small data. Likewise, always emitting `[resume] -> <provider>`
at a TTY fixes the earlier symptom-masking tendency of hiding the
selection rationale from the user most affected by it.

The main residual shortcut risk is `interactive_args` as a full sibling
of `args`. That is real duplication. The proposal acknowledges it
directly, explains why it is not introducing a more abstract shared-base
shape yet, and adds drift tests plus an explicit followup trigger. That
keeps it on the "acceptable debt" side rather than the "hidden hack"
side, but it is still the area most likely to decay first if provider
configs churn.

The other residual risk is `session_capture_method` remaining an
unconstrained `TEXT` field while adding the new `"resumed"` marker. In
strict terms, yes, that leaves typo risk and does not give the database
an enum-level guardrail. But this proposal is extending an existing
pattern rather than inventing a fresh shortcut solely to dodge a schema
change. I would treat that as a pre-existing looseness in the
`session_capture_method` design, not a proposal-specific failure. If the
project wants stronger invariants later, that should be handled as one
cleanup across all method markers, not by special-casing `"resumed"`.

The signal-handling section is no longer pretending to solve the full
cross-platform problem. It is stated as Unix behavior only, with Windows
left open. The stranded-`running`-row issue is also a legitimate
deferment, not a shortcut: auto-reconciling unknown outcomes at startup
would itself be a V10/V11 violation.

Bottom line: there is some deliberate debt, but I do not see the
proposal fixing one shortcut by introducing another. The remaining risks
are visible, bounded, and called out explicitly.
