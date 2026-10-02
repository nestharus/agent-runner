# AGE-319 paired Linux first host install

## Broker descriptor capacity

The packaged unit sets `LimitNOFILE=65536:1048576`: soft **65,536**, hard
**1,048,576**. AGE-380's failed flat 40 run used an unchosen soft default of
1,024. This capacity choice addresses that ceiling; completed-root descriptor
release and truthful observation-error classification are separate corrections.

Sizing follows the installed source path. `installed_launcher.rs` submits `L`
even when invoked inside a parent producer. In `linux_main.rs`, `L` reserves a
new root through `InstalledLaunchLedger::reserve_request` and spawns a
`ControlGrant` in the Broker's host namespace. That control enters the ordinary
`E`/`j` path. Each producer using this entry therefore creates its own root,
including a child producer launched by a parent; it does not reuse the parent's
root. `HeldRootJoin` retains a gate and four `PinnedProcess` objects;
`RootRegistry` retains a fifth pin. Each pin owns a pidfd and a namespace file:
**11 FDs per root**. The connected-control channel adds one; the namespace
helper reaper holds one pidfd while its helper lives. This gives a **13-FD
root/control/helper subtotal** per producer, before variable work and I/O.

Bash `C`/`K` work is a distinct nested-work path. `fresh_provider.rs` preserves
the causal parent namespace, and `work_launch.rs` inserts a `LiveWork` under
the existing root. Its retained PID1 pin costs two FDs, with another helper
pidfd while live. Its launch control/gate and worker pins are local to the
launch. `SourcePhysicalRegistry` retains process stamps rather than pins.
Normal provider K also launches its supervisor/PID1 below the root and stores
physical records; it does not create another `HeldRootJoin`. These costs must
not be counted as another 11-FD root merely because work is nested.

The capacity budget allows **32 FDs per producer**, plus **4,096** for shared
files, SQLite connections, concurrent requests, and launch/observation
temporaries. The extra 19 per producer above the 13-FD subtotal allows nested
work, sleeping-successor channels/pins, and per-producer I/O. These are chosen
allowances, not measured peaks or proven upper bounds on arbitrary workloads.

| Separate-mode geometry | Producers | Root/control/helper subtotal (13 each) | Budget (32 each + 4,096) | Spare under 65,536 |
|---|---:|---:|---:|---:|
| 10 parents + 100 children | 110 | 1,430 | 7,616 | 57,920 |
| 20 parents + 400 children | 420 | 5,460 | 17,536 | 48,000 |
| 32 parents + 1,024 children | 1,056 | 13,728 | 37,888 | 27,648 |

At the largest cohort, spare capacity is about **73% of the budget**. This is
source sizing for one cohort in each mode, not acceptance of any rung. Existing
lifetime root retention still accumulates across completed cohorts; increasing
capacity cannot make rolling churn finite. Variable SQLite retention and peak
transients remain for source attribution and later runtime measurement.

**Resource environment:** Linux inherits resource limits across fork/clone and
exec. The Broker's control Runner, root helper/PID1, Runner child, provider and
Bash descendants inherit the new soft limit and the hard limit; the production
launch paths do not reset NOFILE. This deliberately raises their soft ceiling
too. It reserves no descriptors and adds no workload quota, sandbox, timeout,
or admission limit. Workloads retain ordinary host resource behavior and can
adjust their soft limit up to the hard ceiling. A launcher already running in
the caller's shell retains that shell's limits; its Broker-spawned control
inherits the service's limits. No caller-limit restoration is added here.

After installation, read the configured service limits without a census:

```bash
systemctl show oulipoly-kernel-broker.service -p LimitNOFILESoft -p LimitNOFILE
```

Also read the running Main's effective kernel limits (unit overrides or
process changes can differ from the packaged source):

```bash
broker_main_pid=$(systemctl show oulipoly-kernel-broker.service -p MainPID --value)
awk '$1 == "Max" && $2 == "open" && $3 == "files" { print }' "/proc/$broker_main_pid/limits"
```

