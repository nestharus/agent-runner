#!/usr/bin/env python3
"""Durably retain an AGE-319 old/new fixture; never install or activate it.

There is deliberately no selector writer or public alias in this module.  The
fixed v1 InstalledPair reader cannot consume a v2 selector, and the current
Runner/Bash binaries do not share a production fresh admission protocol.
"""

import argparse
import ctypes
import fcntl
import hashlib
import json
import os
import shutil
import stat
import tomllib
import uuid
from pathlib import Path


ASSETS = {
    "legacy_runner": ("legacy/runner/oulipoly-agent-runner", 0o400),
    "legacy_runner_config": ("legacy/runner/config.toml", 0o400),
    "legacy_bash": ("legacy/bash/agent-bash", 0o400),
    "legacy_bash_config": ("legacy/bash/agent-bash.toml", 0o400),
    "fresh_runner": ("fresh/runner/oulipoly-agent-runner", 0o400),
    "fresh_runner_config": ("fresh/runner/config.toml", 0o400),
    "fresh_bash": ("fresh/bash/agent-bash", 0o400),
    "fresh_bash_config": ("fresh/bash/agent-bash.toml", 0o400),
    "fresh_broker": ("fresh/broker/oulipoly-kernel-broker", 0o400),
    "fresh_launcher": ("fresh/launcher/oulipoly-installed-launcher", 0o400),
}
MANIFEST = "staging-v1.json"


def _rename_new(source: Path, destination: Path) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    result = libc.renameat2(
        -100, os.fsencode(source), -100, os.fsencode(destination), 1  # RENAME_NOREPLACE
    )
    if result != 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error), destination)


def _safe_path(path: Path, *, require_root: bool, directory: bool) -> None:
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError(f"unsafe path: {path}")
    for part in (path, *path.parents):
        info = part.lstat()
        if stat.S_ISLNK(info.st_mode):
            raise ValueError(f"symlink in staging path: {part}")
        if require_root and (info.st_uid != 0 or info.st_mode & 0o022):
            raise ValueError(f"untrusted staging path: {part}")
    info = path.lstat()
    if stat.S_ISDIR(info.st_mode) != directory:
        raise ValueError(f"wrong staging path type: {path}")


def _absolute_config_path(value: object, name: str) -> Path:
    if not isinstance(value, str):
        raise ValueError(f"missing {name}")
    path = Path(value)
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError(f"{name} must be an absolute normalized path")
    return path


def _config(source: Path, fields: tuple[str, ...]) -> dict[str, Path]:
    with source.open("rb") as stream:
        parsed = tomllib.load(stream)
    if set(parsed) != set(fields):
        raise ValueError(f"unexpected fields in {source}")
    return {field: _absolute_config_path(parsed[field], field) for field in fields}


def _overlap(left: Path, right: Path) -> bool:
    for first, second in ((left, right), (left.resolve(), right.resolve())):
        if first == second or first.is_relative_to(second) or second.is_relative_to(first):
            return True
    return False


def _check_configs(sources: dict[str, Path], destination: Path, legacy_runner_path: Path) -> dict[str, str]:
    old_runner = _config(sources["legacy_runner_config"], ("data_dir", "config_home"))
    new_runner = _config(sources["fresh_runner_config"], ("data_dir", "config_home"))
    old_bash = _config(sources["legacy_bash_config"], ("state_root", "agent_runner_bin"))
    new_bash = _config(sources["fresh_bash_config"], ("state_root", "agent_runner_bin"))
    if old_bash["agent_runner_bin"] != legacy_runner_path:
        raise ValueError("legacy Bash helper path does not name the retained legacy Runner")
    if new_bash["agent_runner_bin"] != destination / ASSETS["fresh_runner"][0]:
        raise ValueError("fresh Bash helper path does not name the staged fresh Runner")
    for name in ("data_dir", "config_home"):
        if old_runner[name] == new_runner[name]:
            raise ValueError(f"fresh {name} reuses the legacy path")
    for fresh_root in (new_runner["data_dir"], new_bash["state_root"]):
        for legacy_root in (old_runner["data_dir"], old_bash["state_root"]):
            if _overlap(fresh_root, legacy_root):
                raise ValueError("fresh root aliases or contains legacy storage")
    if _overlap(new_runner["data_dir"], new_bash["state_root"]):
        raise ValueError("fresh Runner and Bash roots overlap")
    return {
        "legacy_runner_path": str(legacy_runner_path),
        "legacy_data_dir": str(old_runner["data_dir"]),
        "legacy_bash_state_root": str(old_bash["state_root"]),
        "fresh_data_dir": str(new_runner["data_dir"]),
        "fresh_bash_state_root": str(new_bash["state_root"]),
    }


def _source_file(path: Path) -> tuple[int, os.stat_result]:
    _safe_path(path, require_root=False, directory=False)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        os.close(fd)
        raise ValueError(f"source is not a single-link regular file: {path}")
    return fd, info


