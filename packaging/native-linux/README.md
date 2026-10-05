# Native-root package (Linux x86_64)

A self-contained package of the native ACP v2 root (`native-root`, the
per-root owner, root PID 1, agent-bash and the locked OpenCode
dependencies) with one bounded privileged entry, the **front door**, and
one programmatic **caller** for an unprivileged requester. The work runs as
the requester with its normal host rights; the owner and root PID 1 run as
root (`host-root`). The package is separate from the old installed
runner/Broker domain (`packaging/linux`, `/usr/local/libexec/oulipoly`)
and does not depend on it.

Nothing here selects a model route for an existing workflow. A caller uses
it explicitly.

## Layout (the asset contract)

Everything is found relative to the package directory. No build or
worktree path is compiled in.

```
<package>/
  bin/oulipoly-agent-runner        runner (its `native-root` subcommand)
  bin/oulipoly-root-supervisor     per-root owner (found next to the runner)
  bin/oulipoly-root-pid1           root PID 1 (found next to the owner)
  bin/oulipoly-native-call         the caller (unprivileged)
  libexec/oulipoly-native-frontdoor  the front door (root, via sudo)
  agent-bash/agent-bash            agent-bash binary
  agent-bash/bash.ts               its matching OpenCode tool
  opencode/deps/                   `npm ci --ignore-scripts` of the owner
                                   crate's native/opencode lockfile
  share/install_package.py         installer and uninstaller
  share/sudoers.template           the one sudoers rule
  share/frontdoor.example.json     site config example
  MANIFEST.json                    source identities, toolchain, dynamic
                                   libraries, sha256 and mode of every file
```

Installed root-owned at `/opt/oulipoly-native/<id>/`. The id is
`oulipoly-native-linux-x86_64-<runner commit 12>-<agent-bash commit 12>`,
with `-dirty` when built from uncommitted sources. Site config lives at
`/etc/oulipoly-native/frontdoor.json` and the run base at
`/var/lib/oulipoly-native/runs`.

## Build (as the user, in a build directory)

```
python3 packaging/native-linux/build_package.py --build-dir B \
  --runner-repo <runner checkout> \
  --agent-bash-repo <agent-bash checkout> --agent-bash-commit <commit>
```

The builder writes only under `B`: cargo home and targets, the npm cache,
dependencies, the stage and `dist/`. It reads the agent-bash checkout only
through `git archive`. Cargo (`--locked`) and `npm ci` fetch locked public
dependencies. The runner is built with default features (never
`age319-closed-fresh`). The output is a deterministic
`B/dist/<id>.tar.gz` plus its `.sha256`.

## Install (as root, reviewed)

```
python3 -I <reviewed checkout>/packaging/native-linux/install_package.py plan \
  --archive B/dist/<id>.tar.gz --sha256 <hex> --user nes
python3 -I <reviewed checkout>/packaging/native-linux/install_package.py install \
  --archive B/dist/<id>.tar.gz --sha256 <hex> --user nes
```

`install` takes these steps:

1. Checks the digest.
2. Extracts the archive member by member into a fresh root-owned
   directory. Only regular files, directories and in-package relative
   symlinks are accepted.
3. Verifies every file against `MANIFEST.json`, then renames the directory
   into place.
4. Makes the run base (`0711`).
5. Writes the site config, only if it is absent.
6. Writes `/etc/sudoers.d/oulipoly-native` (`0440`) after `visudo -cf`. An
   existing rule is never replaced.
7. Records what it created in `<prefix>/<id>.install.json`.

`uninstall --record <that file> [--purge-site]` removes exactly what the
record names, wherever it is unchanged. The installer starts nothing,
enables no service, and refuses the old canonical prefix.

## The front door

Only one sudoers rule reaches it:

```
nes ALL=(root) NOPASSWD: /opt/oulipoly-native/<id>/libexec/oulipoly-native-frontdoor run
```

The rule pins the argument `run`. A `Defaults!` line turns off `use_pty`,
I/O logging and `setenv`. The requester is the user sudo names
(`SUDO_UID`), and must be in the site config's `allowed_users`.

The first stdin line is one JSON request:

```json
{"v": 1, "route": "sol-high", "message": "...", "cwd": "/abs/dir",
 "bash": {"authority": "trusted-task"},
 "env": {"PATH": "..."}, "deadline_s": 1800,
 "credential": {"openai": {"type": "oauth", "refresh": "", "access": "...",
                           "expires": 1700000000000, "accountId": "..."}},
 "retention": "discard"}
```

The front door enforces these rules:

* **Route.** Model and provider come only from the site config's named
  `routes`. The requester supplies no provider config, user, store or
  launch path, no `auth` path and no raw `native-root` field. Unknown
  fields are refused.
* **Bash policy.** `{"authority": "trusted-task"}` (any command, this
  task) or `{"allow": [whole commands]}`. Every other native tool is
  denied either way.
* **Environment.** The front door sets HOME/USER/LOGNAME/SHELL from the
  requester's passwd entry, the site PATH and `LANG`. The requester may
  add other names, except loader and libc-unsafe names (`LD_*`,
  `GLIBC_TUNABLES`, `TMPDIR`, ...) and names reserved for the owner,
  native host or sudo. Those are refused: this environment also reaches
  the owner and root PID 1, which run as root. The work identity is set
  before `chdir(cwd)` and the PATH lookup.
