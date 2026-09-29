# AGE-319 installed connected async Bash receipt and ACK — 2026-09-28

## Result and boundary

The disposable root-mapped schema-2 first-install path now runs the installed
launcher, the normal model/provider, and a clean featureless `agent-bash run
--delivery async` image. Bash returns `schema_version=30` and
`dispatch_state=broker-k-consumed` to the provider before its child completion.
The workload writes one physical effect line and distinct stdout/stderr bytes.
Broker later selects the actual child Q/W event and the original, pinned root
Runner receives the selected F payload over the v30 recipient socket. That
Runner decodes and verifies the raw stream bytes, event hashes, source/root
binding and drained success; it creates and fsyncs an exclusive receipt file
before sending the exact F token. Its separate ACK readback is retained and
checked against the recipient grant and SQLite's transactional ACK evidence.

The test checks L pair/source/root identity, one E and P/G/A/J, U/D, normal
K/Q and caller publication, Bash C and child K/Q/W, selected notify source/row,
F grant and payload, one manual ACK with one delivery attempt, and the installed
terminal certificate/owner close. The Bash source's child D session differs
from the root recipient's D session; both are checked against their own durable
admissions. One effect line and one consumed child K exclude duplicate physical
work in this fixture. The existing sync and help cases are retained.

## Parent lifetime finding

The first two attempts let the normal provider return as soon as Bash returned
K. They failed before receipt: the Bash child had C, a consumed K and captured
stdout/stderr, but its physical exit publication remained temporary, its Q/W
and F row were absent, and the provider namespace helper exited uncleanly.
The root's readback stayed `not_applicable` with unresolved child C. Extending
the recipient wait did not fix it. The child ran in the normal provider's
namespace lifetime; this route does not currently prove that a Bash child can
survive that parent's exit.

For this busy-recipient proof, the fixture provider holds its own lifetime
after Bash's async K until Broker publishes the selected child W artifact.
That artifact is only a lifetime barrier. It is **not** used as a recipient
receipt or ACK: the pinned root Runner separately receives F payload bytes
through the challenged socket and records its own fsynced receipt before ACK.
This test does not prove sleeping-recipient activation, detached parent
survival, simultaneous roots, host installation, or AGE-353 capacity. The
1,056 and 100×100 benchmarks still require independent receipt/ACK and
pre-cleanup physical drain on their own exact images.

The fresh root terminal readback reports `notification_state=acked` with
`ack_basis=manual_ack`. Its separate `execution_state=unknown` projection is
not used as normal provider execution proof; normal K/Q, publication, installed
terminal certificate and owner close have their own exact readbacks.

## Source and verification

- Runner base: `7e4aba4247e861db475a637a990ec06da850ba67` on
  `age319-connected-async-bash-20260928`. Bash trunk was read-only and clean at
  `6c0a83baa8e6cfcb518c31584da0414265bf1df2`; the build used no feature flag.
- Build: `CARGO_TARGET_DIR=/tmp/age319-connected-async-bash-target cargo build --locked --bin agent-bash`
  from the Bash trunk — pass. Bash SHA-256:
  `741a1975fbe7fa666aaac5195cc81697ae626df4bfe5a4899b54a8b7decdc54e`.
- Build: `cargo build --locked --no-default-features -p oulipoly-agent-runner
  --features age319-private-broker-fixture --bin oulipoly-agent-runner` — pass.
  Final fixture image SHA-256 values: Runner
  `9491806f65dbd4afef011a6a2029ef6d4ca2a8ec3b8bc62fe789ed782a13322c`,
  Broker `1cf5eac9276e86c99eb56f519d30491face622537b8e2e9841ac8677a11b134c`,
  launcher `3076b134563b399efc751339ffbaec7cb6b3737cbf0b8b9c828f9142bd037adb`,
  and Bash as above. The fixture copies these into its disposable installed
  directory and attests the schema-2 manifest before entry.
- Exact async test: `AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner"
  AGE319_CONNECTED_BASH_BIN=/tmp/age319-connected-async-bash-target/debug/agent-bash
  cargo test --locked --no-default-features -p oulipoly-kernel-broker
  --features age319-private-broker-fixture --test age319_connected_control
  root_mapped_connected_l_delivers_real_bash_async_to_recipient_and_acks
  -- --exact --nocapture` — pass after the parent lifetime and identity fixes.
- Full connected suite command is the same environment and Cargo options,
  omitting the exact test filter. A concurrent run passed all ten once; after
  the final stream-verification tightening, another concurrent run passed the
  async case and eight other cases but timed out in the existing sync case's
  owner-close wait (`state_sidecar_outstanding_unknown=true`). The final serial
  rerun with `-- --test-threads=1 --nocapture` passed **10/10** in 190.46 s,
  including the existing sync/help cases and new async case.
- `cargo fmt --all --check`, `git diff --check`, and Bash trunk cleanliness
  are final local checks; all pass.

All paths, config, State and effects were disposable under the private user
namespace. No host install, alias, service, existing user State, provider API,
or unrelated process was changed.