Expected soft/hard values are 65,536 / 1,048,576. This readback and the failed
flat-40 rerun belong to verification after all three AGE-380 corrections;
neither is established by the source choice or packaging tests.

## Fresh-only first-install image package

`first_install_v30.py` builds an inert archive from **four explicit images**:
featureless Runner, Broker, Bash, and installed launcher. Its schema-2
`install-v1.json` uses the same image-derived generation as the paired bundle;
the outer manifest also pins the service unit. The archive contains no aliases,
selector, or State database. Its members remain mode `0400`. `verify` checks the
exact member set, image digests, service digest, manifest, and generation.
`stage` and `check` retain the original inert staging interface.

```bash
python3 packaging/linux/first_install_v30.py build \
  --runner /path/to/oulipoly-agent-runner \
  --broker /path/to/oulipoly-kernel-broker \
  --bash /path/to/agent-bash \
  --launcher /path/to/oulipoly-installed-launcher \
  --output /path/to/first-install-v30-inert.tar.gz
python3 packaging/linux/first_install_v30.py verify /path/to/first-install-v30-inert.tar.gz
python3 packaging/linux/first_install_v30.py stage \
  /path/to/first-install-v30-inert.tar.gz /root/first-install-v30-stage
python3 packaging/linux/first_install_v30.py check /root/first-install-v30-stage
```

`install_first_host_v30.py` is the fixed-path first host procedure. Run it from
this source checkout with the **featureless** four-image package. It requires
root and an existing `oulipoly` group. `install` refuses any existing Broker
State root, image directory, or service unit. It publishes the exact four image
bytes at `/usr/local/libexec/oulipoly` as root-owned mode `0555`, the schema-2
manifest as `0444`, and the exact unit at
`/etc/systemd/system/oulipoly-kernel-broker.service` as `0444`. An interrupted
install is visible to `check`; do not overwrite an existing destination.
The package's service bytes must equal this checkout's service source.

```bash
# On the intended clean host, inspect existing paths and group first.
getent group oulipoly || sudo groupadd --system oulipoly
sudo python3 packaging/linux/install_first_host_v30.py install /path/to/first-install-v30-inert.tar.gz
sudo python3 packaging/linux/install_first_host_v30.py check /path/to/first-install-v30-inert.tar.gz
sudo python3 packaging/linux/install_first_host_v30.py activate /path/to/first-install-v30-inert.tar.gz
sudo python3 packaging/linux/install_first_host_v30.py start /path/to/first-install-v30-inert.tar.gz
sudo python3 packaging/linux/install_first_host_v30.py readback /path/to/first-install-v30-inert.tar.gz
```

`activate` runs the **installed** Broker's `--bootstrap-empty-v30-state` and
`--activate-first-install-v30` offline, in that order. It compares their exact
source and pair identities. `start` reloads systemd, starts the installed unit,
then queries the Broker's existing challenged `entry-gate-v1` socket operation.
The procedure requires the live response `entry-gate-v1 fresh-only-open` and
reads the exact pair generation from the installed manifest and source
generation from the bootstrap/activation records. Broker startup validates
that exact binding before it serves the open route. The separate installed-pair
socket operation requires a pinned Runner peer and cannot be called by this
Python operator tool. `start` does not enable the
unit for boot. `activate` may be retried after a lost reply against the same
marked bootstrap root; a previous service start or unmarked State refuses.

For a disposable filesystem proof, pass `--fixture-root /absolute/disposable/root`
to `install` and `check`. They map fixed paths underneath that root and do not
run Broker or systemd. Broker offline commands use hardcoded `/usr/local` and
`/var/lib` paths and have no fixture override here. Fixture proof is not a host
activation or benchmark.

Public alias publication is separate from this procedure. It does not create
or replace `/usr/local/bin/agents`, `/usr/local/bin/oulipoly-agent-runner`, or
`/usr/local/bin/agent-bash`. The first two would enter the installed launcher;
the current launcher captures a Runner CLI/GUI handoff. The Bash path is a
distinct image and must not be pointed at the Runner launcher. Existing
`~/.local/bin/{agents,oulipoly-agent-runner,agent-bash}` currently shadow
system paths on this host and remain untouched. No production alias or
workload admission is claimed by the service readback.

