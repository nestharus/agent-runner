#!/usr/bin/env python3
"""Move one served first-host install aside, intact, before a new first install.

This is a single-campaign operator for the fixed-path schema-2 install made by
install_first_host_v30.py. It never deletes, rewrites, replays or resets the old
State, images or unit. Each is renamed without replacement into one root-only
preservation directory on the same filesystem, so bytes and device/inode
identities stay intact and the old activation record stays valid for restore.
The new pair then uses the unchanged first-install install/activate/start path
with a new empty State. The old served State is never offered to activation.

Fixture roots prove only filesystem and operator control flow. systemd, the
Broker service and the host process census have no fixture mode here; tests
substitute a simulated service manager.
"""

import argparse
import ctypes
import hashlib
import json
import os
import stat
import subprocess
import sys
from pathlib import Path

from first_install_v30 import _read_package, _sync_dir
from install_first_host_v30 import check, paths, write_exclusive


UNIT_NAME = "oulipoly-kernel-broker.service"
RUNTIME = Path("/run/oulipoly-kernel-broker")
PRE_STOP = "pre-stop-v1.json"
BASELINE = "baseline-v1.json"
# Rename order: State first, the unit last. Restore uses the reverse order.
ITEMS = ("state", "images", "unit")
CHANGES = []


class Refusal(Exception):
    pass


def layout(fixture_root, old_generation):
    image_dir, unit, state = paths(fixture_root)
    runtime = RUNTIME if fixture_root is None else fixture_root / RUNTIME.relative_to("/")
    preserved = state.parent / ("oulipoly-age319-preserved-" + old_generation)
    return {"state": state, "images": image_dir, "unit": unit, "runtime": runtime,
            "preserved": preserved,
            "moved": {"state": preserved / "state",
                      "images": preserved / "libexec-oulipoly",
                      "unit": preserved / UNIT_NAME}}


def _hash(path, info):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        opened = os.fstat(fd)
        if (opened.st_dev, opened.st_ino) != (info.st_dev, info.st_ino):
            raise Refusal(f"file changed while hashing: {path}")
        digest = hashlib.sha256()
        while chunk := os.read(fd, 1 << 20):
            digest.update(chunk)
        return digest.hexdigest()
    finally:
        os.close(fd)


def capture(root: Path):
    """Exact lstat tree: type, owner, mode, links, device/inode, file bytes.

    ctime/atime and directory mtimes are not identity: a rename changes the
    moved inode's ctime and its parents' mtimes.
    """
    tree = {}
    pending = [Path(".")]
    while pending:
        rel = pending.pop()
        path = root / rel
        info = os.lstat(path)
        entry = {"mode": oct(stat.S_IMODE(info.st_mode)), "uid": info.st_uid,
                 "gid": info.st_gid, "device": info.st_dev, "inode": info.st_ino,
                 "nlink": info.st_nlink}
        if stat.S_ISDIR(info.st_mode):
            entry["type"] = "dir"
            pending.extend(rel / name for name in sorted(os.listdir(path)))
        elif stat.S_ISREG(info.st_mode):
            entry.update(type="file", size=info.st_size, mtime_ns=info.st_mtime_ns,
                         sha256=_hash(path, info))
        elif stat.S_ISLNK(info.st_mode):
            entry.update(type="symlink", target=os.readlink(path))
        else:
            entry["type"] = "other:" + oct(stat.S_IFMT(info.st_mode))
        tree[str(rel)] = entry
    return tree


def _exists(path: Path):
    return path.exists() or path.is_symlink()


def locations(lay):
    found = {}
    for item in ITEMS:
        original, moved = _exists(lay[item]), _exists(lay["moved"][item])
        found[item] = ("both" if original and moved else "original" if original
                       else "preserved" if moved else "absent")
    return found


def rename_noreplace(source: Path, destination: Path):
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.renameat2(-100, os.fsencode(source), -100, os.fsencode(destination), 1) != 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error), destination)
    _sync_dir(source.parent)
    _sync_dir(destination.parent)


def read_record(path: Path):
    return json.loads(path.read_bytes())


def write_record(path: Path, value):
    write_exclusive(path, (json.dumps(value, sort_keys=True, indent=1) + "\n").encode(), 0o400)
    _sync_dir(path.parent)


class SystemdHost:
    """The live service manager and host process table. Root only."""

    def show(self):
        result = subprocess.run(
            ["systemctl", "show", "-p", "MainPID", "-p", "ActiveState", "-p", "SubState",
             "-p", "ControlGroup", "-p", "UnitFileState", UNIT_NAME],
            check=True, capture_output=True, text=True)
        values = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
        values["MainPID"] = int(values.get("MainPID", "0"))
        return values

    def stop(self):
        # A synchronous stop job. With KillMode=process it signals only the
        # main PID; the drain census below decides whether anything remains.
        subprocess.run(["systemctl", "stop", UNIT_NAME], check=True)

    def pids(self):
        return [int(name) for name in os.listdir("/proc") if name.isdigit()]


