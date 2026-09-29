# AGE-319 connected installed pair with real Bash — 2026-09-28

## Result

The root-mapped, disposable first-install fixture activates a schema-2 pair
with four distinct installed images. Its Bash slot is copied from a clean
default-feature `agent-bash` build in a separate target directory. The test
asserts the source and installed Bash SHA-256 are equal, the Bash digest differs
from Broker, and that exact digest appears in both the pair manifest and the
first-install activation readback. Broker's challenged Bash operations require
the peer executable to be this pinned image.

The original installed launcher selects `fixture-model` through the ordinary
normal-model path. The selected local provider is a shell wrapper that waits
for an attached, synchronous `agent-bash run` child. Its `/bin/sh` workload
writes the sole effect marker and emits distinct stdout and stderr. The wrapper
restores the disposable fixture socket address under the Bash probe variable:
normal provider planning removes `OULIPOLY_KERNEL_*` inherited environment
keys, while installed Bash would use `/run/oulipoly-kernel-broker/v30.sock`.
This address bridge exists only in the private-root fixture.

The passing test observes exact L pair/source/root identity; one E; P/G/A/J;
U and D; the normal model's single State K, physical Q with zero wait status
and tree drain, caller publication, and root owner-close certificate. It also
observes Bash's challenged parent probe, one C row, its D and selected ordinary
child plan, child K and physical drain, accepted W, one synchronous response
reservation, and the exact child stream bytes and SHA-256 values. The child
parent grant is the normal provider's consumed admission ID and its attached
PID 1 receipt; C, K, W and the test each recheck that causal binding. One
effect-marker line, one child consumed K, and one C/W/response row rule out a
duplicate effect in this run. The original installed caller receives a terminal
certificate and exits 0. The eight prior connected cases also pass.

## Source change

The connected normal provider uses `normal_physical` K, while the prior Bash
parent checks understood only a `fresh_provider` root grant. The first real
Bash probe refused with `consumed causal parent work grant absent`. The normal
provider PID 1 now retains its host PID/starttime and namespace receipt before
spawning the configured provider. Broker reopens exact normal State K and the
receipt at Bash probe/C and child K, including live namespace lineage. State W
reopens that same K and receipt, then checks the child K/Q and stream artifacts.
Missing or changed parent evidence is an error; there is no child retry after
K. The existing `fresh_provider` parent path remains selected when no normal K
exists.

The early featureless Bash source path was checked before choosing the fixture
environment. Under a root-mapped user namespace with the fixture socket key,
`agent-bash` sends challenged opcode `0x90` before legacy config or C. It
accepts a validated released parent only for ordinary tree completion. An
exact outside-root refusal falls back to legacy; malformed or other refused
parents fail before C. The separate Bash `fresh_probe_routing` test passed.

## Verification

- Bash source: `/home/nes/projects/agent-bash-tool/trunk` remained clean and
  read-only. Build: `CARGO_TARGET_DIR=/tmp/age319-agent-bash-target cargo build --locked --bin agent-bash` — pass, no feature flags. Final binary SHA-256:
  `741a1975fbe7fa666aaac5195cc81697ae626df4bfe5a4899b54a8b7decdc54e`.
- Runner: `cargo build --locked --no-default-features -p oulipoly-agent-runner --features age319-private-broker-fixture --bin oulipoly-agent-runner` — pass. The fixture feature enables the disposable connected entry; the Bash build has no fixture feature.
- Bash probe: `CARGO_TARGET_DIR=/tmp/age319-agent-bash-target cargo test --locked --test fresh_probe_routing -- --nocapture` — 1 passed.
- Connected pair, from this worktree:

  ```bash
  AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" \
  AGE319_CONNECTED_BASH_BIN=/tmp/age319-agent-bash-target/debug/agent-bash \
  cargo test --locked --no-default-features -p oulipoly-kernel-broker \
    --features age319-private-broker-fixture --test age319_connected_control -- --nocapture
  ```

  Result: 9 passed, 0 failed, including the new real Bash case.

- After adding the exact single child K assertion, the focused rerun used the
  same environment and command with
  `root_mapped_connected_l_runs_real_bash_sync_child_and_closes_owner -- --exact --nocapture`:
  1 passed, 0 failed; 8 filtered out.
- `cargo fmt --all --check` and `git diff --check` — pass.

- The older `private_root_join` ordinary Bash sync reference was attempted
  with the current feature-enabled Runner image. It stopped before J with
  `OULIPOLY_KERNEL_ENTRY_GAP=v30 connected control grant absent`; that older
  harness does not issue the installed connected grant. It gives no regression
  verdict for the private-root path.

The synchronous Bash response retains `phase=unknown`: durable accepted W and
one response reservation are present, and the original attached Bash process
returned the verified bytes to its provider. There is no separate durable
end-consumer ACK for this synchronous response. No host installation, host
aliases, State, services, or existing processes were changed. A fixed-path host
install, live provider, and AGE-353 stress/benchmark remain unproven. The
parent owns PR, merge, and broader observational review after this branch.
