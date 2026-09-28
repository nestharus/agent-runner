#!/usr/bin/env python3
"""Build and durably stage an inert, fresh-only schema-2 Linux image set.

This tool has no install, selector, alias, service, or State writer. The current
production entry paths cannot activate this image set; see the AGE-319 report.
"""

import argparse
import ctypes
import gzip
import hashlib
import io
import json
import os
import stat
import tarfile
import tomllib
import uuid
from pathlib import Path


IMAGES = {
    "runner": "oulipoly-agent-runner",
    "broker": "oulipoly-kernel-broker",
    "bash": "agent-bash",
    "launcher": "oulipoly-installed-launcher",
}
PREFIX = "first-install-v30-inert/"
MANIFEST = PREFIX + "manifest.json"
PAIR = PREFIX + "install-v1.json"
SERVICE = PREFIX + "oulipoly-kernel-broker.service"
MEMBERS = {MANIFEST, PAIR, SERVICE} | {PREFIX + name for name in IMAGES.values()}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def encoded(value: dict) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def generation(version: str, hashes: dict[str, str]) -> str:
    return str(uuid.uuid5(uuid.NAMESPACE_URL, "oulipoly-pair-v2:"
        + ":".join((version, hashes["runner"], hashes["broker"],
                    hashes["launcher"], hashes["bash"]))))


def build(images: dict[str, Path], service: Path, output: Path, version: str) -> dict:
    if set(images) != set(IMAGES):
        raise ValueError("exactly four paired images required")
    data = {role: path.read_bytes() for role, path in images.items()}
    if any(not body for body in data.values()):
        raise ValueError("empty paired image")
    hashes = {role: digest(body) for role, body in data.items()}
    pair = {
        "schema": 2, "version": version,
        "generation": generation(version, hashes),
        **{role + "_sha256": value for role, value in hashes.items()},
    }
    unit = service.read_bytes()
    manifest = {
        "schema": 1, "kind": "fresh-only-first-install-v30-inert",
        "generation": pair["generation"], "activation": "unavailable",
        "installed_pair_sha256": digest(encoded(pair)),
        "service_sha256": digest(unit),
        "images": {role: {"sha256": hashes[role], "size": len(data[role])}
                   for role in IMAGES},
    }
    files = {MANIFEST: encoded(manifest), PAIR: encoded(pair), SERVICE: unit}
    files.update({PREFIX + IMAGES[role]: data[role] for role in IMAGES})
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("xb") as stream:
        with gzip.GzipFile(fileobj=stream, mode="wb", filename="", mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as archive:
                for name, body in sorted(files.items()):
                    item = tarfile.TarInfo(name)
                    item.size, item.mode, item.uid, item.gid, item.mtime = len(body), 0o400, 0, 0, 0
                    archive.addfile(item, io.BytesIO(body))
        os.fsync(stream.fileno())
    if verify(output) != manifest:
        raise ValueError("package readback changed")
    return manifest


def _read_package(package: Path | io.BytesIO) -> tuple[dict, dict[str, bytes]]:
    source = {"fileobj": package} if isinstance(package, io.BytesIO) else {"name": package}
    with tarfile.open(mode="r:gz", **source) as archive:
        entries = archive.getmembers()
        if len(entries) != len(MEMBERS) or {item.name for item in entries} != MEMBERS:
            raise ValueError("package member set changed")
        if any(not item.isfile() or item.mode != 0o400 or item.uid != 0 or item.gid != 0
               or item.name.startswith("/") or ".." in Path(item.name).parts
               or item.size > 256 * 1024 * 1024 for item in entries):
            raise ValueError("unsafe package member")
        files = {item.name: archive.extractfile(item).read() for item in entries}
    return _validate(files), files


def _validate(files: dict[str, bytes]) -> dict:
    if set(files) != MEMBERS:
        raise ValueError("member set changed")
    manifest = json.loads(files[MANIFEST])
    pair = json.loads(files[PAIR])
    if encoded(manifest) != files[MANIFEST] or encoded(pair) != files[PAIR]:
        raise ValueError("noncanonical package manifest")
    if set(manifest) != {"schema", "kind", "generation", "activation",
                         "installed_pair_sha256", "service_sha256", "images"}:
        raise ValueError("package manifest fields changed")
    if (manifest["schema"], manifest["kind"], manifest["activation"]) != (
        1, "fresh-only-first-install-v30-inert", "unavailable"
    ) or set(manifest["images"]) != set(IMAGES):
        raise ValueError("package manifest incompatible")
    if set(pair) != {"schema", "version", "generation", "runner_sha256",
                     "broker_sha256", "launcher_sha256", "bash_sha256"} or pair["schema"] != 2:
        raise ValueError("schema-2 installed pair required")
    hashes = {role: digest(files[PREFIX + name]) for role, name in IMAGES.items()}
    if (not pair["version"] or not files[SERVICE]
            or manifest["generation"] != pair["generation"]
            or pair["generation"] != generation(pair["version"], hashes)
            or manifest["installed_pair_sha256"] != digest(files[PAIR])
            or manifest["service_sha256"] != digest(files[SERVICE])):
        raise ValueError("package generation or service changed")
    for role, name in IMAGES.items():
        if pair[role + "_sha256"] != hashes[role] or manifest["images"][role] != {
            "sha256": hashes[role], "size": len(files[PREFIX + name])
        }:
            raise ValueError(f"{role} image changed")
    return manifest


def verify(package: Path) -> dict:
    return _read_package(package)[0]


def _safe_ancestry(path: Path, require_root: bool) -> None:
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError("absolute normalized staging path required")
    part = path
    while True:
        info = part.lstat()
        if not stat.S_ISDIR(info.st_mode) or (require_root and
                (info.st_uid != 0 or info.st_mode & 0o022)):
            raise ValueError("unsafe staging ancestry")
        if part == Path("/"):
            return
        part = part.parent


def _sync_dir(path: Path) -> None:
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def stage(package: Path, destination: Path, *, require_root: bool = False) -> dict:
    if require_root and os.geteuid() != 0:
        raise PermissionError("root required for installed staging")
    _safe_ancestry(destination.parent, require_root)
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(destination)
    manifest, files = _read_package(package)
    pending = destination.parent / (".first-install-v30-" + str(uuid.uuid4()))
    pending.mkdir(mode=0o700)
    try:
        for name, body in files.items():
            path = pending / Path(name).name
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o400)
            try:
                with os.fdopen(os.dup(fd), "wb") as stream:
                    stream.write(body)
                    stream.flush()
                os.fsync(fd)
            finally:
                os.close(fd)
        _sync_dir(pending)
        libc = ctypes.CDLL(None, use_errno=True)
        if libc.renameat2(-100, os.fsencode(pending), -100, os.fsencode(destination), 1) != 0:
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error), destination)
        _sync_dir(destination.parent)
    finally:
        if pending.exists():
            for path in pending.iterdir():
                path.unlink()
            pending.rmdir()
    if check(destination, require_root=require_root) != manifest:
        raise ValueError("staged readback changed")
    return manifest