def census(host, image_ids, roots):
    """Identity census, no signals: exe inode, unit cgroup, or paths held.

    Any process the census cannot read is an unknown actor, never ignored.
    """
    actors, unknown = [], []
    prefixes = [str(root) for root in roots]
    for pid in host.pids():
        if pid == os.getpid():
            continue
        base = Path("/proc") / str(pid)
        try:
            starttime = (base / "stat").read_text().rsplit(")", 1)[1].split()[19]
            reasons = []
            try:
                exe = os.stat(base / "exe")
                if (exe.st_dev, exe.st_ino) in image_ids:
                    reasons.append("image:" + image_ids[(exe.st_dev, exe.st_ino)])
            except FileNotFoundError:
                pass  # kernel thread or zombie: no executable mapping
            for line in (base / "cgroup").read_text().splitlines():
                if line.split(":", 2)[-1].split("/")[-1:] == [UNIT_NAME] or \
                        ("/" + UNIT_NAME + "/") in line:
                    reasons.append("cgroup")
                    break
            links = [base / "cwd", base / "root"]
            links += [base / "fd" / name for name in os.listdir(base / "fd")]
            for link in links:
                try:
                    target = os.readlink(link)
                except FileNotFoundError:
                    continue
                if link.name != "root" and any(
                        target == prefix or target.startswith(prefix + "/")
                        for prefix in prefixes):
                    reasons.append("holds:" + target)
        except (FileNotFoundError, ProcessLookupError):
            continue  # exited during the census
        except OSError as error:
            unknown.append({"pid": pid, "error": str(error)})
            continue
        if reasons:
            actors.append({"pid": pid, "starttime": starttime, "reasons": sorted(set(reasons))})
    return {"actors": actors, "unknown": unknown}


def roots(lay):
    return [lay["state"], lay["runtime"], lay["preserved"]]


def recorded_image_ids(pre_stop):
    return {(device, inode): name for device, inode, name in pre_stop["image_ids"]}


def image_identities(lay):
    ids = {}
    for name in os.listdir(lay["images"]):
        info = os.lstat(lay["images"] / name)
        if stat.S_ISREG(info.st_mode):
            ids[(info.st_dev, info.st_ino)] = name
    return ids


def verify_packages(old_package, new_package, old_generation, new_generation):
    old, _ = _read_package(old_package)
    new, _ = _read_package(new_package)
    if old["generation"] != old_generation:
        raise Refusal("old package generation differs from --old-generation")
    if new["generation"] != new_generation or new_generation == old_generation:
        raise Refusal("new package generation differs from --new-generation")
    return {"old": old, "new": new,
            "old_sha256": hashlib.sha256(old_package.read_bytes()).hexdigest(),
            "new_sha256": hashlib.sha256(new_package.read_bytes()).hexdigest()}


def verify_old_install(lay, old_package, fixture_root, source_generation, old_generation):
    """Exact old install at the fixed paths: bytes, pair, State source."""
    try:
        check(old_package, fixture_root)
    except (OSError, ValueError) as error:
        raise Refusal(f"installed images/unit are not the exact old package: {error}")
    state = lay["state"]
    info = os.lstat(state)
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid():
        raise Refusal("State root is not a directory owned by this operator")
    # EmptyV30BootstrapIdentity records state.db, not its containing root:
    # fresh_lane.rs:176-203; readback checks the bound file at :246-264.
    state_info = os.lstat(state / "state.db")
    if (not stat.S_ISREG(state_info.st_mode) or state_info.st_uid != os.geteuid()
            or state_info.st_nlink != 1 or stat.S_IMODE(state_info.st_mode) & 0o077):
        raise Refusal("State file is not regular owner-only storage owned by this operator")
    source = read_record(state / "empty-v30-bootstrap-v1.json")
    activation = read_record(state / "first-install-activation-v1.json")
    if (source["source_generation"] != source_generation
            or (source["state_device"], source["state_inode"]) != (state_info.st_dev, state_info.st_ino)
            or activation["source"] != source
            or activation["pair_generation"] != old_generation):
        raise Refusal("State is not the recorded served first install of the old pair")
    stages = [name for name in os.listdir(state.parent) if name.startswith(".empty-v30-bootstrap-")]
    stages += [name for name in os.listdir(lay["images"].parent)
               if name.startswith(".oulipoly-first-install-")]
    stages += [name for name in os.listdir(state)
               if name.startswith(".first-install-activation-")]
    if stages:
        raise Refusal(f"interrupted install/bootstrap/activation stage present: {stages}")
    for item in ITEMS:
        if os.lstat(lay[item]).st_dev != os.lstat(state.parent).st_dev:
            raise Refusal(f"{item} is on another filesystem; rename cannot preserve it")
    return {"source": source, "activation_pair_generation": activation["pair_generation"]}


