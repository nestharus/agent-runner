# Linux ACPv2 continuity fixture

This fixture demonstrated one native OpenCode conversation retaining its prior
history while an external Rust ACPv2 client resumed it, inserted a user message,
received an ACK matching the exact native body, and saw the message render live
on a 140-column × 40-row PTY. The history was freshly seeded by a separate host
that exited before the TUI started. It was not an old user database or migration
test. There is no runner-owner integration and no real model turn.

The three proof sources are unchanged from that run. From the repository root,
check them with `sha256sum -c tools/acp-v2-insertion-proof/SHA256SUMS`.

## Prerequisites

The reproduction recipe targets **Linux x86-64 with glibc and AVX2**, matching
the public `opencode-linux-x64` binary. Other CPUs, musl, Windows and macOS are
outside this recipe and its evidence. You need Python 3.9+ with `venv`/pip
(packaging checked on 3.12.3), Node/npm (checked on Node 22.23.2/npm 11.6.2),
Rust/Cargo 1.88+ (example built on 1.92.0), a C linker, and `unshare`, `setpriv`,
`timeout` (util-linux/coreutils) and `ip` (iproute2). The kernel and local policy
must permit unprivileged user and network namespaces. Do not remove network
isolation to work around a namespace denial.

Public downloads require network access during preparation. The fixture itself
runs with loopback only. No existing build directory, OpenCode installation,
profile, credentials, service or database is needed. No global plugin is installed.

## Prepare a fresh private directory

Run these Bash commands from the repository root. Keep `proof_dir` outside the
checkout and use an absolute path without spaces, `?` or `#` (the driver places
paths directly into URLs). Use a short path to stay within Unix socket limits.

```bash
set -euo pipefail
repo_dir=$(pwd -P)
fixture_dir="$repo_dir/tools/acp-v2-insertion-proof"
umask 077
proof_dir=$(mktemp -d /tmp/acpv2.XXXXXX)
mkdir "$proof_dir/deps" "$proof_dir/pydeps" "$proof_dir/install-home"
touch "$proof_dir/npmrc-user" "$proof_dir/npmrc-global"
cp "$fixture_dir/package.json" "$fixture_dir/package-lock.json" "$proof_dir/deps/"
(
  cd "$proof_dir/deps"
  env -i PATH="$PATH" HOME="$proof_dir/install-home" \
    npm_config_userconfig="$proof_dir/npmrc-user" \
    npm_config_globalconfig="$proof_dir/npmrc-global" \
    npm_config_cache="$proof_dir/npm-cache" \
    npm ci --ignore-scripts --no-audit --no-fund
)
python3 -m venv "$proof_dir/venv"
env -i PATH=/usr/bin:/bin HOME="$proof_dir/install-home" \
  "$proof_dir/venv/bin/python" -m pip --isolated install \
  --disable-pip-version-check --no-cache-dir --no-compile --no-deps \
  --only-binary=:all: --platform any --require-hashes \
  --target "$proof_dir/pydeps" -r "$fixture_dir/requirements.txt"
cargo build --locked -p oulipoly-acp --example v2_socket_submit \
  --target-dir "$proof_dir/target"
proof_opencode="$proof_dir/deps/node_modules/opencode-linux-x64/bin/opencode"
proof_client="$proof_dir/target/debug/examples/v2_socket_submit"
test -x "$proof_opencode"
test -x "$proof_client"
sha256sum -c "$fixture_dir/SHA256SUMS"
```

