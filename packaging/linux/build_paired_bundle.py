#!/usr/bin/env python3
"""Stage a versioned, inert Linux pair. Extraction is not activation."""

import argparse
import hashlib
import io
import json
import tarfile
import tomllib
import uuid
from pathlib import Path


def build(runner: Path, broker: Path, launcher: Path, service: Path, output: Path, version: str, bash: Path | None = None) -> dict:
    runner_bytes = runner.read_bytes()
    broker_bytes = broker.read_bytes()
    launcher_bytes = launcher.read_bytes()
    runner_hash = hashlib.sha256(runner_bytes).hexdigest()
    broker_hash = hashlib.sha256(broker_bytes).hexdigest()
    launcher_hash = hashlib.sha256(launcher_bytes).hexdigest()
    manifest = {
        "schema": 1,
        "version": version,
        "generation": str(uuid.uuid5(uuid.NAMESPACE_URL, f"oulipoly-pair-v1:{version}:{runner_hash}:{broker_hash}:{launcher_hash}")),
        "runner_sha256": runner_hash,
        "broker_sha256": broker_hash,
        "launcher_sha256": launcher_hash,
    }
    files = {
        "usr/local/libexec/oulipoly/oulipoly-agent-runner": (runner_bytes, 0o755),
        "usr/local/libexec/oulipoly/oulipoly-kernel-broker": (broker_bytes, 0o755),
        "usr/local/libexec/oulipoly/oulipoly-installed-launcher": (launcher_bytes, 0o755),
        "usr/local/libexec/oulipoly/install-v1.json": ((json.dumps(manifest, sort_keys=True) + "\n").encode(), 0o644),
        "etc/systemd/system/oulipoly-kernel-broker.service": (service.read_bytes(), 0o644),
        "usr/share/applications/oulipoly-plane.desktop": (
            b"[Desktop Entry]\nType=Application\nName=Oulipoly Plane\nExec=/usr/local/bin/oulipoly-plane\nTerminal=false\nCategories=Development;\n", 0o644
        ),
    }
    if bash is not None:
        bash_bytes = bash.read_bytes()
        bash_hash = hashlib.sha256(bash_bytes).hexdigest()
        manifest.update(
            schema=2,
            generation=str(uuid.uuid5(uuid.NAMESPACE_URL, f"oulipoly-pair-v2:{version}:{runner_hash}:{broker_hash}:{launcher_hash}:{bash_hash}")),
            bash_sha256=bash_hash,
        )
        files["usr/local/libexec/oulipoly/agent-bash"] = (bash_bytes, 0o755)
    files["usr/local/libexec/oulipoly/install-v1.json"] = ((json.dumps(manifest, sort_keys=True) + "\n").encode(), 0o644)
    links = {
        "usr/local/bin/agents": "../libexec/oulipoly/oulipoly-installed-launcher",
        "usr/local/bin/oulipoly-agent-runner": "../libexec/oulipoly/oulipoly-installed-launcher",
        "usr/local/bin/oulipoly-plane": "../libexec/oulipoly/oulipoly-installed-launcher",
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(output, "w:gz") as archive:
        for name, (body, mode) in files.items():
            item = tarfile.TarInfo(name)
            item.size, item.mode, item.uid, item.gid, item.mtime = len(body), mode, 0, 0, 0
            archive.addfile(item, io.BytesIO(body))
        for name, target in links.items():
            item = tarfile.TarInfo(name)
            item.type, item.linkname, item.mode = tarfile.SYMTYPE, target, 0o777
            item.uid = item.gid = item.mtime = 0
            archive.addfile(item)
    return manifest


def verify(archive_path: Path) -> dict:
    with tarfile.open(archive_path, "r:gz") as archive:
        members = {member.name: member for member in archive.getmembers()}
        base = "usr/local/libexec/oulipoly/"
        manifest = json.load(archive.extractfile(members[base + "install-v1.json"]))
        images = [("oulipoly-agent-runner", "runner_sha256"), ("oulipoly-kernel-broker", "broker_sha256"), ("oulipoly-installed-launcher", "launcher_sha256")]
        if manifest["schema"] == 2:
            images.append(("agent-bash", "bash_sha256"))
        else:
            assert manifest["schema"] == 1 and "bash_sha256" not in manifest and base + "agent-bash" not in members
        for name, key in images:
            assert hashlib.sha256(archive.extractfile(members[base + name]).read()).hexdigest() == manifest[key]
        assert members["etc/systemd/system/oulipoly-kernel-broker.service"].isfile()
        for name in ("agents", "oulipoly-agent-runner", "oulipoly-plane"):
            link = members["usr/local/bin/" + name]
            assert link.issym() and link.linkname == "../libexec/oulipoly/oulipoly-installed-launcher"
        return manifest


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--broker", type=Path, required=True)
    parser.add_argument("--launcher", type=Path, required=True)
    parser.add_argument("--bash", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    with (root / "Cargo.toml").open("rb") as source:
        version = tomllib.load(source)["workspace"]["package"]["version"]
    manifest = build(args.runner, args.broker, args.launcher, root / "packaging/linux/oulipoly-kernel-broker.service", args.output, version, args.bash)
    assert verify(args.output) == manifest
    print(f"staged {args.output}: version={version} generation={manifest['generation']}")


if __name__ == "__main__":
    main()
