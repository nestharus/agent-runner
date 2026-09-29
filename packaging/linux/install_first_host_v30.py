#!/usr/bin/env python3
"""Install and read back the fixed-path schema-2 first host image set.

Fixture roots prove only filesystem publication. Broker offline commands and
systemd require the real fixed paths and are never redirected into a fixture.
"""

import argparse
import ctypes
import grp
import json
import os
import socket
import stat
import struct
import subprocess
import sys
import time
import tomllib
import uuid
from pathlib import Path

from first_install_v30 import IMAGES, PREFIX, _read_package, _safe_ancestry, _sync_dir


IMAGE_DIR = Path("/usr/local/libexec/oulipoly")
UNIT = Path("/etc/systemd/system/oulipoly-kernel-broker.service")
STATE = Path("/var/lib/oulipoly-kernel-broker")
SOCKET = Path("/run/oulipoly-kernel-broker/control.sock")


def paths(fixture_root: Path | None):
    if fixture_root is None:
        return IMAGE_DIR, UNIT, STATE
    if (not fixture_root.is_absolute() or ".." in fixture_root.parts
            or fixture_root == Path("/")):
        raise ValueError("absolute normalized fixture root required")
    if not fixture_root.is_dir() or fixture_root.is_symlink():
        raise ValueError("fixture root must already be a directory")
    _safe_ancestry(fixture_root, False)
    return (fixture_root / IMAGE_DIR.relative_to("/"),
            fixture_root / UNIT.relative_to("/"),
            fixture_root / STATE.relative_to("/"))


def require_production(fixture_root: Path | None):
    if fixture_root is not None:
        raise ValueError("Broker and systemd operations have no fixture mode")
    if os.geteuid() != 0:
        raise PermissionError("root required for production first install")


def preflight(package: Path, fixture_root: Path | None):
    if fixture_root is None and os.geteuid() != 0:
        raise PermissionError("root required for production first install")
    manifest, files = _read_package(package)
    canonical_service = Path(__file__).with_name("oulipoly-kernel-broker.service").read_bytes()
    if files[PREFIX + "oulipoly-kernel-broker.service"] != canonical_service:
        raise ValueError("package service is not the exact installed Broker unit")
    with (Path(__file__).resolve().parents[2] / "Cargo.toml").open("rb") as stream:
        version = tomllib.load(stream)["workspace"]["package"]["version"]
    pair = json.loads(files[PREFIX + "install-v1.json"])
    if pair["version"] != version:
        raise ValueError("package version differs from this installer source")
    image_dir, unit, state = paths(fixture_root)
    if fixture_root is None:
        if state.exists() or state.is_symlink():
            raise FileExistsError(f"existing Broker State refuses first install: {state}")
        try:
            grp.getgrnam("oulipoly")
        except KeyError as error:
            raise ValueError("oulipoly group required by Broker service") from error
    if image_dir.exists() or image_dir.is_symlink():
        raise FileExistsError(image_dir)
    if unit.exists() or unit.is_symlink():
        raise FileExistsError(unit)
    return manifest, files


def write_exclusive(path: Path, body: bytes, mode: int):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    try:
        with os.fdopen(os.dup(fd), "wb") as stream:
            stream.write(body)
            stream.flush()
        os.fchmod(fd, mode)
        os.fsync(fd)
    finally:
        os.close(fd)


def install(package: Path, fixture_root: Path | None):
    manifest, files = preflight(package, fixture_root)
    image_dir, unit, _ = paths(fixture_root)
    for parent in (image_dir.parent, unit.parent):
        parent.mkdir(mode=0o755, parents=True, exist_ok=True)
        _safe_ancestry(parent, fixture_root is None)
    # A private sibling is completed before its no-replace publication. An
    # interrupted publication is visible to check; install never overwrites.
    pending = image_dir.parent / (".oulipoly-first-install-" + str(uuid.uuid4()))
    pending.mkdir(mode=0o700)
    try:
        for name in ("install-v1.json", *IMAGES.values()):
            mode = 0o555 if name in IMAGES.values() else 0o444
            write_exclusive(pending / name, files[PREFIX + name], mode)
        pending.chmod(0o755)
        _sync_dir(pending)
        libc = ctypes.CDLL(None, use_errno=True)
        if libc.renameat2(-100, os.fsencode(pending), -100,
                          os.fsencode(image_dir), 1) != 0:
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error), image_dir)
        _sync_dir(image_dir.parent)
    finally:
        if pending.exists():
            for child in pending.iterdir():
                child.unlink()
            pending.rmdir()
    # This separate unit publication is intentionally observable on retry.
    # check reports a partial install if it was interrupted here.
    write_exclusive(unit, files[PREFIX + "oulipoly-kernel-broker.service"], 0o444)
    _sync_dir(unit.parent)
    if check(package, fixture_root) != manifest:
        raise ValueError("installed readback changed")
    return manifest