def _copy_exact(source: Path, destination: Path, mode: int) -> dict[str, object]:
    source_fd, before = _source_file(source)
    try:
        destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        target_fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
        try:
            with os.fdopen(os.dup(source_fd), "rb") as reader, os.fdopen(os.dup(target_fd), "wb") as writer:
                shutil.copyfileobj(reader, writer)
                writer.flush()
            os.fchmod(target_fd, mode)
            os.fsync(target_fd)
            copied = os.fstat(target_fd)
        finally:
            os.close(target_fd)
        after = os.fstat(source_fd)
        identity = lambda info: (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
        if identity(before) != identity(after):
            raise ValueError(f"source changed while staging: {source}")
        os.lseek(source_fd, 0, os.SEEK_SET)
        with os.fdopen(os.dup(source_fd), "rb") as stream:
            original_hash = hashlib.file_digest(stream, "sha256").hexdigest()
        if identity(before) != identity(os.fstat(source_fd)):
            raise ValueError(f"source changed during digest readback: {source}")
        with destination.open("rb") as stream:
            copied_hash = hashlib.file_digest(stream, "sha256").hexdigest()
        if original_hash != copied_hash or before.st_size != copied.st_size:
            raise ValueError(f"copy changed while staging: {source}")
        return {
            "sha256": copied_hash,
            "size": copied.st_size,
            "device": copied.st_dev,
            "inode": copied.st_ino,
        }
    finally:
        os.close(source_fd)


def _sync_tree(root: Path) -> None:
    for directory, _, _ in os.walk(root, topdown=False):
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)


def verify(destination: Path, *, require_root: bool = False) -> dict:
    _safe_path(destination, require_root=require_root, directory=True)
    manifest_path = destination / MANIFEST
    _safe_path(manifest_path, require_root=require_root, directory=False)
    with manifest_path.open("rb") as stream:
        record = json.load(stream)
    if (set(record) != {"schema", "generation", "admission", "activation", "roots", "assets"}
            or record["schema"] != 1 or record["admission"] != "closed"
            or record["activation"] != "inert-no-selector"
            or str(uuid.UUID(record["generation"])) != record["generation"]
            or set(record["assets"]) != set(ASSETS)):
        raise ValueError("incompatible inert staging manifest")
    for name, (relative, mode) in ASSETS.items():
        path = destination / relative
        _safe_path(path, require_root=require_root, directory=False)
        info = path.stat()
        item = record["assets"][name]
        if (set(item) != {"path", "source_path", "sha256", "size", "device", "inode"}
                or item["path"] != relative or info.st_nlink != 1
                or stat.S_IMODE(info.st_mode) != mode
                or (info.st_size, info.st_dev, info.st_ino) != (item["size"], item["device"], item["inode"])):
            raise ValueError(f"staged asset changed: {name}")
        _absolute_config_path(item["source_path"], f"{name} source_path")
        with path.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != item["sha256"]:
                raise ValueError(f"staged asset digest changed: {name}")
    staged = {name: destination / relative for name, (relative, _) in ASSETS.items()}
    legacy_runner_path = _absolute_config_path(record["roots"]["legacy_runner_path"], "legacy_runner_path")
    if _check_configs(staged, destination, legacy_runner_path) != record["roots"]:
        raise ValueError("staged configuration changed")
    return record


def stage(destination: Path, sources: dict[str, Path], *, require_root: bool = False) -> dict:
    if set(sources) != set(ASSETS):
        raise ValueError("all ten legacy/fresh images and configs are required")
    if not destination.is_absolute() or ".." in destination.parts:
        raise ValueError("destination must be an absolute normalized path")
    if require_root and os.geteuid() != 0:
        raise PermissionError("root required for installed staging")
    parent = destination.parent
    _safe_path(parent, require_root=require_root, directory=True)
    roots = _check_configs(sources, destination, sources["legacy_runner"])
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(destination)
    lock_path = parent / ".age319-stage.lock"
    lock_fd = os.open(lock_path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
        if destination.exists() or destination.is_symlink():
            raise FileExistsError(destination)
        pending = parent / f".stage-{uuid.uuid4()}"
        pending.mkdir(mode=0o700)
        try:
            assets = {}
            for name, (relative, mode) in ASSETS.items():
                image = _copy_exact(sources[name], pending / relative, mode)
                assets[name] = {"path": relative, "source_path": str(sources[name]), **image}
            record = {
                "schema": 1,
                "generation": str(uuid.uuid4()),
                "admission": "closed",
                "activation": "inert-no-selector",
                "roots": roots,
                "assets": assets,
            }
            manifest_fd = os.open(pending / MANIFEST, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o400)
            try:
                body = (json.dumps(record, sort_keys=True) + "\n").encode()
                with os.fdopen(os.dup(manifest_fd), "wb") as stream:
                    stream.write(body)
                    stream.flush()
                os.fsync(manifest_fd)
            finally:
                os.close(manifest_fd)
            _sync_tree(pending)
            # A locked, root-owned parent excludes unprivileged publication races.
            # This is only a directory name publication, never a route selector.
            _rename_new(pending, destination)
            parent_fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                os.fsync(parent_fd)
            finally:
                os.close(parent_fd)
            if verify(destination, require_root=require_root) != record:
                raise ValueError("staged generation readback changed")
            return record
        finally:
            if pending.exists():
                shutil.rmtree(pending)
    finally:
        os.close(lock_fd)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--require-root", action="store_true")
    for name in ASSETS:
        parser.add_argument("--" + name.replace("_", "-"), type=Path, required=True)
    args = parser.parse_args()
    sources = {name: getattr(args, name) for name in ASSETS}
    result = stage(args.destination, sources, require_root=args.require_root)
    print(json.dumps({"generation": result["generation"], "admission": result["admission"], "destination": str(args.destination)}))


if __name__ == "__main__":
    main()
