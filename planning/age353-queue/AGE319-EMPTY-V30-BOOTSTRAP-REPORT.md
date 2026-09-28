# AGE-319 empty v30 first-install State bootstrap — 2026-09-28

## Disposition

This Runner-only slice adds an explicit **offline, empty first-install State
bootstrap**. It does not install images, publish aliases, start a service, or
open/migrate user-local State. The production command is
`oulipoly-kernel-broker --bootstrap-empty-v30-state`; its fixed destination is
`/var/lib/oulipoly-kernel-broker`. It requires root and trusted root-owned
ancestry. No host-root command was run here.

The command creates a private sibling stage, initializes the current StateDb
schema, an independent completion domain and activated Broker sidecar, and a
fresh v30 lane. Both sidecars bind their exact State file inode at its final
published path. A root-only marker pins the source generation, domain, State
inode and lane identity. Files and directories are hardened and fsynced before
the complete root is published with a no-replace rename; the parent is fsynced
afterward. A per-root lock serializes initializers. A retry reads back the same
identity; an incompatible existing root or abandoned stage refuses instead of
minting another published generation.

Broker startup validates the marker when present before accepting connections,
then opens the same Broker sidecar generation. The root registry admits only
the exact new State files and marker, with root-only ownership/mode checks.
If the top-level State file survives without its marker, startup refuses;
older cutover roots keep State outside this directory.
The ordinary route selection already returns `StateRoute::BrokerOwned` from
that opened sidecar; installed caller admission remains closed by the earlier
AGE-319 gates.

## Verification

Results and exact commands are frozen under
`/home/nes/.local/share/age319-empty-v30-bootstrap-20260928/`.

| Check | Result |
|---|---|
| `cargo test --locked -p oulipoly-state --features age319-private-broker-fixture --test age319_empty_v30_bootstrap -- --nocapture` | Pass: root-mapped exact bootstrap/retry, State and sidecar conflict, widened mode, missing binding/marker, incompatible root and abandoned stage |
| `cargo test --locked -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_empty_v30_startup -- --nocapture` | Pass: two Broker restarts reopen the published generation and report `BrokerV30Closed`; corrupt and absent markers prevent startup |
| Featureless `cargo build --locked` for Runner, Broker and launcher; Bash `cargo build --locked --bin agent-bash` with external target | Pass |
| `first_install_v30.py build`, `verify`, `stage --fixture`, `check --fixture` | Pass: exact staged readback of generation `67087dc5-08a6-5e1c-b060-6fa66756f0d6` |
| Package unit tests | Pass: 3 tests |
| `cargo fmt --all -- --check`, `git diff --check` | Pass |
| `SHA256SUMS` | Final readback is recorded with the frozen artifacts |

An additional existing `age319_fresh_dual_lane` regression test fails at line 130:
it expects sidecar `PRAGMA user_version=32` while the base source already sets
`BROKER_OWNED_VERSION=38`. That assertion is unchanged by this slice. The new
bootstrap test in the same run passed. The pre-existing version mismatch is
retained as an explicit test gap rather than altering an unrelated fixture.

The featureless Broker command was also invoked as the unprivileged host user;
it refused before any State access with `fresh v30 lane requires root`.

Base Runner HEAD: `6261800d1590e176de146033cc6203d1b8344000`.
Final Runner branch HEAD is recorded in the frozen `heads.txt` after commit.
Bash read-only HEAD:
`6c0a83baa8e6cfcb518c31584da0414265bf1df2` (clean after build).

Frozen image SHA-256 digests:

| Image | SHA-256 |
|---|---|
| Runner | `6c0dbec2edd1b8eb00d12bbcbced1abd187d586bb54db8bc319bfb74356a266a` |
| Broker | `58298ecd9c37afbd8bedcf4530772db79ac5d868511f11726aac8da8ef0d87bd` |
| Installed launcher | `2eedcd10ae062543d0079e5306f15a31d60966fe8a89a6295396191db5954458` |
| Bash | `741a1975fbe7fa666aaac5195cc81697ae626df4bfe5a4899b54a8b7decdc54e` |

The inert package generation is
`67087dc5-08a6-5e1c-b060-6fa66756f0d6`; archive SHA-256 is
`2ad3a14db1a33420b58400337401c3cf8b2353fb89d6dcd91d5b00ee35f9b16a`.

## Limits and next boundary

The command is offline and has not run against the host's fixed root path. A
root-mapped namespace proves the storage and Broker reopen behavior but not
host-root ownership, systemd startup, or an installed Runner `I` request. The
installed launcher, Runner installed-pair gate, Bash
ordinary production v30 dispatch, and activation decision remain closed.
The next slice must implement those admission paths and a fresh-only activation
record with live generation readback, then test the complete featureless pair
in a disposable private root before proposing a host install. No AGE-353
stress or live host action is authorized by this source slice.