* **cwd.** Checked as the requester (a child process with the requester's
  ids), never as root.
* **Credential.** Only an inline access-only OAuth entry for the route's
  provider, and only when the route requires one. A `refresh` that is not
  empty is refused, as are other fields. `expires` must cover
  `deadline_s` + `cancel_grace_s` + `credential_margin_s`. The front door
  writes it root-only into the run's private directory and names that
  file to the entry. It removes the file when the entry reports
  `setup-completed`. After the root ends, it removes the launch
  directory's `auth.json` and `server-password` by exact path. The
  front door never reads a credential file on the requester's behalf,
  never refreshes, and never writes a credential back. The value appears
  in no argv, environment, log or output.
* **Containment.** The entry is PID 1 of a fresh PID namespace (with its
  own mount namespace and `/proc`). It is the front door's direct child
  and dies with it (parent-death SIGKILL). Killing it ends every process
  of the root, Bash work included. `--recover` is not offered.
* **Controls and abandonment.** Later stdin lines are relayed only if they
  are `{"cmd":"cancel"}`, `{"cmd":"close"}` or
  `{"cmd":"send","text":...,"ref"?:...}`. Stdin EOF, a lost stdout,
  SIGHUP/SIGINT/SIGTERM, or `deadline_s` + grace send a cancel. After the
  grace the namespace is killed.
* **Retention.** `discard` (default) removes the run directory after the
  root ends. `keep` (if the site allows it) keeps it without credentials.
  A run left behind by a killed front door has a free lock. The
  requester's next run finds it, removes its credential files, and
  removes the whole run if it was `discard`.

Stdout carries the front door's own lines (with a `frontdoor` key) and the
entry's lines, relayed unchanged. The last line is
`{"frontdoor":"terminal",...}`.

Exit status:

| Status | Meaning |
|---|---|
| entry's own (`0`, `64`, `66`, `69`, `70`, `73`, `74`, `82`–`87`) | The entry ended by itself |
| `90` | Refused before any effect |
| `91` | Failed after the run directory was made |
| `92` | Namespace killed |
| `93` | End unknown |
| `94` | A credential file could not be removed |

Nothing is retried or replayed.

## The caller

```
oulipoly-native-call --route sol-high --prompt-file task.md --cwd /work/tree \
  --out /path/new-dir --trusted-task --deadline 1800 \
  --credential-codex-profile ~/.codex2
```

The caller runs as the requester. It reads the credential only from the
source it is explicitly given, either `--credential-codex-profile DIR`
(`DIR/auth.json`, the access token's own expiry, `openai`) or
`--credential-opencode-auth FILE --credential-provider ID`. It sends the
access token, its expiry and the account id with an empty `refresh`. It
refuses a token that expires before the deadline plus
`--credential-margin`, and does not try another source.

On the message's first turn end, the caller sends `close`. At its
deadline or on SIGINT/SIGTERM it sends `cancel`.

`--out` is created with mode `0700` and holds:

* `events.jsonl`: complete stdout, which includes turn text and Bash argv;
* `stderr.log`;
* `caller.jsonl`;
* `request.public.json`: the credential reduced to provider and expiry;
* `final.md`: the last agent message linked to the message before its
  turn end;
* `result.json`.

Removing `--out` is up to the caller. Exit codes: `0` answered,
`1` no-answer, `2` usage, `3` refused-locally, `4` front-door-refused,
`5` cancelled, `6` incomplete, `7` cleanup-failed, `8` launch-failed,
`9` ended-otherwise. An answer is not a correctness or
processing-completion claim.

## Tests

* `python3 -m unittest test_frontdoor test_native_call test_build_install`
  runs offline and unprivileged.
* `fixture/run_e2e.sh <stage> <new out dir>` (as the user, never root)
  runs the packaged front door and caller against the real binaries and
  OpenCode. It uses a scripted loopback model in a fresh user namespace
  (the user mapped to root, subordinate ids for the work user), with its
  own loopback-only network, mount and PID namespaces.
  * Its test-root loader adds the overflow uid (host `/` as seen inside the
    namespace) to the trusted owners.
  * Its checks are recorded in `<out>/summary.json`.

## Limits

* **Platform.** x86_64 Linux only (`opencode-linux-x64`). Python 3.12 or
  newer (`os.unshare`) is needed at `/usr/bin/python3`.
* **Runner libraries.** The runner is the desktop crate's binary. It runs
  as root with the GUI libraries it links (listed per binary in
  `MANIFEST.json`).
* **Trust scope.** Root-owned custody (package, site config, run base and
  all their parents) is checked on every run. Installation is the only
  digest check. No security review, audit or certification is claimed.
* **Post-death recovery.** None by design: containment kills.
* **Native-runtime debts carried.** No owner deadline; unbounded output
  relay and store growth; turn end trusts the agent's tags; C6 and E1–E3
  fault paths.
* **Cancel during running Bash.** In the end-to-end fixture, a cancel
  during a running Bash was signalled by the owner, but no end was
  reported until the front door's kill.
* **Shared `/tmp`.** OpenCode writes `/tmp/opencode` as the work user.
