# Local raw-runner provenance (Linux x86_64 only)

`runner-provenance.py` owns a bounded **CLI-use-only raw runner** build and
installation boundary. It does not approve reviews, authenticate a reviewer,
release desktop bundles, install adapters, or change runtime configuration.
Use Python 3.11+, bubblewrap, Git, an actual Rust toolchain directory (not rustup
shims), a merged-usr Linux host, and the system development libraries listed in
README. Use a trusted, quiescent build machine with sufficient external disk
space for a fresh source snapshot, all vendored dependencies, and Cargo output.

## Trust boundaries

There are **two independent trust inputs**, neither extracted from the bundle:

1. **Producer custody:** the root observes the actual trusted producer execution
   and retains its successful stdout manifest SHA-256 through its own execution
   channel. This attests which manifest the run emitted, not that the source is
   reviewed. A digest supplied by an artifact submitter is not custody.
2. **Root authorization:** after required exact-identity review, the root creates
   an authorization file and retains its SHA-256 independently. The file binds
   source identity, the producer digest, candidate bytes, one explicit target,
   the root's independently measured current bytes/mode, review evidence, and an
   expiration time. The command's target must independently match this file.

The installer requires both independently retained pins. Files named
`manifest.json`, hashes computed from an untrusted download, a commit identifier,
or the string `approved-for-install` do not establish either trust input. The
CLI cannot determine who typed an argument. The invoking root owns the trusted
channel, review decision, and mutation authority. Do not accept an authorization
file/pin pair from the candidate producer. No signing key or credential is read.
No `--reviewed-commit` assertion is used.

The threat model covers stale outputs, dirty or mismatched sources, accidental
configuration leakage, tampering after independent pinning, incorrect targets,
stale current bytes, and interrupted replacement. It trusts the executing Python/
Git/bubblewrap/OS, reviewed build code and dependencies, the root's channels, and
exclusive custody of evidence/target directories. It does **not** defeat a
malicious compiler, kernel, concurrent noncooperating same-UID/root writer, or a
forged root authorization channel. Content identity is not reproducible-build
proof, a signature, or a claim that the host is trustworthy.

## Produce (no installation or approval)

After committing the exact candidate, run from that clean worktree:

```bash
python3 -B scripts/runner-provenance.py build \
  --source "$PWD" --commit FULL_EXACT_HEAD_SHA \
  --destination /absolute/external/evidence/NEW_BUILD_DIRECTORY \
  --toolchain /absolute/path/to/actual/rust/toolchain
```

The destination must not exist, even if it contains an apparently usable binary.
Do not reuse failed directories. Keep every attempt. The producer refuses dirty
tracked/untracked state, hidden index flags, external symlinks, submodules, a
mismatching HEAD, a producer invoked from a different source, and failed commands.
Ignored working files are never copied; Git fetches only the exact selected commit
into a new detached snapshot. Relative symlinks entirely inside that snapshot are
allowed. Git hooks and user/system Git configuration are disabled.

The profile `linux-x86_64-raw-cli-release-v1` executes exactly:

```text
/toolchain/bin/cargo build --frozen --release --target x86_64-unknown-linux-gnu -p oulipoly-agent-runner --bin oulipoly-agent-runner --target-dir /output/target --config /control/vendor.toml
```

It uses the tracked Cargo lock, fresh `cargo vendor --locked` resolution, and an
explicit crates.io-to-vendor configuration. Vendor fetching has network access
but no host home/credentials. Compilation has no network; vendored source,
toolchain, source snapshot, and public system tools/libraries are read-only.
The environment is cleared and replaced by the exact mapping in the manifest.
Host homes and `/usr/local` are neither mounted nor hashed. No user Cargo config,
rustup selection, inherited RUSTFLAGS, TAURI_CONFIG, build
cache, target directory, dist, node_modules, or Bun lock is admitted.

`src-tauri/tauri.conf.json` and icons **are inputs**. With the locked Tauri macro
and Cargo's default features, `custom-protocol` is absent; `dev=true` plus the
tracked `devUrl` makes `generate_context!` use empty embedded assets even for a
release-optimized raw binary. Direct Cargo does not run Tauri CLI's
`beforeBuildCommand`. Therefore this profile needs no frontend build, private
FontAwesome registry, or ignored Bun lock. **It is not a desktop distributable**:
a no-argument launch retains the existing GUI/dev-server behavior. Do not infer
UI or bundled-frontend correctness from a raw build.

Tauri's `gen/` schema directory is an explicit fresh writable output mount,
shadowing tracked generated schemas; they are regenerated rather than trusted
as build inputs. All other source remains read-only. Its generated content digest
is recorded. Other generated output lives in the fresh output/Cargo home; `/tmp` is also
bound to retained private attempt storage, not a discarded tmpfs.

The producer hashes only the mounted public distro trees (`/usr/bin`, `sbin`,
`lib`, `lib64`, `libexec`, `include`, `share` when present), merged-usr aliases,
`/etc/ld.so.cache`, public `/etc/alternatives` links, actual toolchain tree, source snapshot and vendor tree before/
after the build. Any observed change rejects attestation. This can take time and
requires a quiescent host. It records kernel/architecture, toolchain command logs,
lock hash, sandbox argv/environment, successful build log identity, and final
binary SHA-256/size. Only a successful command and newly generated ELF output can
produce a manifest. The manifest explicitly says the producer has **not** approved
review. Keep the whole directory, not just the binary/JSON.

