# Observation-native read accounting

`session.read_turns` retains the ordinary `source_bytes_examined <=
max_source_bytes` rule for canonical ingestion and for providers that do not
explicitly declare an observation reconstruction allowance. No provider-name
routing or inspection of opaque tokens selects this allowance.

For `user_observation` only, Runner accepts exactly one declaration in the
existing bounded warnings array:

```
codex_observation_io_v1:forward=<decimal>;reconstruction=<decimal>;metadata=<decimal>
```

This matches the Codex provider's AGE-347 observation contract. Each field must
be an unsigned decimal representable as `u64` (at most 20 digits). The checked
sum of forward and metadata must not exceed the requested source quantum;
reconstruction must be strictly less than 8,388,608 bytes; the checked total of
all three must equal `source_bytes_examined`. Reconstruction is native reading,
not staging I/O. The existing warning count/length, response, turn, body,
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
HOME/data/transcripts. Run the three `age347_paired_` tests with `--ignored`;
the fresh-process recovery helper is invoked by its parent, not independently.
No installed provider, native CLI/model launch, or live state is required.