## Preserved reinstall over the served AGE-319 first host

`reinstall_preserved_v30.py` is a **single-campaign** operator. It is for the one
host where this first install already served the old pair. It is not a
general upgrade path.

Keeping the old State in place is not possible. Its activation record pins
every old image by SHA-256, device and inode, so the Broker refuses to start
with it once the images change. Activation also refuses any root that has
already served. So `preserve` moves the old install aside instead:

1. It checks the old install is exactly as expected:
   - the images and unit match the old package byte for byte;
   - the State bootstrap/activation source is `--source-generation`, bound to
     the old pair and to this `state.db` file's device/inode;
   - no stage from an interrupted install, bootstrap or activation is present;
   - everything is on the same filesystem.
2. It takes a census of actors. An actor is any process that is running one
   of the installed images, is in the unit cgroup, or holds a path under
   State, `/run/oulipoly-kernel-broker` or the preservation root. The only
   actor allowed is the running service `MainPID`, which must equal
   `--broker-pid` and run the installed Broker image. Any other actor, or any
   process that can't be read, refuses before anything changes. Nothing is
   signalled by name or by PID.
3. It writes `pre-stop-v1.json` to the root-only
   `/var/lib/oulipoly-age319-preserved-<old generation>` directory, then stops
   only the unit.
4. It confirms the Broker has drained. The unit must be inactive with no
   `MainPID`, the old Broker incarnation must be gone, and the census must
   find no actors and no unreadable processes. `KillMode=process` makes the
   stop alone no proof of this.
5. It records the exact quiescent tree of State, images and unit in
   `baseline-v1.json`.
6. It renames State, then images, then unit into that directory with
   `RENAME_NOREPLACE`. It rechecks identity and drain before each rename and
   compares the result with the baseline afterwards.

Nothing is deleted, rewritten, replayed or reset. A refusal keeps every change
already made, including the stop; stderr lists them. A rerun continues from the
records and refuses anything that changed. `status` is read-only. `restore`
renames the preserved install back only while the original paths are still
free, and leaves the service stopped. Initial `status` succeeds only with
`readiness: "ready"` and `verdict: "ready"`. After preservation, it compares all
three old objects with the quiescent baseline at their preserved paths, even
when the new install also occupies the original paths. It succeeds with
`verdict: "preserved"` and `preservation_verdict: "preserved old State, images
and unit match baseline"` only when all three remain preserved and match.
Refused readiness, incomplete preservation, or a baseline mismatch exits
nonzero. Locations, comparison paths and per-item matches remain in the JSON
for diagnosing a refusal; a partial move is not a success verdict.

After `preserve`, the unchanged first-install procedure installs the new pair.
It bootstraps a **new empty** State. The old served State stays preserved and
is never offered to activation.

The root invocation for this host follows. Run it from a checkout containing
this file.

```bash
OLD=/home/nes/.local/share/age319-first-install-v30-20260929-04aab777/first-install-v30-main-04aab777-release.tar.gz
NEW=/home/nes/.local/share/age319-fixed-path-release-20260929-ba6f537a/first-install-v30-release.tar.gz
EXPECT="--old-generation 2ca23e6d-fe12-5b8e-9656-970801e22377 --new-generation 4e0edeee-816a-5ff7-aa29-930a974adcd7 --source-generation 5ba95834-a10b-441f-a5fe-0f6f348f7029 --broker-pid 768990"
sha256sum "$OLD" "$NEW"   # 045613d2...79af9e0 and 8367b3ea...98fc37d9
sudo python3 packaging/linux/reinstall_preserved_v30.py status "$OLD" "$NEW" $EXPECT    # readiness must be "ready"
sudo python3 packaging/linux/reinstall_preserved_v30.py preserve "$OLD" "$NEW" $EXPECT
sudo python3 packaging/linux/install_first_host_v30.py install "$NEW"
sudo python3 packaging/linux/install_first_host_v30.py check "$NEW"
sudo python3 packaging/linux/install_first_host_v30.py activate "$NEW"
sudo python3 packaging/linux/install_first_host_v30.py start "$NEW"
sudo python3 packaging/linux/install_first_host_v30.py readback "$NEW"  # pair_generation 4e0edeee-...
sudo python3 packaging/linux/reinstall_preserved_v30.py status "$OLD" "$NEW" $EXPECT    # verdict "preserved"; all three matches_baseline true
```