def check(destination: Path, *, require_root: bool = False) -> dict:
    _safe_ancestry(destination, require_root)
    names = {Path(name).name for name in MEMBERS}
    if {path.name for path in destination.iterdir()} != names:
        raise ValueError("staged member set changed")
    for path in destination.iterdir():
        info = path.lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1
                or stat.S_IMODE(info.st_mode) != 0o400
                or (require_root and info.st_uid != 0)):
            raise ValueError("unsafe staged file")
    # Use the same exact manifest/generation rules as archive verification.
    files = {PREFIX + path.name: path.read_bytes() for path in destination.iterdir()}
    return _validate(files)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    build_cmd = sub.add_parser("build")
    for role in IMAGES:
        build_cmd.add_argument("--" + role, type=Path, required=True)
    build_cmd.add_argument("--output", type=Path, required=True)
    verify_cmd = sub.add_parser("verify")
    verify_cmd.add_argument("package", type=Path)
    stage_cmd = sub.add_parser("stage")
    stage_cmd.add_argument("package", type=Path)
    stage_cmd.add_argument("destination", type=Path)
    stage_cmd.add_argument("--fixture", action="store_true")
    check_cmd = sub.add_parser("check")
    check_cmd.add_argument("destination", type=Path)
    check_cmd.add_argument("--fixture", action="store_true")
    args = parser.parse_args()
    if args.command == "build":
        root = Path(__file__).resolve().parents[2]
        with (root / "Cargo.toml").open("rb") as stream:
            version = tomllib.load(stream)["workspace"]["package"]["version"]
        result = build({role: getattr(args, role) for role in IMAGES},
                       Path(__file__).with_name("oulipoly-kernel-broker.service"),
                       args.output, version)
    elif args.command == "verify":
        result = verify(args.package)
    elif args.command == "stage":
        result = stage(args.package, args.destination, require_root=not args.fixture)
    else:
        result = check(args.destination, require_root=not args.fixture)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
