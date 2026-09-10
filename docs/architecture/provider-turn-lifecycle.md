# Provider-Turn Acceptance And Settlement Lifecycle

This document owns the relationship between the current headless resume path
and the resident-supervisor provider-turn adapter. They are a staged migration,
not two production alternatives.

## Current Production Authority

`src-tauri/src/run/resume` is the production authority for manual and automatic
headless resume. Its terminal and wake modules currently acquire transcript and
zero-turn evidence, validate prompt acceptance, classify terminal outcomes,
settle mailbox rows, finalize invocations, and drive wake coordination.

`ProviderTurnAdapter` is production-capable but is not a production entrypoint.
It currently has no construction or consumption site outside its contract
tests. It defines the target adapter between `SessionSupervisor` turn requests
and the existing CLI or external-provider execution boundaries.

## Compatibility Period

Until the joined cutover, changes to provider prompt acceptance, submission or
confirmation evidence, exact session/generation/invocation fencing, invocation
finalization, per-turn caller results, or mailbox batch bounds must be assessed
against both paths. A change may update only one path when the other operating
domain cannot express that concern, but the non-applicability must be explicit
in the change. Neither path may weaken the shared validation performed by
`promote_prompt_acceptance_attestation` or treat process launch, transport
acceptance, assistant absence, malformed evidence, or
`resume_completion_unconfirmed` as stronger evidence than its contract allows.

`provider_turn_contract::MAILBOX_BATCH_MAX_ROWS` is the single provider-turn
mailbox batch bound. The current mailbox selector and target adapter validator
must consume that owner directly; changing the bound is one atomic compatibility
change rather than an independently evolving producer and consumer decision.

The paths deliberately differ at their lifecycle boundaries:

- `run::resume` acquires provider transcript and zero-turn evidence, applies
  provider routing and retry policy, projects delivery into the current mailbox
  sidecar, and coordinates the current wake lifecycle.
- `ProviderTurnAdapter` receives already fenced evidence from resident-owner
  ports, projects submitted/confirmed stages into
  `session_delivery_acknowledgements`, finalizes the exact invocation, and
  completes one `SessionSupervisor` turn while its owner remains resident.

These representation and coordination differences are permitted during the
compatibility period. Evidence trust, exact identity, monotonic settlement, and
one caller-visible completion are compatibility obligations rather than
independent conventions.

## Activation And Retirement

AGE-278 owns the joined cutover after its session-authoritative routing and
targeted-recovery prerequisites are satisfied. Activation requires ordinary
manual and automatic headless turns to construct the exact `TurnRequest`, run
through `ProviderTurnAdapter`, and publish the corresponding `TurnCompletion`.
Cutover proof must cover current CLI and external-provider callers and must
prove that one turn cannot be settled by both paths.

After those production callers and proofs are active, the direct provider-turn
acceptance and settlement ownership in `run::resume` must be removed. Resume
code may remain as provider-specific evidence acquisition or routing input to
the adapter, but it must no longer independently decide and apply the same turn
outcome. If the joined cutover instead abandons `ProviderTurnAdapter`, that work
must remove its public export and maintained contract tests rather than leave a
dormant second authority.