`package-lock.json` pins the [ACP SDK 1.7.0](https://www.npmjs.com/package/@agentclientprotocol/sdk/v/1.7.0),
[zod 4.6.5](https://www.npmjs.com/package/zod/v/4.6.5) and
[OpenCode Linux x64 1.18.30](https://www.npmjs.com/package/opencode-linux-x64/v/1.18.30)
with registry integrity values. OpenCode embeds the plugin's JavaScript runtime;
Node is used for installation/import checks, not as another ACP bridge process.
The downloaded OpenCode binary was byte-identical to the prior installed binary
(`87bd160e053af86b5b409daabf71f8dc05bbc3a2a3a5f563f36011cdf706a999`).
This is a binary comparison, not a source-build attestation.

`requirements.txt` pins [pyte 0.8.2](https://pypi.org/project/pyte/0.8.2/) and
[wcwidth 0.9.1](https://pypi.org/project/wcwidth/0.9.1/), using hashed Python-only
wheels. The earlier run used an extracted macOS wcwidth wheel's Python fallback
on Linux; the Python files match this Python-only wheel byte for byte. This
packaging check does not independently validate the dependency supply chain.

## Run the continuity fixture in the foreground

The preserved driver hardcodes `OPENCODE = "/home/nes/.opencode/bin/opencode"`.
The invocation below loads the unchanged file with `runpy`, changes only that
module variable to the private public binary, then calls `main()`. It neither
edits the file nor replaces an installed binary. The driver receives the fresh
directory, mode and built Rust client as its usual arguments.

```bash
set +e
env -i PATH=/usr/bin:/bin \
  timeout --signal=TERM --kill-after=20s 500s \
  unshare --user --map-current-user --keep-caps --net sh -c '
    ip link set lo up || exit
    exec setpriv --inh-caps=-all --bounding-set=-all \
      python3 -B -c '\''import runpy, sys
proof = runpy.run_path(sys.argv.pop(1))
proof["main"].__globals__["OPENCODE"] = sys.argv.pop(1)
proof["main"]()'\'' "$@"
  ' sh "$fixture_dir/run_proof.py" "$proof_opencode" \
    "$proof_dir" tui-existing "$proof_client" \
  > "$proof_dir/attempt-tui-existing.log" 2>&1
proof_exit=$?
set -e
cat "$proof_dir/attempt-tui-existing.log"
printf 'proof_exit=%s\nproof_dir=%s\n' "$proof_exit" "$proof_dir"
```

Await this exact foreground command to its native exit; if a terminal tool
yields a session handle, collect that handle through completion before reading
the log. Do not detach or infer completion from an output file. Allow up to
500 seconds (driver alarm: 420 seconds). A successful result requires exit 0
**and** `PROOF RESULT PASS` with the supporting evidence below. A timeout,
exception or missing native exit is incomplete evidence.

The driver copies the endpoint beside `deps/node_modules`, rebuilds the host
environment from scratch, and creates fresh HOME/XDG/project directories inside
`run-tui-existing-<timestamp>/`. It disables default plugins, project config,
Claude configuration, autoupdate and sharing. The plugin is selected only by
the child host's `OPENCODE_CONFIG_CONTENT`. Keep runs sequential in this private
directory; port 4797 is loopback-local inside each network namespace.

## Read the result and retain the evidence

The seed host creates two user history messages with native `noReply=true`,
then exits. A new native TUI starts with `--session <seeded-id>`. The listener
lives inside that host; the Rust client initializes protocol 2, resumes the
same id over its Unix socket and submits one token. ACK waits for native bus
events and readback matching id, user role and exact text, rather than just
HTTP 204. Native readback must retain history one, history two, then the token.

Inspect `pty-pre-attach.raw` / `screen-pre-attach.txt` and
`pty-post-submit.raw` / `screen-post-submit.txt` in the run directory. The
screens are reconstructed with pyte. Confirm both history messages before
attach, token absence before attach, and the live token below the history
after submit. The driver enforces only the last prior history row on the
pre-attach screen; its printed diagnostics for the other row and token absence
still need inspection. `acp-v2-endpoint.log` holds plugin diagnostics separately
from the PTY. Native logs and the fresh database remain under `xdg/`.

The fixture deliberately uses the invalid `proof-none/none` model with
`OULIPOLY_ACP_V2_PROOF_NO_REPLY=1`. The prior TUI showed an invalid-model toast
before insertion; it had cleared afterward. Loopback-only operation also
produced models.dev fetch failures and a background dependency-install warning.
The two fixed 2-second settle pumps print `TIMEOUT`; those are expected pumps.
Do not remove noReply: the endpoint also hardcodes the invalid model, so omitting
the knob does not turn this into configured model processing.

Only one TUI continuity run has been observed. This packaging work checked
public dependency installation/imports, byte agreement and the example build;
it did not rerun the TUI or demonstrate repeatability. `serve-existing` can
replace `tui-existing` for native seed/resume/readback without a PTY; `serve`
uses a new ACP session without prior history. Neither establishes TUI continuity.

Remaining limits: minimal resume has no history replay, cwd check or instance
isolation; its capability is advertised using an `any` cast for a field absent
from the pinned SDK type. No full ACPv2 conformance, auth, dedup, disconnect,
busy/cancel, production ACK guarantees, model processing, runner lifecycle,
old-history migration, full workspace/319 coverage or stress result is claimed.
Host group cleanup lacks a PGID-reuse guard; client startup readline is bounded
only by the outer alarm and exception cleanup is incomplete. Inspect failures
before removing evidence, and target only recorded fixture-owned processes if
cleanup is needed. No broad process scans/signals are part of this recipe.

Keep useful logs/screens/state for review; once owned executables are no longer
in use, remove only the exact `proof_dir` you created. Downloads, node_modules,
Python caches, Cargo targets, sockets, databases and raw logs are local residue,
not repository content. This package adds no ignore rule or automatic cleanup.
