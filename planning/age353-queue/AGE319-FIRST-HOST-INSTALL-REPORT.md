# AGE-319 fixed-path first host install — 2026-09-28

Branch: `age319-first-host-install-20260928`, based on Runner `def1d5019390f386378ea56f8d19aa5bc0549f8b`.
Disposition: source and disposable fixture procedure only; no real host install,
State activation, service start, alias publication, or benchmark.

## Delivered

- `packaging/linux/install_first_host_v30.py` consumes the unchanged schema-2
  `first_install_v30.py` package. It checks the archive's four image digests,
  manifest generation and pinned service digest, plus exact equality to this
  checkout's service unit and workspace version. The four image bytes and
  image-derived generation algorithm are unchanged.
- `install` requires root in production, the `oulipoly` group, absent
  `/var/lib/oulipoly-kernel-broker`, absent
  `/usr/local/libexec/oulipoly`, and absent
  `/etc/systemd/system/oulipoly-kernel-broker.service`. It publishes the four
  images as root-owned `0555`, manifest and unit as `0444`, with checked safe
  ancestry, exclusive file creation, file/directory fsync, and no-replace image
  directory rename. `check` reads back exact bytes and modes. A partial
  publication is visible and cannot be silently overwritten.
- `activate` runs the installed Broker offline
  `--bootstrap-empty-v30-state`, then `--activate-first-install-v30`, and
  compares the returned source and pair identities. `start` requires the
  activation record before `systemctl daemon-reload` and `systemctl start`.
  `readback` checks installed package bytes, compares the fixed bootstrap
  marker and activation record, and requires the challenged live Broker socket
  response `entry-gate-v1 fresh-only-open` from a root peer. It reports the
  exact pair and source generations from the records that Broker startup
  validates before opening that route. The Broker's `0x91` pair response
  requires the pinned Runner peer; this Python operator does not impersonate it.
- `install --fixture-root` and `check --fixture-root` map fixed file paths below
  an absolute disposable directory. `activate`, `start`, and `readback` reject
  fixture mode because the Broker's offline commands are hardcoded to the real
  fixed paths. The procedure leaves `/usr/local/bin/{agents,oulipoly-agent-runner,agent-bash}`
  and existing user-local aliases untouched. The current launcher is a Runner
  CLI/GUI handoff, so publishing `agent-bash` through it would be wrong.

## Contracts inspected

- Broker `main.rs` dispatches offline bootstrap and first-install activation
  before serving. `first_install_activation.rs` requires root, an exact empty
  bootstrap, schema-2 pair, trusted executable image paths, and a never-served
  root; retry reads the same record.
- Broker `linux_main.rs` serves only as host root; production uses
  `/run/oulipoly-kernel-broker/control.sock` and
  `/var/lib/oulipoly-kernel-broker`. It validates activation before `b'i'`
  returns `fresh-only-open`. The separate `0x91` pair/source response checks a
  pinned Runner peer. The unit runs as `User=root`, `Group=oulipoly`, creates
  runtime directory mode `0750`, and uses `KillMode=process`.
- Read-only Bash checkout `src/fresh_run.rs` probes
  `/run/oulipoly-kernel-broker/v30.sock` for ordinary fresh child entry; its
  private fixture socket override is user-namespace gated. The installed Bash
  image is distinct from the Runner launcher and was not changed here.

## Commands and observed results

| Command | Result |
|---|---|
| `python3 -m unittest discover -s packaging/linux -p 'test_first_install_v30.py' -v` | 3 passed: original deterministic package, tamper, and inert staging tests. |
| `python3 -m unittest discover -s packaging/linux -p 'test_install_first_host_v30.py' -v` | 3 passed: fixture publication/readback and exact modes, existing destination and changed installed file refusal, wrong unit and tampered package refusal, mocked existing State refusal, and real non-root CLI refusal. |
| `python3 packaging/linux/install_first_host_v30.py install /home/nes/.local/share/age319-first-install-v30-20260928/first-install-v30-inert.tar.gz` | Exit 1: `root required for production first install`; no host path changed. |
| `python3 packaging/linux/first_install_v30.py verify <archive>` | Passed; generation `0e65ae94-9dbd-5b15-9264-66d593ff0086`. |
| `python3 packaging/linux/install_first_host_v30.py install <archive> --fixture-root <tmp>` then `check <archive> --fixture-root <tmp>` | Both passed with the same generation, exact four hashes and unit digest; `stat` showed all four images `555`, manifest/unit `444`. Disposable fixture was removed. |
| `python3 packaging/linux/install_first_host_v30.py activate <archive> --fixture-root <tmp>` | Exit 1: `Broker and systemd operations have no fixture mode`. |
| `git diff --check` | Passed. |

Here `<archive>` is
`/home/nes/.local/share/age319-first-install-v30-20260928/first-install-v30-inert.tar.gz`,
SHA-256 `763ac517e815a1e942239c3b302c57b5abba813e78a5805a8d931bfe576ff791`.
The image hashes are Runner `960f93aa6ef60fc2cb25e072d63d76892518ec91eb4f3d7bb75ac79282282821`,
Broker `07d78506b82a326a66d23b678a4f71788727982193432ae455b6504ee4756204`,
Bash `3bbaeb9b1a10f310123be872a82197509f8ca5056390e8dafd3eb07d9e00e5e9`,
launcher `b12852379b2e2de428cc0f1bd45318edb661b6761b51c7aa299d9127b06dee02`.
The unit digest is `26250bf243387753461b3dcbdab2506d18d8f1656fe8adb58d3e108ec96a2acd`.

## Root operator sequence and remaining barrier

On an intentionally empty host, after inspecting fixed destinations and the
archive, create the required `oulipoly` group if absent, then run:

```bash
getent group oulipoly || sudo groupadd --system oulipoly
sudo python3 packaging/linux/install_first_host_v30.py install <archive>
sudo python3 packaging/linux/install_first_host_v30.py check <archive>
sudo python3 packaging/linux/install_first_host_v30.py activate <archive>
sudo python3 packaging/linux/install_first_host_v30.py start <archive>
sudo python3 packaging/linux/install_first_host_v30.py readback <archive>
```

The remaining barrier is root access to a clean host with safe fixed paths and
the required systemd/group setup. `sudo -n` is unavailable here and this task
forbids touching host `/usr/local`, `/var/lib`, `/run`, systemd, aliases, or old
user State. The fixture therefore proves filesystem behavior only; it does not
prove Broker offline execution, real socket readback, service startup, alias
cutover, production work admission, or benchmarks. User-local aliases currently
shadow system paths. Their inventory and any future publication remain a
separate operator decision. CRW cohort review remains visible debt under the
explicit rapid handoff override; no full review claim is made.