Before `status`, both SHA-256 values must match the recorded package hashes:
`045613d20b1a603070c2c081b0c73966e0a4ec8361b9a2179663b392a79af9e0` (old) and
`8367b3ea0f58c075915d23bad2f149e5c2076e169e265ea33957e26698fc37d9` (new).
The final reinstall `status` establishes preservation against the captured
baseline; the preceding installer `readback` establishes the new live pair.

Stop at the first nonzero exit and keep its output. Do not use `install` or
`activate` to repair a partial `preserve`. The fixture tests simulate the
service manager and only list the test's own child processes. They prove
operator control flow, not the host service, the host census or the UID split.

The featureless Broker also has an explicit offline storage command:
`oulipoly-kernel-broker --bootstrap-empty-v30-state`. As root, it publishes
`/var/lib/oulipoly-kernel-broker` only from an absent path. The publication
contains a current empty State database, completion-domain sidecar, fresh v30
lane, exact inode-bound State source bindings, and an identity marker. A retry
reads back that same identity; an incompatible root or abandoned stage is a
refusal. This command does not start the Broker service or activate the image
package.

With the final files in place, the procedure runs the installed Broker as root
with `--activate-first-install-v30` before its first service start. It accepts
only
the exact unserved empty bootstrap root. It pins the manifest and all four
named images by SHA-256, device and inode, and atomically publishes
`/var/lib/oulipoly-kernel-broker/first-install-activation-v1.json`. A retry
prints the identical source and pair identity. An old root, incomplete stage,
changed file, or already served root refuses. On startup the Broker validates
that record before reporting `fresh-only-open`; its installed-pair observation
includes both pair generation and bootstrap source generation. No startup path
creates the activation record. Production L, launcher result/status, and
Runner fresh-only execution remain closed.

## Separate old/new staging fixture

`stage_versioned_island.py` accepts ten explicit source paths: retained legacy
Runner/Bash images and adjacent configs, plus fresh Runner/Bash/broker/launcher
images and adjacent configs. It copies them into separate directories in one
new generation, records exact SHA-256/size/device/inode, syncs every file and
directory, renames the complete generation, syncs its parent, and verifies the
readback. It rejects an existing generation, mixed old/fresh State roots,
missing or changed images, and a fresh Bash config that does not point to the
staged fresh Runner. Every copied image is mode `0400` and cannot be executed
from the staging directory. It never modifies the old named image, config, State,
sidecar, WAL, handle, or installed service. `--require-root` requires a
root-owned, non-writable destination ancestry for installed staging.

The artifact is **inert**: its manifest says `admission=closed` and
`activation=inert-no-selector`; the script writes no `active-v2.json`, v1
manifest, public alias, service, or State database. Its fresh Bash image should
be built with Bash's `age319-closed-fresh` feature, which refuses every entry
before any helper or State effect. The fresh Runner has the matching
`age319-closed-fresh` feature, which refuses before its existing entry gate and
all CLI, GUI, maintenance, or State work. Staging alone does not attest those features
or prove new root admission. The old v1 `InstalledPair` cannot consume the
proposed v2 active record, and there is no installed common launcher for Runner
and Bash. Never place an active-v2 selector beside the v1 manifest or treat
this staging directory as an activated package.

On this joined source, the default Runner binary retains the broker-gated
offline root entry; `age319-closed-fresh` is a separate build feature. Pass an
explicit closed-feature image as `fresh_runner` when preparing v2 staging.
`stage_versioned_island.py` records exact bytes but does not attest Cargo
features. `build_paired_bundle.py` retains the fixed schema-v1 bundle when
no Bash image is supplied; the current Linux release job uses that path.
An explicit `--bash` selects a separate schema-2 image bundle. Neither path
publishes or installs this v2 root. Do not reuse a
closed-feature Runner image as that bundle's old Runner image, or overwrite a
historical pinned v1 image during v2 preparation.

