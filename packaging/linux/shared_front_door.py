#!/usr/bin/env python3
"""Prepare and publish the closed AGE-319 shared front door; never install live here.

The stable root is separate from the fixed v1 pair. Alias adoption is a distinct,
sequential operation; epoch zero permits old routes while adoption is incomplete.
"""

import argparse
import fcntl
import hashlib
import json
import os
import shutil
import socket
import stat
import uuid
from pathlib import Path

from stage_versioned_island import _rename_new, _safe_path, _sync_tree, verify


ROLES = ("runner", "runner_alt", "gui", "bash")
LAUNCHER = Path("launcher/oulipoly-shared-front-door")
ALIAS_NAMES = {"runner": "agents", "runner_alt": "oulipoly-agent-runner",
               "gui": "oulipoly-plane", "bash": "agent-bash"}


def _alias_owner(alias: Path, *, require_root: bool = False) -> int:
    """An exact local alias may be owned by its local user, never by another user."""
    _absolute(alias)
    info = alias.lstat()
    if not stat.S_ISLNK(info.st_mode) or alias.name not in ALIAS_NAMES.values():
        raise ValueError(f"managed alias must be an exact symlink: {alias}")
    owner = info.st_uid
    if alias.parent.lstat().st_uid != owner:
        raise ValueError(f"managed alias owner differs from parent: {alias}")
    for part in (alias.parent, *alias.parent.parents):
        entry = part.lstat()
        if (stat.S_ISLNK(entry.st_mode) or entry.st_uid not in (0, owner)
                or require_root and entry.st_mode & 0o022):
            raise ValueError(f"untrusted managed alias parent: {part}")
    return owner


def _alias_link(alias: Path) -> str:
    # The retained root-owned package uses a relative ../libexec link. Its
    # exact text and resolved old image are both recorded before adoption.
    return os.readlink(alias)


def _adopted(alias: Path, launcher: Path, owner: int, *, require_root: bool = False) -> bool:
    return (_alias_owner(alias, require_root=require_root) == owner
            and _alias_link(alias) == str(launcher))


def _digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _write_sync(path: Path, body: bytes, mode: int = 0o400) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    try:
        remaining = memoryview(body)
        while remaining:
            written = os.write(fd, remaining)
            if written <= 0:
                raise OSError("short front-door file write")
            remaining = remaining[written:]
        os.fchmod(fd, mode)
        os.fsync(fd)
    finally:
        os.close(fd)


def _json_bytes(value: dict) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def _sync_dir(path: Path) -> None:
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def _absolute(path: Path) -> Path:
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError(f"absolute normalized path required: {path}")
    return path


def _uuid(value: str) -> str:
    if str(uuid.UUID(value)) != value:
        raise ValueError("noncanonical generation identity")
    return value


