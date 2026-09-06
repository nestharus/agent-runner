"""One controlled Linux raw-runner producer; not a review-approval issuer."""

import os
from pathlib import Path
import platform

from .common import (
    canonical_path,
    command,
    fingerprint,
    git_environment,
    json_bytes,
    require,
    source_identity,
    tree_digest,
    write_json,
    write_new,
)

# Public distro build inputs only: never mount/hash /usr/local or host homes.
SYSTEM_TREES = (
    "/usr/bin",
    "/usr/sbin",
    "/usr/lib",
    "/usr/lib64",
    "/usr/libexec",
    "/usr/include",
    "/usr/share",
)
PROFILE = "linux-x86_64-raw-cli-release-v1"
BUILD = [
    "/toolchain/bin/cargo",
    "build",
    "--frozen",
    "--release",
    "--target",
    "x86_64-unknown-linux-gnu",
    "-p",
    "oulipoly-agent-runner",
    "--bin",
    "oulipoly-agent-runner",
    "--target-dir",
    "/output/target",
    "--config",
    "/control/vendor.toml",
]
ENVIRONMENT = {
    "PATH": "/toolchain/bin:/usr/bin:/bin",
    "HOME": "/home/build",
    "CARGO_HOME": "/cargo-home",
    "RUSTC": "/toolchain/bin/rustc",
    "LC_ALL": "C",
    "TZ": "UTC",
    "CARGO_TERM_COLOR": "never",
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_NO_REPLACE_OBJECTS": "1",
    "GIT_TERMINAL_PROMPT": "0",
}


def host_identity(toolchain):
    # These are public compiler/library trees, never the caller's home/config.
    return {
        "system_trees": {
            path: tree_digest(path) for path in SYSTEM_TREES if Path(path).exists()
        },
        "alternatives": tree_digest("/etc/alternatives"),
        "toolchain": tree_digest(toolchain),
        "lib": os.readlink("/lib"),
        "lib64": os.readlink("/lib64"),
        "bin": os.readlink("/bin"),
        "ld_cache": fingerprint("/etc/ld.so.cache"),
        "kernel": platform.release(),
        "machine": platform.machine(),
    }


def sandbox(root, toolchain, network):
    argv = [
        "/usr/bin/bwrap",
        "--die-with-parent",
        "--new-session",
        "--unshare-all",
        "--clearenv",
        "--symlink",
        os.readlink("/bin"),
        "/bin",
        "--symlink",
        os.readlink("/lib"),
        "/lib",
        "--symlink",
        os.readlink("/lib64"),
        "/lib64",
        "--ro-bind",
        "/etc/alternatives",
        "/etc/alternatives",
        "--ro-bind",
        "/etc/ld.so.cache",
        "/etc/ld.so.cache",
        "--ro-bind",
        str(toolchain),
        "/toolchain",
        "--ro-bind",
        str(root / "source"),
        "/source",
        "--bind",
        str(root / "output"),
        "/output",
        "--bind",
        str(root / "generated"),
        "/source/src-tauri/gen",
        "--ro-bind",
        str(root / "control"),
        "/control",
        "--bind",
        str(root / "cargo-home"),
        "/cargo-home",
        "--bind",
        str(root / "temp"),
        "/tmp",
        "--dir",
        "/home/build",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--chdir",
        "/source",
    ]
    for path in SYSTEM_TREES:
        if Path(path).exists():
            argv += ["--ro-bind", path, path]
    if network:
        argv += [
            "--share-net",
            "--ro-bind",
            "/etc/resolv.conf",
            "/etc/resolv.conf",
            "--ro-bind",
            "/etc/ssl/certs",
            "/etc/ssl/certs",
            "--bind",
            str(root / "vendor"),
            "/vendor",
        ]
    else:
        argv += ["--ro-bind", str(root / "vendor"), "/vendor"]
    for key, value in ENVIRONMENT.items():
        argv += ["--setenv", key, value]
    return argv


def snapshot(repo, root, identity):
    env = git_environment()
    source = root / "source"
    command(["/usr/bin/git", "init", str(source)], root, root / "001-init.log", env)
    command(
        [
            "/usr/bin/git",
            "-c",
            "protocol.file.allow=always",
            "-C",
            str(source),
            "fetch",
            "--no-tags",
            "--depth=1",
            str(repo),
            identity["commit"],
        ],
        root,
        root / "002-fetch-source.log",
        env,
    )
    command(
        [
            "/usr/bin/git",
            "-c",
            "core.hooksPath=/dev/null",
            "-C",
            str(source),
            "checkout",
            "--detach",
            identity["commit"],
        ],
        root,
        root / "003-checkout.log",
        env,
    )
    require(
        source_identity(source, identity["commit"]) == identity, "snapshot mismatch"
    )


