# AGE319 fixed-path completion-driver correction

The owned driver now reaches dispatch at the literal installed Runner path in
the disposable oracle. Markerless and double-marker public entry still refuse.
This is source construction evidence for root's subsequent Observe/Frame/Decide,
not delivery admission, host certification, or restoration.

Base: `04aab77798ed2e93c723a5169da3fad3053aebb9`; owned branch:
`age319-fixed-path-driver-20260929`. Shared instructions read at AI HEAD
`cb7a748a87174f0ae1aa793db479d89be18565e5`. The supplied Decision selects a
bounded correction because the guardian's markerless self-exec was rejected by
the public installed-ingress check. It leaves host ENOENT and deployment open.

## Construction witness

- `main.rs::main` still checks installed ingress before dispatch.
- `kernel_entry.rs::verify_installed_entry_route` still verifies the fixed
  image, immutable pair and activated Broker route. For schema 2 with neither
  public entry marker, it now asks the completion owner to verify internal
  driver custody. Double-marker entry never enters that alternative.
- `completion_owner/driver.rs::verify_installed_entry` treats internal argv and
  the FD number as locators. It duplicates the descriptor, requires the socket
  peer to be the live same-UID parent with the same Runner executable, then
  reads the existing guardian owner frame. It compares driver/guardian
  incarnations and requires `V30OwnerRoute::read_running` from Broker. Broker's
  existing `read_broker_state`/`state_actor_matches` authenticate the exact
  driver, guardian lineage, owner, image and source; no new Broker operation or
  public bypass was added.
- The verified frame is retained in-process for `driver::entry`, avoiding a
  second read. The original channel remains available for completion custody.
  `linux.rs::start_v30_driver` and guardian preparation/release are unchanged:
  the self-exec stays alive on its held channel until the guardian publishes
  the released owner, and EOF refuses. The legacy/temporary-path route keeps
  its existing frame read.

## Fixed-path oracle and results

`crates/oulipoly-kernel-broker/tests/age319_fixed_path_driver.rs` enters
`unshare -Urpfm --mount-proc`, copies the four featureless images into a
temporary root, and chroots there. Libraries and proc are bound only in the private mount
namespace. Runner, manifest, Broker, launcher, socket and State use their
literal installed paths inside that root. Host installed paths/State are never
written. Parent-owned temporary cleanup follows the namespace's native exit.

The oracle observes `/proc/<driver>/exe` as
`/usr/local/libexec/oulipoly/oulipoly-agent-runner` and verifies that neither
host-entry nor joined-child marker is present. Thus the actual gate predicate
is true solely because of the fixed path. It checks the observed driver PID
against Broker's sealed `prepared_driver`, requires an unprefixed driver
dispatch result, help output and durable terminal exit 0. Public markerless
and double-marker entries must emit the original gate refusal. An outside
caller supplying driver argv and a same-UID inherited socket must also refuse
at installed ingress.

Run from this worktree, using these inputs:

```bash
export CARGO_TARGET_DIR=/home/nes/.cache/age319-featureless-release/runner
export AGE319_FEATURELESS_RUNNER_BIN="$CARGO_TARGET_DIR/release/oulipoly-agent-runner"
export AGE319_FEATURELESS_BASH_BIN=/home/nes/.local/share/age319-first-install-v30-20260929-04aab777/images/agent-bash
cargo build --release --locked --no-default-features -p oulipoly-agent-runner -p oulipoly-kernel-broker --bin oulipoly-agent-runner --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher
cargo test --release --locked --no-default-features -p oulipoly-kernel-broker --test age319_fixed_path_driver -- --exact disposable_fixed_path_driver_admission_and_public_refusal --nocapture
cargo fmt --all --check
git diff --check
```

The pre-fix cached Runner (SHA-256
`ac0b296e2487e7d7202c8a4a2373878395877cc64202b591851418649bd29ffa`)
failed the fixed-path owned launch: launcher exit 70, original entry refusal,
then Broker `dead peer`/owner-absent readback. Both public controls refused.
Three earlier fixture setup failures are separately preserved (nonrecursive
locked-library bind EINVAL, then missing fixture State inputs).

Candidate release build: exit 0. Candidate oracle: 1 selected test passed;
the final run includes the dispatch assertion, actual fixed-path observation,
sealed PID equality and all three public refusal controls. Both candidate runs
passed; the final native test command exited 0. Formatting/diff checks pass.
Cargo fingerprints record `features: []` for Runner, Broker and launcher.
The supplied Bash hash was read back as
`e2793edef2a47cae9296a0d26eaf187bb01515d78e6a45dd361da42526d14dbc`.

Complete stdout/stderr, supplied host stderr (clearly labeled), commands,
feature readback, built-image hashes and delivery identities are preserved in
`/home/nes/projects/agent-runner/planning/context-routing-20260929/driver-maker-logs`.
The exact signed commit/tree are returned in the native final and
`delivery.txt`, avoiding a self-referential commit identity here.

## Uncertainty and root-owned next work

The final isolated run retains unprefixed driver `error dead peer` after
admission/dispatch and Broker recorder diagnostics about missing
`OULIPOLY_DATA_DIR`, despite help and durable terminal success. Their subsequent
causal stage is not diagnosed or corrected here; root receives them as current
observations. The test's dispatch-result assertion accepts the existing empty
work refusal or late dead-peer result, not a sustained driver-health claim.

Host ENOENT remains syscall/path/stage ambiguous and is not claimed fixed. No
live host run, automatic host retry, install/reset/replay, service restart,
provider/account edit or deployment occurred. The disposable topology maps one
host non-root UID to namespace UID 0; it does not prove actual host UID
separation between non-root launcher and root Broker. Root retains the one
host rerun, source consequence observation/admission, later deployment and
closure of the local/remote branch and worktree after merge/evidence retention.
No Core/full CRW nine-family/benchmark claim is made. Indefinite-pending debt
remains untriggered with its existing owner. All Maker native lanes exited.
