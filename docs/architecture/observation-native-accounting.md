# Observation-native read accounting

`session.read_turns` retains the ordinary `source_bytes_examined <=
max_source_bytes` rule for canonical ingestion and for providers that do not
explicitly declare an observation reconstruction allowance. No provider-name
routing or inspection of opaque tokens selects this allowance.

Observation context is the existing `turn_projection=user_observation` request
with a valid `expected_delivery_nonce`, sent through the selected account's
pinned paging-capable endpoint. The response must match that request's instance,
settings, session, projection and page/snapshot position. Neither a warning nor
a provider/product name establishes this context; a declared observation response
to a canonical request still fails the production identity check.

For that observation context only, Runner accepts exactly one declaration in the
existing bounded warnings array:

```
codex_observation_io_v1:forward=<decimal>;reconstruction=<decimal>;metadata=<decimal>
```

This matches the Codex provider's AGE-347 observation contract. Each field must
be an unsigned decimal representable as `u64` (at most 20 digits). The checked
sum of forward and metadata must not exceed the requested source quantum;
reconstruction must be strictly less than 8,388,608 bytes; the checked total of
all three must equal `source_bytes_examined`. Reconstruction is native reading,
not staging I/O. The embedded `contract/v1/session.schema.json` admits total
native reads at most 16,777,215 for declared observation, while canonical and
undeclared observation retain the 8,388,608 structural ceiling. The real client
validates that schema before the runtime validator checks request-relative sums.
This is not the provider-local schema's requirement for an accounting warning on
every observation: the shared Runner contract preserves unchanged providers'
ordinary total quota and permits unrelated warnings alongside one declaration.
The existing warning count/length, response, turn, body,
identity, snapshot, sequence and continuation validation still applies.

Any malformed declaration in this namespace, duplicate declaration, incorrect
projection, overflow, excessive quota or inconsistent total is rejected. A
missing or unrecognized declaration never grants extra quota. Other providers
may continue using the original total quota without this declaration, and
unrelated warnings remain ordinary warnings.

This resource declaration cannot acknowledge a notification. Exact-envelope
matching, durable uncertainty, submission admission, and listener settlement
remain Runner-owned. Cursors remain opaque. Full staging can coexist with
observation recovery, but filesystem or source/identity failures can still
prevent recovery; no restoration or rollback compatibility is implied.

The opt-in tests in `observation_paired_tests.rs` require
`AGE347_PROVIDER_BINARY` to point at a freshly source-built compatible provider.
They invoke only its `describe` and `session.read_turns` methods against synthetic
HOME/data/transcripts. Run the `age347_paired_` tests (including `observation_paired_boundaries.rs`)
with `--ignored`;
the fresh-process recovery helper is invoked by its parent, not independently.
No installed provider, native CLI/model launch, or live state is required.
