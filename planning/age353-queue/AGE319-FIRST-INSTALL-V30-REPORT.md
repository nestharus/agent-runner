# AGE-319 fresh-only first-install v30 source — 2026-09-28

## Disposition

This slice provides a **source prerequisite**, not activation. The clean first
install cannot be made soundly from the authorized Runner-only worktree while
Bash at `6b553927982ccb0624de6cad5b6187d5589c8763` is read-only. A
featureless Bash `run` calls its ordinary v30 registration only when
`fresh_run::private_probe_available()` is true (`agent-bash-tool/src/main.rs`
line 287); that predicate requires an isolated private user namespace and
fixture socket (`fresh_run.rs` lines 65–83). Under a host-root installed Broker,
Bash therefore falls through to its old local handle path. Routing it to the
new v30 root by alias alone would risk opening the wrong State.

Two other production gaps independently block activation. The installed
launcher submits `L`, then exits with a gap (`installed_launcher.rs`); the
Broker's production `L` handler explicitly refuses workload admission
(`linux_main.rs` line 5532). Runner's installed-pair entry check requires a
live pair route and rejects `broker-v30-closed` (`kernel_entry.rs` lines
494–558). The shared front door's fresh route also explicitly refuses effects
(`shared_front_door.rs` line 622). A positive selector or service installation
now would be misleading. No selector, installed file/service, alias, old user
State, live DB/config, or AGE-353 stress was changed.

Because the target host has no installed pair and a new empty root is allowed,
the old/new two-island front door is unnecessary for this path. Its old-alias
adoption, old handle settlement, and rollback machinery solve a cutover problem
that this clean first install does not have. This slice instead packages only
the fresh images and keeps them inert until a direct fresh-only launcher and
durable activation decision exist.

## Source change

`packaging/linux/first_install_v30.py` builds a deterministic inert archive
from four supplied images. The inner schema-2 `install-v1.json` pins Runner,
Broker, Bash, and launcher with the same image-derived generation as the
existing paired bundle. The outer manifest pins that pair and the service
unit. The archive has no aliases or activation record; all members are mode
`0400`. `verify` rejects extra, missing, nonregular, writable/executable,
tampered, or noncanonical members. `stage` creates a new non-executable
directory with file and directory fsync and no-replace rename. Its default
requires root-owned safe ancestry; `--fixture` allows rootless disposable
readback. `check` rereads the staged exact bytes and modes. There is no
install or activation subcommand. The existing v1/v2 bundle source was not
changed.

## Frozen images and checks

Evidence: `/home/nes/.local/share/age319-first-install-v30-20260928/`.
`commands.sh` records the exact commands, `logs/` their results, `images/`
the featureless binaries, `first-install-v30-inert.tar.gz` the package, and
`staged/` the disposable inert readback. `SHA256SUMS` covers the frozen
evidence and passes `sha256sum -c SHA256SUMS`.

| Image or package | SHA-256 |
|---|---|
| Runner | `960f93aa6ef60fc2cb25e072d63d76892518ec91eb4f3d7bb75ac79282282821` |
| Broker | `07d78506b82a326a66d23b678a4f71788727982193432ae455b6504ee4756204` |
| Bash | `3bbaeb9b1a10f310123be872a82197509f8ca5056390e8dafd3eb07d9e00e5e9` |
| Installed launcher | `b12852379b2e2de428cc0f1bd45318edb661b6761b51c7aa299d9127b06dee02` |
| Inert archive | `763ac517e815a1e942239c3b302c57b5abba813e78a5805a8d931bfe576ff791` |

Schema-2 generation: `0e65ae94-9dbd-5b15-9264-66d593ff0086`.
Runner base: `6bb23a873918ac4e7278521b6c903c5247529544`.
Bash base and unchanged HEAD: `6b553927982ccb0624de6cad5b6187d5589c8763`.

| Command | Result |
|---|---|
| Runner `CARGO_TARGET_DIR=.../runner-target cargo build --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher -p oulipoly-agent-runner --bin oulipoly-agent-runner` | Pass, featureless; `logs/runner-featureless-build.log` |
| Bash `CARGO_TARGET_DIR=.../bash-target cargo build --locked --bin agent-bash` | Pass, featureless; `logs/bash-featureless-build.log` |
| `cargo test --locked -p oulipoly-kernel-broker --lib installed_pair -- --nocapture` with the Runner target dir | 3 passed; `logs/installed-pair-tests.log` |
| `python3 -m unittest discover -s packaging/linux -p 'test_first_install_v30.py' -v` | 3 passed, including deterministic bytes, tamper and extra-member refusal; `logs/package-unit-tests.log` |
| `python3 packaging/linux/test_paired_bundle.py` | 1 passed; `logs/paired-bundle-regression.log` |
| `first_install_v30.py build`, `verify`, `stage --fixture`, `check --fixture` with the frozen four images | All passed and read back generation `0e65ae94-9dbd-5b15-9264-66d593ff0086`; corresponding `logs/` files |
| Featureless launcher `--model age319-fixture` outside the installed path | Refused with exit 70 before State work; `logs/featureless-launcher-closed.log` |
| `git diff --check` | Pass; `logs/git-diff-check.log` |

The earlier disposable private featureless ordinary Runner/Bash dispatch
remains documented in `AGE319-PRODUCTION-BASH-DISPATCH-REPORT.md`; this slice
did not repeat it or claim it as an installed host-root proof. No positive
featureless end-to-end analog exists through this package because the
production entry paths above are closed. Host-root ownership, service startup,
new-root initialization, live Broker generation readback, and activation
remain untested.

## Required next work and host action

After merging this prerequisite, change Bash source so its featureless
ordinary `run` probes the installed Broker for a released v30 parent before
any local handle/State work. Implement a fresh-only installed launcher to
Broker-owned CLI/root admission, the matching Runner installed pair v30 route,
and a durable explicit activation record that is read back together with the
live Broker generation. Preserve the existing exact-image and connected-peer
checks and no-replay semantics. Give unsupported GUI, PTY, and legacy modes
clear refusals. Then test the complete featureless pair in a disposable
private root before proposing a host install.

The host action **after those source changes merge** needs root authority:
create a new empty root-owned v30 State directory, install the exact paired
images and unit to their fixed root-owned paths, verify the live Broker
generation against the explicit activation decision, and only then publish
the named `agents`, `oulipoly-agent-runner`, and `agent-bash` aliases. A
host-root CLI plus attached Bash end-to-end test must precede AGE-353
benchmarking. Nothing in this report authorizes running the current inert
archive as an installed service or changing the old user State.