A real candidate build requires clean committed source first. Synthetic contract
tests are not a substitute. Review the exact source/configuration/tests and
supplied build evidence through the required claim/evidence and unscoped
corrective gates before creating installation authorization.

## Root authorization and verify-only

The root independently measures the **actual selected target** (a canonical,
existing, regular single-link file), then authorizes this shape. This is a schema
example, **not** a ready-to-use authorization:

```json
{
  "schema": 1,
  "decision": "approved-for-install",
  "review_evidence": "independent exact-identity review record reference",
  "expires_at": "2030-01-01T00:00:00+00:00",
  "source": {"commit": "FULL_COMMIT", "tree": "FULL_TREE", "clean": true},
  "producer_manifest_sha256": "INDEPENDENT_PRODUCER_CUSTODY_PIN",
  "target": "/absolute/explicit/target/oulipoly-agent-runner",
  "current": {"sha256": "ROOT_MEASURED_CURRENT_SHA256", "size": 123},
  "current_mode": 493,
  "candidate": {"sha256": "REVIEWED_CANDIDATE_SHA256", "size": 456}
}
```

`current_mode` is the numeric Unix permission mode (493 decimal = 0755).
Use a real bounded expiration covering the authorized install/rollback window,
not the illustrative date above. Store the authorization outside the source;
retain its digest independently. Current and candidate fingerprints must differ.
The default is verify-only and does not create lock files or transactions:

```bash
python3 -B scripts/runner-provenance.py install \
  --source /absolute/clean/reviewed/worktree \
  --manifest /absolute/external/evidence/BUILD/manifest.json \
  --producer-manifest-sha256 INDEPENDENT_PRODUCER_CUSTODY_PIN \
  --authorization /absolute/root-owned/authorization.json \
  --authorization-sha256 INDEPENDENT_ROOT_AUTHORIZATION_PIN \
  --target /absolute/explicit/target/oulipoly-agent-runner
```

Missing/stale/mismatched provenance or authorization fails closed. Verification
rechecks the selected clean source, lock, command/environment, supplied build
logs, candidate, and actual current target bytes/mode. It does not rerun the
compiler or manufacture approval. To mutate, the independently authorized root
must repeat the same invocation with `--apply`. No default global path exists.

## Atomic installation and rollback

Installation takes a nonblocking flock on the target parent directory inode;
other invocations of this installer cooperate. It creates a retained, private
`.runner-install-<uuid>` transaction **next to the target**, stages/fsyncs both
candidate and previous bytes plus an authorization-bound receipt, rechecks current
bytes/expiry, and replaces the target with one same-filesystem atomic rename.
The previous bytes/mode remain in the transaction. A failed pre-rename operation
leaves the target unchanged; failures/crashes after rename can leave the complete
new binary installed even if no success was printed. Inspect actual target bytes
and the prewritten receipt; do not assume a nonzero command means no mutation.
This is single-file atomicity, not multi-file transactional deployment. Target ownership must match the installing effective uid/gid;
ACL/xattr/capability-bearing targets are rejected rather than silently losing
metadata. Special permission bits and symlink paths are rejected.

Rollback requires the same unexpired pinned root authorization and explicit target:

```bash
python3 -B scripts/runner-provenance.py rollback \
  --authorization /absolute/root-owned/authorization.json \
  --authorization-sha256 INDEPENDENT_ROOT_AUTHORIZATION_PIN \
  --target /absolute/explicit/target/oulipoly-agent-runner \
  --transaction /absolute/explicit/target/.runner-install-TRANSACTION_ID
```

Only the recorded previous fingerprint/mode can be restored, and only while the
actual target is the authorized candidate. Tampered backup, receipt, changed
current bytes, expired authority, and repeated rollback fail closed. Rollback
also stages/fsyncs and atomically renames; evidence/backup remain. An expired
window requires root disposition, not bypassing or editing the original record.
Do not manually clean transaction/build directories as part of this procedure.

## Contract tests

```bash
python3 -B scripts/tests/runner-provenance.test.py /absolute/external/NEW_FIXTURE_DIRECTORY
RUFF_CACHE_DIR=/absolute/external/ruff-cache ruff check scripts/runner-provenance.py scripts/runner_provenance scripts/tests/runner-provenance.test.py
RUFF_CACHE_DIR=/absolute/external/ruff-cache ruff format --check scripts/runner-provenance.py scripts/runner_provenance scripts/tests/runner-provenance.test.py
```

The test directory is exclusive/new and is never auto-cleaned. Source fixtures
fetch exactly the repository's current HEAD into retained detached checkouts;
they never commit. Installer I/O and atomic rename/rollback are exercised on
synthetic evidence-directory targets. Producer state-machine tests mock compiler
execution and host identities, clearly labeling their output synthetic. They
prove rejection/transition behavior, **not** a real runner build or review.
