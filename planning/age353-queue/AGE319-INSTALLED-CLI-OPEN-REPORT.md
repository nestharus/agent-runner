# AGE-319 installed CLI open slice — 2026-09-29

## Disposition

**Host admission remains closed.** This branch is a coherent production-path
prerequisite, not an installed Runner/Bash v30 release or AGE-353 benchmark
authorization. The host-root Broker rejects `L` before request ledger
reservation, even for supported headless syntax. No host State, install,
service, alias, or Bash checkout was changed.

The starting commit already contained a Broker-spawned connected control
Runner, durable request/root ledger, v30 `E/P/G/A/J/U/D`, physical close,
and terminal status join beyond the earlier entry report. This slice made that
path executable in a **featureless/default-disabled disposable user namespace**
and repaired reuse after an exactly closed help root. It does not treat the
private feature-gated executor or v2/v29 authority as production proof.

## Source changes

- A root-mapped disposable user namespace can run the same default-disabled
  installed launcher, Broker, and Runner images against a private schema-2
  pair, source, and socket. The runtime override requires mapped UID 0 with
  host UID nonzero. The host-root image continues to use fixed installed
  paths. The detached control FD, Broker peer, image, and exact pair/source/
  request/root still gate `E`; an environment variable or copied FD cannot
  reserve a root.
- Disposable-only launcher controls select one request ID, drop its first `L`
  reply, and resubmit the exact same `L`. The launcher uses read-only status
  for uncertainty. A Broker refusal with no ledger record is reported as a
  refusal, while only a matching `exit <code> drained <request ID>` becomes
  caller success or failure.
- A validated offline terminal certificate now has an exact digest marker in
  the entry registry. The Broker writes it only after joining the prior
  help/diagnostic root to physical close, owner close, the installed terminal
  certificate, and State continuity. This lets a later `E` proceed without
  treating help as an ordinary provider publication. An absent or changed
  certificate still blocks the next root.
- An entryless orphan root, work, grant, or physical source now blocks a new
  `E` instead of passing the empty-entry fast path.
- Host-root production `L` has an explicit pre-ledger closed guard until the
  remaining connected proof is complete. The read-only status route remains
  available for an existing exact request.

## Observed connected evidence

The new `age319_featureless_installed` test used actual current default-disabled
Runner, Broker, and launcher binaries plus a featureless `agent-bash` built from
the current Bash checkout into a separate temporary Cargo target. It copied
all four images into a disposable schema-2 pair, bootstrapped and activated a
private v30 source, then served the Broker in a user and PID namespace.

Observed in that test:

1. Direct Runner, copied control FD, unsupported flag, and stale pair were
   refused before any installed request or root record.
2. A supported help `L` with a deliberately dropped reply and duplicate
   submission produced one durable request/root binding. The launcher
   recovered by read-only status and returned after a validated offline
   terminal certificate. A different launcher process could not reuse the
   request ID.
3. A subsequent normal model invocation ran one local provider effect and
   produced a distinct validated drained terminal. Its provider observed the
   positional prompt and the exact launcher working directory through the
   connected custody path. This exercised the repaired prior-help close marker.
4. A third normal invocation reached the real featureless Bash sync child,
   where Broker refused the parent probe with `Bash child scope uncertain`.
   No Bash child effect occurred. The launcher returned failure and its code
   matched the validated failed terminal certificate; submission alone did
   not become success.

The featureless test's launcher is mapped UID 0 inside the disposable user
namespace, though its host user is non-root. It does not reproduce the real
host non-root launcher and host-root Broker UID split. No host-root service or
fixed-path run was performed.

## Verification

- `cargo build --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher -p oulipoly-agent-runner --bin oulipoly-agent-runner --no-default-features` — passed before final changes; final featureless integration test rebuilt the Broker and launcher.
- `CARGO_TARGET_DIR=/tmp/age319-bash-target cargo build --locked --no-default-features --bin agent-bash` from the Bash checkout — passed; no Bash source or trunk target was written.
- `AGE319_FEATURELESS_RUNNER_BIN=... AGE319_FEATURELESS_BASH_BIN=... cargo test --locked -p oulipoly-kernel-broker --test age319_featureless_installed --no-default-features -- --nocapture` — passed, one connected test with the controls and outcomes above.
- `cargo test --locked -p oulipoly-kernel-broker --lib installed_launch_ledger --no-default-features` — passed, five tests.
- `cargo test --locked -p oulipoly-kernel-broker --test age319_connected_control --features age319-private-broker-fixture --no-default-features --no-run` — passed; preserves compilation of the private connected suite.
- `AGE319_CONNECTED_RUNNER_BIN=... cargo test --locked -p oulipoly-kernel-broker --test age319_connected_control --features age319-private-broker-fixture --no-default-features root_mapped_connected_l_reaches_ordinary_help_with_exact_one_use_custody -- --nocapture` — passed, one preexisting private connected case.
- `cargo fmt --all --check` and `git diff --check` — passed.

Existing unused-code warnings remain. A targeted `entry_registry::tests` filter selected zero binary tests and is not counted as behavioral proof.

## Still-closed production seams and next dependency

The v30 Bash lane opens its own `RootRegistry`/`WorkRegistry` and sees a
previously closed root as historical debt. Its `0x90` parent probe therefore
cannot classify a later Bash process in the connected root, even though the
old Broker loop has validated and closed that prior root. The next source
slice must give the fresh lane an independently checked exact closed-history
readback under Broker custody, then prove real featureless Bash sync and async
recipient paths, including busy and sleeping recipient cases, F/ACK, terminal
and physical drain, duplicate/lost reply controls, and restart behavior.

Before any host installation or AGE-353 benchmark, separately prove the real
host non-root caller/root Broker UID boundary, fixed installed images and
socket, and the full L-to-drain chain. The host admission guard stays closed
until those results exist. Parent owns PR, merge, and cleanup.