## Shared front-door source candidate (not installed)

`oulipoly-shared-front-door` is a separate Linux image. The installer helper
`shared_front_door.py` prepares a new `front-door-v2` root, a stable launcher
image, an immutable generation manifest, and non-executable fresh Runner/Bash
copies. It leaves the fixed v1 pair, its manifest/socket, original old Runner
and Bash/config paths, and all old State/WAL/sidecars untouched. The root must
be a child separate from the v1 manifest directory; the active file is named
`front-door-v2/active.json`, never `active-v2.json` beside `install-v1.json`.

The installer has explicit `prepare`, `check`, `publish`, `adopt`, and `read`
commands. Its CLI requires a root-owned control root, stage, launcher and broker
unless `--fixture` is supplied. Exact inventoried public symlinks may instead
live in a single user's `~/.local/bin`: their link owner must match their parent,
and every ancestor must be owned by root or that user and be free of group/other
write permissions. Their absolute link text, resolved old target and owner are
recorded before adoption; the root-owned manifest remains the source of truth.
Prepare checks all ten staged assets, the original adjacent old configs, and
the exact broker image. It does not change a public alias. Publish initializes
epoch zero as `legacy`; only then may an administrator adopt the named
aliases (`agents`, `oulipoly-agent-runner`, and `agent-bash`, plus
`oulipoly-plane` when an actual GUI entry exists)
one at a time. The exclusive publication lock and launcher shared lock
serialize the selector read through any old-image `exec`. A later
`fresh-closed` publication requires all aliases and the live root broker route
and image to match the generation. It uses a staged file, file and directory
fsync, atomic replacement, exact compare-and-swap digest, publication UUID,
and readback. A lost reply can be read back or retried with the same UUID.
Rollback to legacy or a second activation is refused. Production preparation
must declare the adapter disabled or bind both of its effective entries to the
inventoried Runner and Bash alias paths. An environment override to another
path remains outside this managed route. `check` reads every inventoried alias
back and reports `legacy`, `adopted` or `absent`; activation requires all present
aliases adopted. No command here has been run on installed paths.

The launcher recognizes the kernel `AT_EXECFN` of the exact Runner CLI, GUI,
or Bash alias. Before it touches old handle metadata or launches anything, it
reads the one active selector and verifies its own image, generation manifest,
old/fresh images and configs, and named broker image. Epoch zero calls each
alias's recorded old target image, so adoption before activation can create
late old debt.
After `fresh-closed`, Runner CLI/GUI and new Bash runs refuse. An exact old
Bash handle with matching old metadata may call old Bash for settlement;
unknown/ambiguous handles refuse. The fresh route reads the live broker's
generation and process image before refusing. The broker permits the launcher's
payload-free `I` route observation from a different image; fresh State
reservation and effect operations still require the pinned Runner image and
their physical checks. No fresh effect is opened by
this source. Direct/cached old binaries and adapter overrides that bypass the
named aliases are outside this front door and remain old-island debt.

The repository's `.desktop` archive entry names `/usr/local/bin/oulipoly-plane`,
but this host currently has no installed GUI alias. The installed OpenCode
adapter defaults to `~/.local/bin/agent-bash` and `~/.local/bin/agents`, with
`AGENT_BASH_BIN` and `AGENT_BASH_AGENT_RUNNER_BIN` overrides. On this host
`opencode.json` sets `tools.bash=true`, but the global custom tool file is named
`bash.ts.disabled`; the effective runtime adapter therefore needs a separate
inventory before declaring it bound or disabled. `PATH` lookup may resolve to the
user-local aliases; the launcher uses kernel `AT_EXECFN`, including relative
entries from the current directory, and accepts only exact inventoried names.
Direct launcher paths, alternate `PATH` hits, override paths, raw cached binaries
and deliberately changed local links are outside universal managed routing.
The fixed v1
Runner launcher and old Bash absolute helper cannot be replaced while pinned
old actors still need their images and paths. Fresh binaries stay mode `0444`
in this candidate; the copied fresh Bash config still points to the inert
stage. A later positive route needs a new compatible installation and separate
root/child/source/result proof, including host `sudo`/setuid/native access.