def require_drained(host, lay, image_ids, pre_stop):
    shown = host.show()
    broker = pre_stop["service"]["MainPID"]
    try:
        starttime = (Path("/proc") / str(broker) / "stat").read_text().rsplit(")", 1)[1].split()[19]
    except FileNotFoundError:
        starttime = None
    counted = census(host, image_ids, roots(lay))
    if (shown["ActiveState"] not in ("inactive", "failed") or shown["MainPID"] != 0
            or starttime == pre_stop["broker_starttime"]
            or counted["actors"] or counted["unknown"]):
        raise Refusal("Broker drain not confirmed: "
                      + json.dumps({"service": shown, "census": counted}, sort_keys=True))
    return {"service": shown, "census": counted}


def pre_stop_checks(host, lay, image_ids, broker_pid):
    shown = host.show()
    counted = census(host, image_ids, roots(lay))
    if shown["ActiveState"] != "active" or shown["MainPID"] != broker_pid:
        raise Refusal(f"service is not the expected live Broker {broker_pid}: {shown}")
    broker = [actor for actor in counted["actors"] if actor["pid"] == broker_pid]
    others = [actor for actor in counted["actors"] if actor["pid"] != broker_pid]
    if (counted["unknown"] or others or len(broker) != 1
            or "image:oulipoly-kernel-broker" not in broker[0]["reasons"]):
        raise Refusal("other or unknown actor, or unexpected Broker identity: "
                      + json.dumps(counted, sort_keys=True))
    runtime = sorted(os.listdir(lay["runtime"])) if _exists(lay["runtime"]) else None
    return {"service": shown, "census": counted, "broker_starttime": broker[0]["starttime"],
            "runtime_listing": runtime}


def status(args, host, fixture_root):
    lay = layout(fixture_root, args.old_generation)
    packages = verify_packages(args.old_package, args.new_package,
                               args.old_generation, args.new_generation)
    found = locations(lay)
    report = {"locations": found, "preservation_root": str(lay["preserved"]),
              "old_package_sha256": packages["old_sha256"],
              "new_package_sha256": packages["new_sha256"],
              "pre_stop_record": _exists(lay["preserved"] / PRE_STOP),
              "baseline_record": _exists(lay["preserved"] / BASELINE),
              "verdict": "not ready"}
    if report["baseline_record"]:
        baseline = read_record(lay["preserved"] / BASELINE)
        # Once new objects occupy the original paths ("both"), compare the
        # preserved old objects. Never substitute the new install for them.
        comparison_paths = {
            item: lay["moved"][item] if where in ("preserved", "both") else lay[item]
            for item, where in found.items()}
        report["comparison_paths"] = {item: str(path) for item, path in comparison_paths.items()}
        report["matches_baseline"] = {
            item: _exists(path) and capture(path) == baseline["trees"][item]
            for item, path in comparison_paths.items()}
        if (all(where in ("preserved", "both") for where in found.values())
                and all(report["matches_baseline"].values())):
            report["verdict"] = "preserved"
            report["preservation_verdict"] = "preserved old State, images and unit match baseline"
        else:
            report["preservation_verdict"] = "old preservation incomplete or differs from baseline"
    if report["pre_stop_record"]:
        ids = recorded_image_ids(read_record(lay["preserved"] / PRE_STOP))
    elif found["images"] == "original":
        ids = image_identities(lay)
    else:
        ids = {}
    report["census_image_names"] = sorted(ids.values())
    report["service"] = host.show()
    report["census"] = census(host, ids, roots(lay))
    if set(found.values()) == {"original"} and not _exists(lay["preserved"]):
        # The same checks preserve runs before it stops anything; read only.
        try:
            verify_old_install(lay, args.old_package, fixture_root,
                               args.source_generation, args.old_generation)
            pre_stop_checks(host, lay, ids, args.broker_pid)
            report["readiness"] = "ready"
            report["verdict"] = "ready"
        except (Refusal, OSError, ValueError, KeyError) as error:
            report["readiness"] = f"preserve would refuse: {error}"
    return report


