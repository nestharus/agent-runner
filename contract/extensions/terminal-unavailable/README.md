# Terminal unavailable v1

This provider-neutral, independently versioned extension describes a terminal
failure caused by temporary native model-service overload or unavailability.
It does not assert account quota exhaustion or rate limiting. Recognition of a
native service's error vocabulary belongs in the provider adapter. Scheduling,
replay decisions, and durable incident classification remain host responsibilities.
The signal itself authorizes neither automatic replay nor marking an account
exhausted.

The extension identifier is `oulipoly.terminal_unavailable/v1`; the authoritative
terminal-signal schema is [v1.schema.json](v1.schema.json). Its selection key is
`host.env.OULIPOLY_HOST_TERMINAL_UNAVAILABLE_V1`, with the exact string value `1`,
in the current `launch` or `terminal.classify` request. Ambient process environment
variables do not select the extension. No new describe capability is required:
selection states that the host accepts this optional additional result kind.
Providers that do not implement the extension may retain their previous result.

A selected provider may emit `terminal_signal.kind = "provider_unavailable"`
inside the existing launch `exit` event or `terminal.classify` result. Hosts must
validate the normal envelope, correlation, ordering, process status, and all
other fields, and admit this additional terminal-signal shape only when selected.
Evidence is optional, bounded to 1024 Unicode characters, and must exclude
credentials and request-specific secrets. A fixed diagnostic is preferred.

This is a terminal failure classification. An earlier transient error followed
by a successful exit must not replace `clean_exit`. Cancellation, signal exit,
spawn failure, and prolonged silence retain their process-status classifications.
For an unselected host, the same native service failure remains `nonzero_exit`
with fixed evidence identifying temporary service unavailability. A new provider
must never send the new enum value to a host that did not select it.

## Compatibility and provenance

This artifact extends the existing provider/v1 terminal-signal position; it is
not a second host/provider protocol and does not revise the pinned base snapshot
under `contract/v1`. The SDK's base schema registry and generated DTOs continue
to reject `provider_unavailable`; extension-aware consumers must explicitly
select and compose this artifact into their route's terminal-signal admission.
The standalone Rust DTO and admission helper are in `terminal_unavailable`.

Consumers can copy this complete extension directory byte-for-byte, recording
the SDK source commit and artifact SHA-256, or consume the crate's extension
module. This extension may be added to an already deployed host/provider route
without importing unrelated revisions of its base schema. It does not certify
that an existing route uses the SDK's pinned base snapshot, repair pre-existing
snapshot divergence, or relax the matched-snapshot policy for a base-contract
upgrade. Retain the prior host/provider pair through replacement verification
and restore that pair together if rollback is needed. Changing this extension's
selection, payload, or classification semantics incompatibly requires a new
extension version.
