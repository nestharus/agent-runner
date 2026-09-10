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

## AGE-347 generation4 policy authority and retry boundary

The root's frozen AGE-347 problem generation4 (2026-09-07,
`root_policy_resolution` and `retry_resolution`) supersedes the historical
zero-new-token decisions **only** for the exact wire declaration above in its
protocol declaration, validation, documentation and discriminating tests. It
does not authorize account/provider-specific routing, arbitrary provider
literals, baseline resets, whole-file exclusions or split-token concealment.
Earlier generation3 execution reports remain historical, not current gate passes.

`test-support/provider_wire_policy.rs` is shared by the provider vocabulary,
AGE-244 and AGE-245 guards. It recognizes closed source syntax at exact protocol
roles and removes only the authentic wire identifier from the text being
counted. Every remaining character is still inspected. An extra literal on an
otherwise admitted line, a routing expression using that same wire identifier,
an arbitrary literal, and the same declaration in an unrelated file all remain
violations. Historical baseline values and unrelated findings are preserved.
This source guard is a conservative syntax tripwire, not a substitute for
semantic review of dataflow: a warning declaration must never select a provider,
account, endpoint or native command. Request identity and arithmetic validation
remain mandatory; schema and hostile real-response tests enforce those limits.

The failed-wake fixtures distinguish two causally different failures:

* A failed tail read in `capture_pre_delivery_observation_anchor` propagates
  through `bind_headless_resume_delivery_attempt` **before** the production
  `begin_headless_delivery_submission` CAS. The real admission path records the
  error while the attempt remains `prepared`, unstarted and without an anchor.
  Replacement of that prepared attempt is existing non-submission authority;
  no new provider attestation or state repair is needed. Repeated rejections
  preserve ownership, exponential cadence, saturation of chronology, oldest
  batch FIFO, and eligibility past six retries, then deliver once per batch.
  Rejected anchor reads are not counted as semantic delivery attempts.
* A provider launch exit17 after the CAS, even with no native turn file and an
  empty observation snapshot, leaves the exact started attempt `possible` and
  unresolved across fresh-process rechecks. It cannot be retried as new work.
  Observation may later settle actual exact-envelope evidence; absence never
  fabricates acceptance. There is currently no production negative-submission
  authority for failures after this CAS, including an executor that fails before
  writing anything. Such cases remain pending uncertainty, not legitimate
  automatic resubmission. Changing that boundary requires separate product work.

These distinctions preserve AGE-309's unbounded *genuine* retries without
reintroducing the incident's paid semantic replay, a permanent cap, or a new
scheduler. Test evidence counts anchor admissions separately from native
submissions, durable delivery and invocation completion.

The shared guard also checks the structural warning-schema locations and the
private declaration's warning-only mapper/dataflow. Moving the same schema
syntax under an identity field, exporting the constant through a routing alias,
or feeding provider names instead of response warnings fails those controls.