def preserve(args, host, fixture_root):
    lay = layout(fixture_root, args.old_generation)
    root = lay["preserved"]
    packages = verify_packages(args.old_package, args.new_package,
                               args.old_generation, args.new_generation)
    found = locations(lay)
    if "both" in found.values() or "absent" in found.values():
        raise Refusal(f"unexpected item locations: {found}")
    if not _exists(root):
        if set(found.values()) != {"original"}:
            raise Refusal(f"items moved without a preservation root: {found}")
        verified = verify_old_install(lay, args.old_package, fixture_root,
                                      args.source_generation, args.old_generation)
        image_ids = image_identities(lay)
        pre_stop = pre_stop_checks(host, lay, image_ids, args.broker_pid)
        pre_stop.update(verified=verified,
                        image_ids=[[*key, name] for key, name in image_ids.items()],
                        live_state_snapshot=capture(lay["state"]))
        os.mkdir(root, 0o700)
        _sync_dir(root.parent)
        CHANGES.append(f"created preservation root {root}")
        write_record(root / PRE_STOP, pre_stop)
        host.stop()
        CHANGES.append(f"stopped {UNIT_NAME} (known Broker {args.broker_pid})")
    elif not _exists(root / PRE_STOP):
        raise Refusal(f"unknown preservation root without {PRE_STOP}: {root}")
    pre_stop = read_record(root / PRE_STOP)
    image_ids = recorded_image_ids(pre_stop)
    drained = require_drained(host, lay, image_ids, pre_stop)
    if not _exists(root / BASELINE):
        if set(found.values()) != {"original"}:
            raise Refusal(f"items moved without a baseline: {found}")
        verified = verify_old_install(lay, args.old_package, fixture_root,
                                      args.source_generation, args.old_generation)
        write_record(root / BASELINE, {
            "schema": 1, "old_generation": args.old_generation,
            "new_generation": args.new_generation,
            "old_package_sha256": packages["old_sha256"],
            "new_package_sha256": packages["new_sha256"],
            "verified": verified, "drained": drained,
            "trees": {item: capture(lay[item]) for item in ITEMS},
            "original_paths": {item: str(lay[item]) for item in ITEMS}})
        CHANGES.append(f"captured quiescent baseline {root / BASELINE}")
    baseline = read_record(root / BASELINE)
    for item in ITEMS:
        if found[item] == "original":
            if capture(lay[item]) != baseline["trees"][item]:
                raise Refusal(f"{item} changed since the quiescent baseline")
            require_drained(host, lay, image_ids, pre_stop)
            rename_noreplace(lay[item], lay["moved"][item])
            CHANGES.append(f"moved {lay[item]} -> {lay['moved'][item]}")
        if capture(lay["moved"][item]) != baseline["trees"][item]:
            raise Refusal(f"preserved {item} differs from the quiescent baseline")
    return {"preserved": {item: str(lay["moved"][item]) for item in ITEMS},
            "baseline": str(root / BASELINE), "drained": drained}


def restore(args, host, fixture_root):
    """Rename the preserved install back. Refuses once a new install exists."""
    lay = layout(fixture_root, args.old_generation)
    root = lay["preserved"]
    if not _exists(root / BASELINE):
        raise Refusal("no quiescent baseline to restore")
    baseline = read_record(root / BASELINE)
    pre_stop = read_record(root / PRE_STOP)
    image_ids = recorded_image_ids(pre_stop)
    require_drained(host, lay, image_ids, pre_stop)
    found = locations(lay)
    for item in reversed(ITEMS):
        if found[item] == "preserved":
            if capture(lay["moved"][item]) != baseline["trees"][item]:
                raise Refusal(f"preserved {item} differs from the quiescent baseline")
            rename_noreplace(lay["moved"][item], lay[item])
            CHANGES.append(f"moved {lay['moved'][item]} -> {lay[item]}")
        elif found[item] != "original":
            raise Refusal(f"cannot restore {item}: location {found[item]}")
        if capture(lay[item]) != baseline["trees"][item]:
            raise Refusal(f"{item} at its original path is not the preserved install")
    return {"restored": {item: str(lay[item]) for item in ITEMS},
            "service": "left stopped; start it separately only if restoration is chosen"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("status", "preserve", "restore"))
    parser.add_argument("old_package", type=Path)
    parser.add_argument("new_package", type=Path)
    parser.add_argument("--old-generation", required=True)
    parser.add_argument("--new-generation", required=True)
    parser.add_argument("--source-generation", required=True)
    parser.add_argument("--broker-pid", type=int, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        raise PermissionError("root required for the fixed-path preserved reinstall")
    result = globals()[args.command](args, SystemdHost(), None)
    print(json.dumps(result, sort_keys=True))
    return 1 if args.command == "status" and result["verdict"] not in ("ready", "preserved") else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (Refusal, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"preserved reinstall refused: {error}", file=sys.stderr)
        print("administrative changes made by this invocation: "
              + json.dumps(CHANGES), file=sys.stderr)
        sys.exit(1)