def broker_readback(path: Path, image: Path, *, require_root: bool) -> dict[str, str]:
    """Read the existing broker's read-only I response, with a peer-UID check."""
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as peer:
        peer.settimeout(5)
        peer.connect(str(path))
        import struct
        pid, uid, _ = struct.unpack("3i", peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
        if require_root and uid != 0:
            raise ValueError("broker peer is not host root")
        if require_root:
            named = image.stat()
            running = Path(f"/proc/{pid}/exe").stat()
            if (named.st_dev, named.st_ino) != (running.st_dev, running.st_ino):
                raise ValueError("live broker image differs from generation")
        challenge = peer.recv(16, socket.MSG_WAITALL)
        if len(challenge) != 16:
            raise ValueError("short broker challenge")
        peer.sendall(b"I" + challenge)
        response = bytearray()
        while len(response) <= 256:
            chunk = peer.recv(257 - len(response))
            if not chunk:
                break
            response.extend(chunk)
        if len(response) > 256 or not response.endswith(b"\n"):
            raise ValueError("invalid broker route response")
    fields = response.decode().strip().split(" ")
    if len(fields) != 4 or fields[0] != "fresh-v30-route":
        raise ValueError("fresh broker route unavailable")
    return dict(zip(("lane_id", "source_generation", "domain_id"), map(_uuid, fields[1:])))


def prepare(root: Path, stage: Path, launcher: Path, aliases: dict[str, Path | None],
            broker_image: Path, broker_socket: Path, broker_route: dict[str, str],
            *, require_root: bool = False, adapter_bindings: dict[str, Path] | None = None,
            adapter_disabled: bool = False) -> dict:
    """Create a private, immutable generation and stable image; leave aliases alone."""
    for path in (root, stage, launcher, broker_image, broker_socket,
                 *(alias for alias in aliases.values() if alias is not None)):
        _absolute(path)
    if broker_socket == Path("/run/oulipoly-kernel-broker/control.sock") or broker_image == Path("/usr/local/libexec/oulipoly/oulipoly-kernel-broker"):
        raise ValueError("fresh broker must not reuse the fixed v1 image or socket")
    if set(aliases) != set(ROLES) or set(broker_route) != {"lane_id", "source_generation", "domain_id"}:
        raise ValueError("all alias roles and exact broker route are required")
    if any(aliases[role] is None for role in ("runner", "runner_alt", "bash")):
        raise ValueError("Runner and Bash aliases are required")
    if require_root and (adapter_bindings is None) == (not adapter_disabled):
        raise ValueError("declare exact adapter bindings or explicitly declare adapter disabled")
    if adapter_bindings is not None:
        if set(adapter_bindings) != {"runner", "bash"} or any(
                adapter_bindings[role] != aliases[role] for role in ("runner", "bash")):
            raise ValueError("adapter entries must be the inventoried Runner and Bash aliases")
    present_aliases = [alias for alias in aliases.values() if alias is not None]
    if len(set(present_aliases)) != len(present_aliases):
        raise ValueError("duplicate managed alias")
    old_alias_targets = {}
    old_alias_hashes = {}
    old_alias_owners = {}
    old_alias_links = {}
    alias_owners = {}
    for name, alias in aliases.items():
        if alias is None:
            old_alias_targets[name] = old_alias_hashes[name] = None
            old_alias_owners[name] = None
            old_alias_links[name] = alias_owners[name] = None
            continue
        if alias.name != ALIAS_NAMES[name]:
            raise ValueError(f"wrong managed alias name: {alias}")
        alias_owners[name] = _alias_owner(alias, require_root=require_root)
        old_alias_links[name] = _alias_link(alias)
        old_alias_targets[name] = str(alias.resolve(strict=True))
        old_alias_owners[name] = Path(old_alias_targets[name]).stat().st_uid
        old_alias_hashes[name] = _digest(Path(old_alias_targets[name]))
    for value in broker_route.values():
        _uuid(value)
    if require_root and os.geteuid() != 0:
        raise PermissionError("root required")
    if require_root:
        _safe_path(broker_image, require_root=True, directory=False)
        if not broker_image.stat().st_mode & 0o111:
            raise ValueError("installed fresh broker image is not executable")
    _safe_path(root.parent, require_root=require_root, directory=True)
    if (root / "install-v1.json").exists() or root.name == "oulipoly":
        raise ValueError("v2 front door cannot coexist in the fixed v1 pair root")
    if root.exists() or root.is_symlink():
        raise FileExistsError(root)
    stage_record = verify(stage, require_root=require_root)
    generation = stage_record["generation"]
    stage_manifest_hash = _digest(stage / "staging-v1.json")
    assets = stage_record["assets"]
    if _digest(broker_image) != assets["fresh_broker"]["sha256"]:
        raise ValueError("running broker image differs from staged broker")
    old_runner = Path(stage_record["roots"]["legacy_runner_path"])
    old_bash = Path(assets["legacy_bash"]["source_path"])
    if _digest(old_runner) != assets["legacy_runner"]["sha256"] or _digest(old_bash) != assets["legacy_bash"]["sha256"]:
        raise ValueError("historical source image changed")
    old_runner_config = old_runner.parent / "config.toml"
    old_bash_config = old_bash.parent / "agent-bash.toml"
    if (_digest(old_runner_config) != assets["legacy_runner_config"]["sha256"]
            or _digest(old_bash_config) != assets["legacy_bash_config"]["sha256"]):
        raise ValueError("historical adjacent config differs from staged config")
    if not stat.S_ISREG(launcher.stat().st_mode) or launcher.is_symlink():
        raise ValueError("launcher source must be a regular image")
    if require_root and b"OULIPOLY_AGE319_FRONT_DOOR_FIXTURE_ONLY_UNSAFE" in launcher.read_bytes():
        raise ValueError("fixture launcher cannot be prepared for installed use")
    pending = root.parent / f".front-door-{uuid.uuid4()}"
    pending.mkdir(mode=0o755)
    try:
        launcher_target = pending / LAUNCHER
        launcher_target.parent.mkdir(mode=0o755)
        shutil.copyfile(launcher, launcher_target)
        launcher_target.chmod(0o555)
        with launcher_target.open("rb") as stream:
            os.fsync(stream.fileno())
        generation_dir = pending / "generations" / generation
        generation_dir.mkdir(parents=True, mode=0o755)
        fresh = pending / "images" / "fresh"
        for name, relative, mode in (
            ("fresh_runner", "runner/oulipoly-agent-runner", 0o444),
            ("fresh_runner_config", "runner/config.toml", 0o444),
            ("fresh_bash", "bash/agent-bash", 0o444),
            ("fresh_bash_config", "bash/agent-bash.toml", 0o444),
        ):
            target = fresh / relative
            target.parent.mkdir(parents=True, exist_ok=True, mode=0o755)
            shutil.copyfile(stage / assets[name]["path"], target)
            target.chmod(mode)
            with target.open("rb") as stream:
                os.fsync(stream.fileno())
            if _digest(target) != assets[name]["sha256"]:
                raise ValueError("fresh image copy changed")
        record = {
            "schema": 2,
            "generation": generation,
            "stage_path": str(stage),
            "stage_manifest_sha256": stage_manifest_hash,
            "launcher_sha256": _digest(launcher_target),
            "aliases": {name: str(aliases[name]) if aliases[name] is not None else None for name in ROLES},
            "alias_owner_uids": alias_owners,
            "legacy_alias_links": old_alias_links,
            "adapter_bindings": {role: str(path) for role, path in adapter_bindings.items()}
                                if adapter_bindings is not None else None,
            "legacy_alias_targets": old_alias_targets,
            "legacy_alias_sha256": old_alias_hashes,
            "legacy_alias_owner_uids": old_alias_owners,
            "legacy_runner_entry": str(old_runner),
            "legacy_runner_sha256": assets["legacy_runner"]["sha256"],
            "legacy_runner_owner_uid": old_runner.stat().st_uid,
            "legacy_runner_config": str(old_runner_config),
            "legacy_runner_config_sha256": assets["legacy_runner_config"]["sha256"],
            "legacy_runner_config_owner_uid": old_runner_config.stat().st_uid,
            "legacy_bash_entry": str(old_bash),
            "legacy_bash_sha256": assets["legacy_bash"]["sha256"],
            "legacy_bash_owner_uid": old_bash.stat().st_uid,
            "legacy_bash_config": str(old_bash_config),
            "legacy_bash_config_sha256": assets["legacy_bash_config"]["sha256"],
            "legacy_bash_config_owner_uid": old_bash_config.stat().st_uid,
            "legacy_bash_state_root": stage_record["roots"]["legacy_bash_state_root"],
            "fresh_runner_image": str(root / "images/fresh/runner/oulipoly-agent-runner"),
            "fresh_runner_sha256": assets["fresh_runner"]["sha256"],
            "fresh_runner_config": str(root / "images/fresh/runner/config.toml"),
            "fresh_runner_config_sha256": assets["fresh_runner_config"]["sha256"],
            "fresh_bash_image": str(root / "images/fresh/bash/agent-bash"),
            "fresh_bash_sha256": assets["fresh_bash"]["sha256"],
            "fresh_bash_config": str(root / "images/fresh/bash/agent-bash.toml"),
            "fresh_bash_config_sha256": assets["fresh_bash_config"]["sha256"],
            "broker_image": str(broker_image),
            "broker_image_sha256": assets["fresh_broker"]["sha256"],
            "broker_socket": str(broker_socket),
            "broker_route": broker_route,
        }
        _write_sync(generation_dir / "manifest.json", _json_bytes(record), 0o444)
        _write_sync(pending / "publication.lock", b"", 0o444)
        _sync_tree(pending)
        _rename_new(pending, root)
        _sync_dir(root.parent)
        if read_generation(root, generation, require_root=require_root) != record:
            raise ValueError("prepared generation readback differs")
        return record
    finally:
        if pending.exists():
            shutil.rmtree(pending)


def read_generation(root: Path, generation: str, *, require_root: bool = False) -> dict:
    _uuid(generation)
    path = root / "generations" / generation / "manifest.json"
    _safe_path(path, require_root=require_root, directory=False)
    record = json.loads(path.read_bytes())
    if record["schema"] != 2 or record["generation"] != generation:
        raise ValueError("generation manifest invalid")
    bindings = record["adapter_bindings"]
    if bindings is not None and (set(bindings) != {"runner", "bash"} or any(
            bindings[role] != record["aliases"][role] for role in ("runner", "bash"))):
        raise ValueError("adapter binding differs from managed alias")
    stage = Path(record["stage_path"])
    staged = verify(stage, require_root=require_root)
    if staged["generation"] != generation or _digest(stage / "staging-v1.json") != record["stage_manifest_sha256"]:
        raise ValueError("staged generation changed")
    if _digest(root / LAUNCHER) != record["launcher_sha256"]:
        raise ValueError("launcher image changed")
    for key, image in (("legacy_runner_sha256", "legacy_runner_entry"),
                       ("legacy_runner_config_sha256", "legacy_runner_config"),
                       ("legacy_bash_sha256", "legacy_bash_entry"),
                       ("legacy_bash_config_sha256", "legacy_bash_config"),
                       ("fresh_runner_sha256", "fresh_runner_image"),
                       ("fresh_runner_config_sha256", "fresh_runner_config"),
                       ("fresh_bash_sha256", "fresh_bash_image"),
                       ("fresh_bash_config_sha256", "fresh_bash_config"),
                       ("broker_image_sha256", "broker_image")):
        if _digest(Path(record[image])) != record[key]:
            raise ValueError(f"{image} changed")
        owner_key = image.replace("_entry", "_owner_uid").replace("_config", "_config_owner_uid")
        if image.startswith("legacy_") and Path(record[image]).stat().st_uid != record[owner_key]:
            raise ValueError(f"{image} owner changed")
    for role in ROLES:
        if record["aliases"][role] is None:
            continue
        if _digest(Path(record["legacy_alias_targets"][role])) != record["legacy_alias_sha256"][role]:
            raise ValueError(f"legacy {role} alias target changed")
        if Path(record["legacy_alias_targets"][role]).stat().st_uid != record["legacy_alias_owner_uids"][role]:
            raise ValueError(f"legacy {role} alias target owner changed")
    if record["broker_image_sha256"] != staged["assets"]["fresh_broker"]["sha256"]:
        raise ValueError("broker image incompatible with stage")
    for key in ("fresh_runner_image", "fresh_bash_image"):
        if Path(record[key]).stat().st_mode & 0o111:
            raise ValueError("fresh image became executable")
    return record


def read_active(root: Path, *, require_root: bool = False) -> tuple[dict, str]:
    path = root / "active.json"
    _safe_path(path, require_root=require_root, directory=False)
    raw = path.read_bytes()
    record = json.loads(raw)
    if set(record) != {"schema", "epoch", "phase", "generation", "manifest_sha256", "publication_id"} \
            or record["schema"] != 2 or record["phase"] not in ("legacy", "fresh-closed") \
            or (record["phase"] == "legacy") != (record["epoch"] == 0):
        raise ValueError("active selector incompatible")
    manifest = root / "generations" / record["generation"] / "manifest.json"
    if _digest(manifest) != record["manifest_sha256"]:
        raise ValueError("active generation digest mismatch")
    read_generation(root, record["generation"], require_root=require_root)
    return record, hashlib.sha256(raw).hexdigest()


def publish(root: Path, generation: str, phase: str, expected_sha256: str | None,
            publication_id: str, *, require_root: bool = False) -> tuple[dict, str]:
    """fsynced CAS. A retry of the same publication returns exact readback."""
    if require_root and os.geteuid() != 0:
        raise PermissionError("root required")
    _safe_path(root, require_root=require_root, directory=True)
    _uuid(generation)
    _uuid(publication_id)
    if phase not in ("legacy", "fresh-closed"):
        raise ValueError("unknown phase")
    lock_path = root / "publication.lock"
    _safe_path(lock_path, require_root=require_root, directory=False)
    with lock_path.open("rb") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        current = read_active(root, require_root=require_root) if (root / "active.json").exists() else None
        if current is not None and current[0]["publication_id"] == publication_id:
            if (current[0]["generation"], current[0]["phase"]) == (generation, phase):
                return current
            raise ValueError("publication ID reused with incompatible decision")
        if current is None:
            if expected_sha256 is not None or phase != "legacy":
                raise ValueError("initial publication must be legacy epoch zero")
            epoch = 0
        else:
            epoch = current[0]["epoch"] + 1
        generation_record = read_generation(root, generation, require_root=require_root)
        if phase == "fresh-closed":
            if current is None or current[0]["phase"] != "legacy":
                raise ValueError("fresh activation requires legacy selector")
            for role, path in generation_record["aliases"].items():
                if path is None:
                    continue
                alias = Path(path)
                if not _adopted(alias, root / LAUNCHER, generation_record["alias_owner_uids"][role],
                                require_root=require_root):
                    raise ValueError(f"managed alias not adopted: {alias}")
            if broker_readback(Path(generation_record["broker_socket"]), Path(generation_record["broker_image"]), require_root=require_root) != generation_record["broker_route"]:
                raise ValueError("live broker generation incompatible")
        manifest_hash = _digest(root / "generations" / generation / "manifest.json")
        desired = {"schema": 2, "epoch": epoch, "phase": phase, "generation": generation,
                   "manifest_sha256": manifest_hash, "publication_id": publication_id}
        if current is not None and expected_sha256 != current[1]:
            raise ValueError("selector compare-and-swap lost")
        if current is not None and phase != "fresh-closed":
            raise ValueError("rollback or second legacy epoch refused")
        target = root / "active.json"
        pending = root / f".active-{uuid.uuid4()}"
        try:
            _write_sync(pending, _json_bytes(desired), 0o444)
            os.replace(pending, target)
            _sync_dir(root)
        finally:
            pending.unlink(missing_ok=True)
        observed = read_active(root, require_root=require_root)
        if observed[0] != desired:
            raise ValueError("selector publication readback differs")
        return observed


def adopt_alias(root: Path, role: str, expected_old: Path, *, require_root: bool = False) -> None:
    """One alias at a time; callers retain all preactivation late-old debt."""
    if require_root and os.geteuid() != 0:
        raise PermissionError("root required")
    if role not in ROLES:
        raise ValueError("unknown alias role")
    active, _ = read_active(root, require_root=require_root)
    if active["phase"] != "legacy":
        raise ValueError("alias adoption after activation refused")
    record = read_generation(root, active["generation"], require_root=require_root)
    if record["aliases"][role] is None:
        raise ValueError(f"alias role not inventoried: {role}")
    alias = Path(record["aliases"][role])
    if str(expected_old) != record["legacy_alias_targets"][role]:
        raise ValueError("old alias target differs from prepared plan")
    if _adopted(alias, root / LAUNCHER, record["alias_owner_uids"][role], require_root=require_root):
        return
    if (_alias_owner(alias, require_root=require_root) != record["alias_owner_uids"][role]
            or _alias_link(alias) != record["legacy_alias_links"][role]
            or alias.resolve() != expected_old):
        raise ValueError("alias old target changed")
    pending = alias.parent / f".front-door-alias-{uuid.uuid4()}"
    pending.symlink_to(root / LAUNCHER)
    try:
        if os.geteuid() == 0:
            os.lchown(pending, record["alias_owner_uids"][role], -1)
        os.replace(pending, alias)
        _sync_dir(alias.parent)
    finally:
        pending.unlink(missing_ok=True)
    if not _adopted(alias, root / LAUNCHER, record["alias_owner_uids"][role], require_root=require_root):
        raise ValueError("alias adoption readback differs")


def check(root: Path, generation: str, *, require_root: bool = False) -> dict:
    """Read back every inventoried entry, including optional GUI and adapter bindings."""
    record = read_generation(root, generation, require_root=require_root)
    active, _ = read_active(root, require_root=require_root)
    if active["generation"] != record["generation"]:
        raise ValueError("checked generation is not selected")
    aliases = {}
    for role, path in record["aliases"].items():
        if path is None:
            aliases[role] = "absent"
            continue
        alias = Path(path)
        if _adopted(alias, root / LAUNCHER, record["alias_owner_uids"][role], require_root=require_root):
            aliases[role] = "adopted"
        elif (_alias_owner(alias, require_root=require_root) == record["alias_owner_uids"][role]
              and _alias_link(alias) == record["legacy_alias_links"][role]
              and alias.resolve() == Path(record["legacy_alias_targets"][role])):
            aliases[role] = "legacy"
        else:
            raise ValueError(f"managed alias changed: {alias}")
    if active["phase"] == "fresh-closed" and "legacy" in aliases.values():
        raise ValueError("activated selector has legacy alias")
    return {"generation": record["generation"], "phase": active["phase"],
            "aliases": aliases, "adapter_bindings": record["adapter_bindings"],
            "readback": "exact"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("prepare", "read", "publish", "adopt", "check"))
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--stage", type=Path)
    parser.add_argument("--launcher", type=Path)
    parser.add_argument("--alias-runner", type=Path)
    parser.add_argument("--alias-runner-alt", type=Path)
    parser.add_argument("--alias-gui", type=Path)
    parser.add_argument("--alias-bash", type=Path)
    parser.add_argument("--adapter-runner-entry", type=Path)
    parser.add_argument("--adapter-bash-entry", type=Path)
    parser.add_argument("--adapter-disabled", action="store_true")
    parser.add_argument("--broker-image", type=Path)
    parser.add_argument("--broker-socket", type=Path)
    parser.add_argument("--broker-lane-id")
    parser.add_argument("--broker-source-generation")
    parser.add_argument("--broker-domain-id")
    parser.add_argument("--generation")
    parser.add_argument("--phase", choices=("legacy", "fresh-closed"))
    parser.add_argument("--expected-sha256")
    parser.add_argument("--publication-id")
    parser.add_argument("--role", choices=ROLES)
    parser.add_argument("--expected-old", type=Path)
    parser.add_argument("--fixture", action="store_true", help="permit a disposable non-root fixture")
    args = parser.parse_args()
    require_root = not args.fixture
    if args.command == "prepare":
        aliases = {"runner": args.alias_runner, "runner_alt": args.alias_runner_alt,
                   "gui": args.alias_gui, "bash": args.alias_bash}
        route = {"lane_id": args.broker_lane_id, "source_generation": args.broker_source_generation,
                 "domain_id": args.broker_domain_id}
        bindings = None
        if args.adapter_runner_entry is not None or args.adapter_bash_entry is not None:
            bindings = {"runner": args.adapter_runner_entry, "bash": args.adapter_bash_entry}
        record = prepare(args.root, args.stage, args.launcher, aliases, args.broker_image,
                         args.broker_socket, route, require_root=require_root,
                         adapter_bindings=bindings, adapter_disabled=args.adapter_disabled)
        print(json.dumps({"generation": record["generation"], "root": str(args.root)}))
        return
    if args.command == "adopt":
        adopt_alias(args.root, args.role, args.expected_old, require_root=require_root)
        print(json.dumps({"adopted": args.role, "root": str(args.root)}))
        return
    if args.command == "check":
        print(json.dumps(check(args.root, args.generation, require_root=require_root), sort_keys=True))
        return
    if args.command == "read":
        record, digest = read_active(args.root, require_root=require_root)
    else:
        record, digest = publish(args.root, args.generation, args.phase, args.expected_sha256,
                                 args.publication_id, require_root=require_root)
    print(json.dumps({"selector": record, "sha256": digest}, sort_keys=True))


if __name__ == "__main__":
    main()