def build(repo, commit, destination, toolchain):
    require(
        platform.system() == "Linux" and platform.machine() == "x86_64",
        "only Linux x86_64 is supported",
    )
    repo = canonical_path(repo)
    require(
        repo == Path(__file__).resolve().parents[2], "run producer from selected source"
    )
    identity = source_identity(repo, commit)
    root = canonical_path(destination)
    require(not root.is_relative_to(repo), "build storage must be outside source")
    toolchain = canonical_path(toolchain)
    require(
        (toolchain / "bin/cargo").is_file() and (toolchain / "bin/rustc").is_file(),
        "supply actual Rust toolchain directory, not rustup shims",
    )
    # Exclusive creation rejects stale output before any command can run.
    root.mkdir(mode=0o700)
    for name in ("output", "control", "cargo-home", "vendor", "generated", "temp"):
        (root / name).mkdir()
    snapshot(repo, root, identity)
    before = host_identity(toolchain)
    write_json(root / "host-inputs.json", before)
    source_before = tree_digest(root / "source")
    online = sandbox(root, toolchain, True)
    offline = sandbox(root, toolchain, False)
    command(offline + ["/toolchain/bin/rustc", "-vV"], root, root / "004-rustc.log", {})
    command(offline + ["/toolchain/bin/cargo", "-vV"], root, root / "005-cargo.log", {})
    # Config is known, rather than stdout of an untrusted producer command.
    write_new(
        root / "control/vendor.toml",
        b'[source.crates-io]\nreplace-with = "vendored-sources"\n[source.vendored-sources]\ndirectory = "/vendor"\n',
    )
    command(
        online + ["/toolchain/bin/cargo", "vendor", "--locked", "/vendor"],
        root,
        root / "006-vendor.log",
        {},
    )
    vendor_before = tree_digest(root / "vendor")
    lock = fingerprint(root / "source/Cargo.lock")
    command(offline + BUILD, root, root / "007-build.log", {})
    require(tree_digest(root / "vendor") == vendor_before, "dependency inputs changed")
    require(tree_digest(root / "source") == source_before, "source snapshot changed")
    require(host_identity(toolchain) == before, "host/toolchain inputs changed")
    require(source_identity(repo, commit) == identity, "selected source changed")
    output = (
        root / "output/target/x86_64-unknown-linux-gnu/release/oulipoly-agent-runner"
    )
    output_identity = fingerprint(output)  # Reject symlink/nonregular outputs.
    output_bytes = output.read_bytes()
    require(output_bytes[:4] == b"\x7fELF", "output is not an ELF binary")
    candidate = root / "oulipoly-agent-runner"
    write_new(candidate, output_bytes, 0o755)
    require(
        fingerprint(candidate) == output_identity, "output changed during custody copy"
    )
    manifest = {
        "schema": 1,
        "profile": PROFILE,
        "source": identity,
        "build": {
            "argv": BUILD,
            "environment": ENVIRONMENT,
            "exit_code": 0,
            "sandbox_argv": offline,
            "network": False,
        },
        "inputs": {
            "cargo_lock": lock,
            "vendor_sha256": vendor_before,
            "source_snapshot_sha256": source_before,
            "host": before,
            "configuration": fingerprint(root / "control/vendor.toml"),
            "generated_sha256": tree_digest(root / "generated"),
        },
        "toolchain": {
            "rustc": fingerprint(root / "004-rustc.log"),
            "cargo": fingerprint(root / "005-cargo.log"),
        },
        "evidence": {
            name: fingerprint(root / name)
            for name in (
                "004-rustc.log",
                "005-cargo.log",
                "006-vendor.log",
                "007-build.log",
            )
        },
        "output": fingerprint(candidate),
        "producer": {
            "review_approval": False,
            "script_sha256": tree_digest(repo / "scripts/runner_provenance"),
        },
    }
    write_json(root / "manifest.json", manifest)
    # Custodian retains this through its independent execution channel.
    print(
        json_bytes(
            {
                "manifest": str(root / "manifest.json"),
                **fingerprint(root / "manifest.json"),
            }
        ).decode(),
        end="",
    )
    return manifest
