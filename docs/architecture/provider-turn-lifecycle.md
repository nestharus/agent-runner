# Provider-turn acceptance and settlement lifecycle

The ordinary `src-tauri/src/run/resume` path retains manual and automatic
headless execution, prompt acceptance, exact attempt custody, artifacts and
invocation outcome settlement. Its shared delivery and wake internals remain
for a later removal slice.

The unused public `SessionSupervisor`, `SessionMailboxIngress` and
`ProviderTurnAdapter` construction surfaces and their exclusive implementation
and contract tests have been removed. There is no staged adapter activation or
requirement to maintain parity with those retired APIs.

`provider_turn_contract::MAILBOX_BATCH_MAX_ROWS` remains the batch bound used by
the retained mailbox selector. `promote_prompt_acceptance_attestation` continues
to validate ordinary provider prompt acceptance. Launch or transport acceptance
alone does not establish consumption or caller-visible completion.

Canonical session-turn ingest and readers remain part of ordinary account
execution. Neither these projections nor the shared delivery internals become
native-root authority or a migration bridge.