def check(package: Path, fixture_root: Path | None):
    if fixture_root is None and os.geteuid() != 0:
        raise PermissionError("root required for production installed check")
    manifest, files = _read_package(package)
    if files[PREFIX + "oulipoly-kernel-broker.service"] != Path(__file__).with_name(
            "oulipoly-kernel-broker.service").read_bytes():
        raise ValueError("package service is not the exact installed Broker unit")
    image_dir, unit, _ = paths(fixture_root)
    _safe_ancestry(image_dir, fixture_root is None)
    _safe_ancestry(unit.parent, fixture_root is None)
    if stat.S_IMODE(image_dir.stat().st_mode) != 0o755:
        raise ValueError("installed image directory mode changed")
    expected = {"install-v1.json", *IMAGES.values()}
    if {path.name for path in image_dir.iterdir()} != expected:
        raise ValueError("installed member set changed")
    for name in expected:
        path = image_dir / name
        info = path.lstat()
        mode = 0o555 if name in IMAGES.values() else 0o444
        if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1
                or stat.S_IMODE(info.st_mode) != mode
                or (fixture_root is None and info.st_uid != 0)
                or path.read_bytes() != files[PREFIX + name]):
            raise ValueError(f"installed file changed: {name}")
    info = unit.lstat()
    if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1
            or stat.S_IMODE(info.st_mode) != 0o444
            or (fixture_root is None and info.st_uid != 0)
            or unit.read_bytes() != files[PREFIX + unit.name]):
        raise ValueError("installed service changed")
    return manifest


def activate(package: Path):
    require_production(None)
    check(package, None)
    broker = IMAGE_DIR / IMAGES["broker"]
    bootstrap = subprocess.run([broker, "--bootstrap-empty-v30-state"],
                               check=True, capture_output=True, text=True)
    activation = subprocess.run([broker, "--activate-first-install-v30"],
                                check=True, capture_output=True, text=True)
    source = json.loads(bootstrap.stdout)
    record = json.loads(activation.stdout)
    pair = json.loads((IMAGE_DIR / "install-v1.json").read_text())
    if record["source"] != source or record["pair_generation"] != pair["generation"]:
        raise ValueError("offline Broker pair/source identity changed")
    return record


def activation_identity():
    pair = json.loads((IMAGE_DIR / "install-v1.json").read_text())
    record = json.loads((STATE / "first-install-activation-v1.json").read_text())
    source = json.loads((STATE / "empty-v30-bootstrap-v1.json").read_text())
    if record["source"] != source or record["pair_generation"] != pair["generation"]:
        raise ValueError("stored Broker pair/source identity changed")
    return pair, record, source


def readback(package: Path):
    require_production(None)
    check(package, None)
    pair, record, source = activation_identity()
    _safe_ancestry(SOCKET.parent, True)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(5)
        connection.connect(str(SOCKET))
        _, uid, _ = struct.unpack("3i", connection.getsockopt(
            socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")))
        if uid != 0:
            raise ValueError("live Broker peer is not root")
        challenge = connection.recv(16, socket.MSG_WAITALL)
        if len(challenge) != 16:
            raise ValueError("live Broker challenge changed")
        connection.sendall(b"i" + challenge)
        reply = connection.recv(257)
        if reply != b"entry-gate-v1 fresh-only-open\n" or connection.recv(1):
            raise ValueError(f"live Broker route changed: {reply!r}")
    return {"live_route": reply.decode().strip(), "pair_generation": pair["generation"],
            "source_generation": source["source_generation"], "activation": record}


def start(package: Path):
    require_production(None)
    check(package, None)
    activation_identity()
    subprocess.run(["systemctl", "daemon-reload"], check=True)
    subprocess.run(["systemctl", "start", "oulipoly-kernel-broker.service"], check=True)
    # The socket appears after the service process has completed its own
    # startup checks; wait only for that short service startup interval.
    deadline = time.monotonic() + 10
    while True:
        try:
            return readback(package)
        except (OSError, subprocess.CalledProcessError):
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("install", "check", "activate", "start",
                                           "readback"))
    parser.add_argument("package", type=Path)
    parser.add_argument("--fixture-root", type=Path)
    args = parser.parse_args()
    if args.command in ("install", "check"):
        result = globals()[args.command](args.package, args.fixture_root)
    else:
        require_production(args.fixture_root)
        result = globals()[args.command](args.package)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"first host install refused: {error}", file=sys.stderr)
        sys.exit(1)