`build_paired_bundle.py` stages one versioned archive with the fixed Runner,
broker and thin launcher images, optional Bash image, `install-v1.json`, broker service unit, `agents` and
`oulipoly-agent-runner` CLI links, and an `oulipoly-plane` GUI link and desktop
entry. The archive is inert: building or extracting it does not enable the
service or activate v30 State. The existing Tauri `.deb` and raw Runner release
assets continue to use their legacy paths and are **not** a paired deployment.

The manifest contains the workspace package version and exact image digests.
Schema 1 derives its generation from Runner, broker and launcher; schema 2
also includes Bash. The production broker
checks root ownership, path safety, all named image digests, its running image,
and the manifest before
binding `/run/oulipoly-kernel-broker/control.sock`. A Runner started from the
fixed `/usr/local/libexec/oulipoly/oulipoly-agent-runner` path checks its running
image and manifest, then asks the live broker
for that same generation and an open legacy entry route before any State work.
The `agents`, `oulipoly-agent-runner`, and `oulipoly-plane` links now enter the
exact installed launcher. It checks its own image and manifest, captures argv,
environment, stdio/TTY and cwd as bytes and descriptors, then submits a
challenged `oulipoly-installed-launch/v1` request. The broker pins that peer to
the launcher image, checks generation and descriptors, and refuses before
execution while the full supervisor and State routing are unfinished. A lost
reply is an error with no automatic retry. Direct invocation of the fixed
Runner also refuses unless it has one of the existing broker-owned entry
paths. An old or replaced image, absent or old broker, changed image, draining
gate, or v30 route refuses.

## Cutover requirements still open

The first-install procedure establishes an empty fresh-only Broker route, not
a production workload cutover. The service currently owns only the broker and
uses `KillMode=process`; it cannot
inventory or stop CLI/GUI/helper descendants. WSL here has no usable writable
unified cgroup-v2 subtree for that purpose. The broker currently admits only
help/offline diagnostic host entry and lacks a normal GUI/PTY or provider-work
custody path. The staged launcher does not start any workload. Terminal signal
and window-size relay, GUI display/socket lifecycle, lost-reply readback,
restart recovery, and cancellation still need a single broker-owned root/PID1
tree with exact physical drain. Standalone `agent-store`, `agent-scratchpad`, and
`agent-messenger` assets have no paired ingress contract. Existing `.deb`,
user-local links, historical binaries and already-running State writers can
still address the retired user State path. A manifest or process-name scan
cannot make them join one supervision boundary. No process is signalled by
this artifact.

An eventual administrator cutover must first hold persistent ingress across
service restart, stop new launches through every supported entry, and obtain
an exact owned process tree for every admitted CLI, GUI and helper. Stop new
entry before stopping the supervisor; join its known processes and descendants
by pinned incarnation, and treat any unowned or uncertain writer as a blocker.
Only after a held sidecar fence, root-observed main/WAL/SHM census, and
recheck through offline publication may root-only State migration proceed.
Rollback before publication may reopen the old gate only after the new
supervisor is stopped and known children are joined. After publication, do not
reopen legacy writers or roll back to old State. Never kill an arbitrary
same-UID or process-name match. Host `sudo`/setuid behavior inside workloads
must remain unrestricted.

The manifest check is an image compatibility prerequisite only. It does not
prove a global writer census, persistent ingress, process custody, State v30
routing, or delivery readiness.

When supplied, the fixed Bash image and digest are staged for future child admission. The
ordinary Bash `run` command still refuses a v30 owner before local handle or
workload creation. The private Bash child fixture is compiled under a separate
feature and is not part of this package.

Bundles with Bash use manifest schema 2 and a distinct `oulipoly-pair-v2`
generation derived from all four image digests. The reader still accepts
retained schema 1 manifests with no Bash field. A schema 1 bundle cannot admit
a Bash child; adding Bash to an old schema 1 manifest is invalid.
