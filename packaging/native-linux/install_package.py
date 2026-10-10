#!/usr/bin/python3 -I
"""Plan, install or recover/remove one Linux native ACP v2 package.

Plan performs transient private-copy writes, then reports destination
effects. Installation writes a cleanup record before private-copy or
persistent effects, validates resolved destination custody, hashes only
its private archive copy, verifies the extracted package and enables the
recorded visudo-checked rule last. Failures leave the exact record for
explicit recovery; uninstall preserves changed content and live uses,
and keeps its record until removals finish. No service/runtime is started.

    install_package.py plan|install --archive A --sha256 HEX --user NAME
    install_package.py uninstall --record RECORD [--purge-site]

Administrative paths and unprivileged dest-root seam: --help.
Full effects/failure paths and limitations: share/README.md.
"""

import argparse
import hashlib
import json
import os
import fcntl
import re
import secrets
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile

OLD_CANONICAL = "/usr/local/libexec/oulipoly"
VISUDO = "/usr/sbin/visudo"
FRONTDOOR = "libexec/oulipoly-native-frontdoor"


class Stop(Exception):
    pass


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


class Paths:
    def __init__(self, args):
        self.dest = os.path.realpath(args.dest_root) if args.dest_root else "/"
        for name in ("prefix", "etc", "run_base", "sudoers"):
            value = getattr(args, name)
            if not value.startswith("/"):
                raise Stop(f"--{name.replace('_', '-')} must be absolute")
        prefix = os.path.normpath(args.prefix)
        if prefix == OLD_CANONICAL or prefix.startswith(OLD_CANONICAL + "/"):
            raise Stop("the old canonical path is not a native package prefix")
        self.prefix = prefix
        self.etc = os.path.normpath(args.etc)
        self.run_base = os.path.normpath(args.run_base)
        self.sudoers = os.path.normpath(args.sudoers)

    def host(self, path):
        """Where `path` (as installed) is on this filesystem."""
        if not path.startswith("/") or os.path.normpath(path) != path:
            raise Stop("record contains a noncanonical path")
        return os.path.join(self.dest, path.lstrip("/")) if self.dest != "/" else path


def members(tar, package_id):
    """The archive's members, refused unless each is a directory, regular
    file or in-package relative symlink below `package_id/`."""
    checked = []
    for member in tar.getmembers():
        name = os.path.normpath(member.name)
        if name != package_id and not name.startswith(package_id + "/"):
            raise Stop(f"archive member {member.name!r} is outside {package_id}")
        if name != member.name.rstrip("/") or name in {n for n, _ in checked}:
            raise Stop("noncanonical or duplicate archive member")
        if member.isdir() or member.isfile():
            pass
        elif member.issym():
            target = os.path.normpath(os.path.join(os.path.dirname(name), member.linkname))
            if member.linkname.startswith("/") or not (target == package_id or target.startswith(package_id + "/")):
                raise Stop(f"archive symlink {member.name!r} leaves the package")
        else:
            raise Stop(f"archive member {member.name!r} has an unsupported type")
        checked.append((name, member))
    return checked


def package_id_of(archive):
    with tarfile.open(archive, "r:gz") as tar:
        first = tar.next()
        if first is None or not first.isdir() or "/" in first.name.strip("/"):
            raise Stop("archive does not start with its package directory")
        value = first.name.strip("/")
        if not re.fullmatch(r"oulipoly-native-linux-x86_64-[A-Za-z0-9-]+", value):
            raise Stop("invalid package id")
        return value


def extract(archive, package_id, partial, unprivileged):
    os.mkdir(partial, 0o700)
    with tarfile.open(archive, "r:gz") as tar:
        for name, member in members(tar, package_id):
            rel = os.path.relpath(name, package_id)
            path = partial if rel == "." else os.path.join(partial, rel)
            if member.isdir():
                if rel != ".":
                    os.mkdir(path, 0o700)
            elif member.isfile():
                # No extraction through a previously extracted symlink.
                parent = os.path.dirname(path)
                while parent != partial:
                    if os.path.islink(parent):
                        raise Stop("archive has a symlink directory ancestor")
                    parent = os.path.dirname(parent)
                source = tar.extractfile(member)
                fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                with os.fdopen(fd, "wb") as out:
                    shutil.copyfileobj(source, out)
                os.chmod(path, 0o755 if member.mode & 0o111 else 0o644)
            else:
                os.symlink(member.linkname, path)
            if not unprivileged:
                os.lchown(path, 0, 0)
    # Directories last, so a partly written tree stays closed.
    for directory, dirs, _ in os.walk(partial):
        os.chmod(directory, 0o755)


def verify(root):
    with open(os.path.join(root, "MANIFEST.json"), encoding="utf-8") as file:
        manifest = json.load(file)
    seen = set()
    for directory, dirs, files in os.walk(root):
        for name in files + [d for d in dirs if os.path.islink(os.path.join(directory, d))]:
            rel = os.path.relpath(os.path.join(directory, name), root)
            if rel != "MANIFEST.json":
                seen.add(rel)
    expected = set(manifest["files"])
    if seen != expected:
        raise Stop(f"package files differ from MANIFEST.json ({len(seen ^ expected)} paths)")
    for rel, entry in manifest["files"].items():
        path = os.path.join(root, rel)
        if "symlink" in entry:
            if os.readlink(path) != entry["symlink"]:
                raise Stop(f"{rel}: symlink differs from MANIFEST.json")
        elif os.path.islink(path) or not stat.S_ISREG(os.lstat(path).st_mode) or sha256_file(path) != entry["sha256"]:
            raise Stop(f"{rel}: digest differs from MANIFEST.json")
        if "mode" in entry and oct(os.lstat(path).st_mode & 0o7777) != entry["mode"]:
            raise Stop(f"{rel}: mode differs from MANIFEST.json")
    return manifest


def site_config(example, user, run_base):
    with open(example, encoding="utf-8") as file:
        config = json.load(file)
    config["allowed_users"] = [user]
    config["run_base"] = run_base
    return (json.dumps(config, indent=1, sort_keys=True) + "\n").encode()


def sudoers_rule(template, frontdoor, user):
    with open(template, encoding="utf-8") as file:
        text = file.read()
    return text.replace("@FRONTDOOR@", frontdoor).replace("@USER@", user).encode()


def write_new(path, data, mode, unprivileged):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "wb") as file:
        file.write(data)
    if not unprivileged:
        os.chown(path, 0, 0)
    os.chmod(path, mode)


def plan(args, paths, package_id):
    installed = os.path.join(paths.prefix, package_id)
    frontdoor = os.path.join(installed, FRONTDOOR)
    config = os.path.join(paths.etc, "frontdoor.json")
    return {
        "package": installed,
        "frontdoor": frontdoor,
        "effects": [
            {"create": installed, "owner": "root:root", "modes": "dirs 0755, executables 0755, files 0644"},
            {"create-if-absent": paths.run_base, "mode": "0711", "parents": "0755"},
            {"create-if-absent": config, "mode": "0644", "from": "share/frontdoor.example.json", "allowed_users": [args.user]},
            {"create": paths.sudoers, "mode": "0440", "rule": f"{args.user} ALL=(root) NOPASSWD: {frontdoor} run", "checked_by": "visudo -cf"},
            {"create": os.path.join(paths.prefix, package_id + ".install.json"), "mode": "0600"},
        ],
        "transient_writes": "private 0700 temporary directory and 0600 archive copy, removed after reading; plan does not modify destinations",
        "recovery": "bootstrap record in nearest existing prefix ancestor, moved to <id>.install.json; rule enabled last; failed effects recovered only on explicit uninstall",
        "not_done": ["no service, no old canonical path, no Broker, no existing file replaced, nothing started"],
    }


def install(args, paths):
    test = args.unprivileged_test
    if args.command == "install" and not test and os.geteuid() != 0:
        raise Stop("install runs as root")
    if not re.fullmatch(r"[a-f0-9]{64}", args.sha256):
        raise Stop("sha256 must be 64 lowercase hex characters")
    journal = None
    if args.command == "plan":
        private = tempfile.mkdtemp(prefix="oulipoly-native-plan-", dir=paths.dest if paths.dest != "/" else None)
    else:
        for configured in (paths.prefix, paths.etc, paths.run_base, paths.sudoers):
            custody(paths.host(configured), test)
        if os.path.lexists(paths.host(paths.sudoers)) or os.path.lexists(paths.host(paths.sudoers) + ".partial"):
            raise Stop("sudoers or its stage exists; not replaced")
        prefix = paths.host(paths.prefix)
        custody(prefix, test)
        parent = prefix
        while not os.path.isdir(parent):
            parent = os.path.dirname(parent)
        record_path = os.path.join(parent, ".oulipoly-native-" + args.sha256[:12] + ".install.json")
        private = os.path.join(parent, ".oulipoly-native-copy-" + secrets.token_hex(8))
        private_installed = "/" + os.path.relpath(private, paths.dest) if paths.dest != "/" else private
        data = {"v": 2, "id": "copy-" + args.sha256[:12], "package": "", "run_base": paths.run_base,
                "sudoers": paths.sudoers, "status": "copying", "archive_sha256": args.sha256,
                "effects": [{"kind": "private-copy", "path": private_installed, "state": "planned"}]}
        synced_json(record_path, data, new=True)
        journal = Record(record_path, data)
        print(json.dumps({"recovery_record": record_path, "state": "copying"}), flush=True)
    try:
        if journal is not None:
            os.mkdir(private, 0o700)
            journal.done(journal.data["effects"][0], private)
        archive = os.path.join(private, "package.tar.gz")
        if private_copy(args.archive, archive) != args.sha256:
            raise Stop("archive digest differs from --sha256")
        package_id = package_id_of(archive)
        if args.command == "plan":
            print(json.dumps(plan(args, paths, package_id), indent=1))
            return 0
        return install_from(args, paths, archive, package_id, journal)
    except BaseException as error:
        if journal is None:
            raise
        journal.data.update(status="failed-or-interrupted", failure=type(error).__name__)
        journal.save()
        print(json.dumps({"stopped": type(error).__name__, "recovery_record": journal.path}), file=sys.stderr)
        return 3
    finally:
        try:
            if os.path.lexists(private):
                shutil.rmtree(private)
            if journal is not None:
                journal.data["effects"][0]["state"] = "removed"
                journal.save()
        except OSError as error:
            if journal is not None:
                print(json.dumps({"stopped": "private-copy-cleanup-failed", "reason": type(error).__name__, "recovery_record": journal.path}), file=sys.stderr)
                return 3
            else:
                raise



def private_copy(source, target):
    """Copies `source` to the new file `target` (`0600`) and returns the
    sha256 of exactly the bytes written."""
    digest = hashlib.sha256()
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with open(source, "rb") as src, os.fdopen(fd, "wb") as out:
        for chunk in iter(lambda: src.read(1 << 20), b""):
            digest.update(chunk)
            out.write(chunk)
    return digest.hexdigest()


def custody(path, unprivileged=False):
    """Validate literal and resolved existing ancestry before root effects.
    Test mode skips ownership only, and stays below its private dest-root."""
    existing = path
    while not os.path.lexists(existing):
        existing = os.path.dirname(existing)
    for start in (existing, os.path.realpath(existing)):
        current = start
        while True:
            st = os.lstat(current)
            if not unprivileged and (st.st_uid != 0 or (st.st_mode & 0o022 and not stat.S_ISLNK(st.st_mode))):
                raise Stop("destination ancestry is not root custody")
            if current == "/":
                break
            current = os.path.dirname(current)
    if os.path.lexists(path) and os.path.islink(path):
        raise Stop("destination is a symlink")


def synced_json(path, value, new=False):
    data = (json.dumps(value, indent=1, sort_keys=True) + "\n").encode()
    if new:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "wb") as file:
            file.write(data)
            file.flush()
            os.fsync(file.fileno())
    else:
        staged = path + ".writing"
        fd = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            with os.fdopen(fd, "wb") as file:
                file.write(data)
                file.flush()
                os.fsync(file.fileno())
            os.replace(staged, path)
        finally:
            if os.path.lexists(staged):
                os.unlink(staged)
    directory = os.open(os.path.dirname(path), os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def tree_inventory(root):
    result = {}
    for directory, dirs, files in os.walk(root):
        for name in dirs + files:
            path = os.path.join(directory, name)
            rel = os.path.relpath(path, root)
            st = os.lstat(path)
            if stat.S_ISLNK(st.st_mode):
                result[rel] = {"symlink": os.readlink(path)}
            elif stat.S_ISDIR(st.st_mode):
                result[rel] = {"directory": True}
            elif stat.S_ISREG(st.st_mode):
                result[rel] = {"sha256": sha256_file(path)}
            else:
                raise Stop("package has an unexpected object")
    return result


def expected_tree(archive, package_id):
    with tarfile.open(archive, "r:gz") as tar:
        checked = members(tar, package_id)
        manifest_bytes = tar.extractfile(package_id + "/MANIFEST.json").read()
        manifest = json.loads(manifest_bytes)
        result = {}
        for name, member in checked:
            rel = os.path.relpath(name, package_id)
            if rel == ".":
                continue
            if member.isdir():
                result[rel] = {"directory": True}
            elif member.issym():
                result[rel] = {"symlink": member.linkname}
            elif rel == "MANIFEST.json":
                result[rel] = {"sha256": sha256_bytes(manifest_bytes)}
            else:
                result[rel] = {"sha256": manifest["files"][rel]["sha256"]}
        return result


# A small write-ahead cleanup record, not an automatic rollback platform.
# A crash may leave a planned effect; recovery checks its contents before
# removal. The enabling rule is the last destination effect.
class Record:
    def __init__(self, path, data):
        self.path, self.data = path, data

    def save(self):
        synced_json(self.path, self.data)

    def prepare(self, path, kind, **fields):
        effect = {"path": path, "kind": kind, "state": "planned", **fields}
        self.data["effects"].append(effect)
        self.save()
        return effect

    def done(self, effect, host):
        st = os.lstat(host)
        effect.update(state="created", dev=st.st_dev, ino=st.st_ino)
        self.save()


def install_from(args, paths, archive, package_id, journal):
    test = args.unprivileged_test
    for path in (paths.prefix, paths.etc, paths.run_base, paths.sudoers):
        custody(paths.host(path), test)
    sudoers = paths.host(paths.sudoers)
    if not os.path.isdir(os.path.dirname(sudoers)):
        raise Stop("sudoers parent is not a directory")
    if os.path.lexists(sudoers) or os.path.lexists(sudoers + ".partial"):
        raise Stop("sudoers or its stage exists; not replaced")
    prefix = paths.host(paths.prefix)
    final = os.path.join(prefix, package_id)
    partial = os.path.join(prefix, "." + package_id + ".partial")
    record_path = final + ".install.json"
    for path in (final, partial, record_path, record_path + ".writing"):
        if os.path.lexists(path):
            raise Stop("package or prior recovery record exists; inspect its record")
    expected = expected_tree(archive, package_id)
    data = journal.data
    data.update(id=package_id, package=os.path.join(paths.prefix, package_id),
                status="installing", left=[])
    bootstrap = journal.path
    journal.save()
    def mkdirs(host, mode):
        missing = []
        current = host
        while not os.path.lexists(current):
            missing.append(current)
            current = os.path.dirname(current)
        for directory in reversed(missing):
            installed = "/" + os.path.relpath(directory, paths.dest) if paths.dest != "/" else directory
            effect = journal.prepare(installed, "directory")
            os.mkdir(directory, mode if directory == host else 0o755)
            os.chmod(directory, mode if directory == host else 0o755)
            journal.done(effect, directory)

    def file_effect(path, content, mode):
        host = paths.host(path)
        effect = journal.prepare(path, "file", sha256=sha256_bytes(content))
        write_new(host, content, mode, test)
        journal.done(effect, host)
        return effect

    try:
        mkdirs(prefix, 0o755)
        # Save the relocation before doing it; either name remains usable.
        data["record_final"] = record_path
        journal.save()
        os.rename(bootstrap, record_path)
        journal.path = record_path
        journal.save()
        print(json.dumps({"recovery_record": record_path}), flush=True)
        partial_path = os.path.join(paths.prefix, "." + package_id + ".partial")
        tree = journal.prepare(partial_path, "tree", expected=expected, partial=True,
                               alternate=data["package"])
        extract(archive, package_id, partial, test)
        manifest = verify(partial)
        journal.done(tree, partial)
        os.rename(partial, final)
        tree.update(path=data["package"], partial=False)
        journal.done(tree, final)
        data["manifest_runner"] = manifest["runner"]
        data["manifest_agent_bash"] = manifest["agent_bash"]
        mkdirs(paths.host(paths.run_base), 0o711)
        config_path = os.path.join(paths.etc, "frontdoor.json")
        config = paths.host(config_path)
        if os.path.lexists(config):
            custody(config, test)
            data["left"].append({"path": config_path, "why": "pre-existing; not owned by this install"})
        else:
            mkdirs(paths.host(paths.etc), 0o755)
            file_effect(config_path, site_config(os.path.join(final, "share/frontdoor.example.json"), args.user, paths.run_base), 0o644)
        staged_path = paths.sudoers + ".partial"
        rule = sudoers_rule(os.path.join(final, "share/sudoers.template"), os.path.join(data["package"], FRONTDOOR), args.user)
        effect = file_effect(staged_path, rule, 0o440)
        if os.path.exists(VISUDO):
            checked = subprocess.run([VISUDO, "-cf", paths.host(staged_path)], capture_output=True, timeout=10)
            if checked.returncode:
                raise Stop("visudo refused the rule")
            data["sudoers_checked"] = "visudo -cf (syntax only)"
        elif not test:
            raise Stop("visudo absent; rule not enabled")
        else:
            data["sudoers_checked"] = "skipped: unprivileged test"
        # Last enabling effect is already described durably at BOTH names.
        effect["alternate"] = paths.sudoers
        data["status"] = "ready-to-enable"
        journal.save()
        os.rename(paths.host(staged_path), sudoers)
        effect["path"] = paths.sudoers
        journal.done(effect, sudoers)
        data["status"] = "installed"
        journal.save()
        print(json.dumps({"installed": data["package"], "record": record_path,
                          "frontdoor": os.path.join(data["package"], FRONTDOOR)}))
        return 0
    except BaseException as error:
        data["status"] = "failed-or-interrupted"
        data["failure"] = type(error).__name__
        try:
            journal.save()
        except OSError:
            pass  # The last write-ahead version still describes effects.
        print(json.dumps({"stopped": type(error).__name__, "recovery_record": journal.path,
                          "action": "inspect then uninstall --record with --purge-site"}), file=sys.stderr)
        return 3


LOSS_ACCOUNTS = "loss-accounts"
# Both durable kinds produced by this package's frontdoor. Pending writes
# (.next-*) and future/unknown kinds deliberately remain outside the inventory.
LOSS_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}\.json(?P<physical>\.retired)?")


def loss_record_kind(name):
    match = LOSS_NAME.fullmatch(name)
    if match is None:
        raise Stop("unknown loss-account object")
    return "physical-only-disposition" if match["physical"] else "semantic-account"


def loss_evidence_report(path, files, disposition):
    kinds = [loss_record_kind(os.path.basename(file)) for file in files]
    return {"directory": path, "accounts": kinds.count("semantic-account"),
            "physical_dispositions": kinds.count("physical-only-disposition"),
            "disposition": disposition}


def managed_loss_directory(path, unprivileged=False):
    """Exact managed type/custody. Evidence is not a live run or settlement."""
    custody(path, unprivileged)
    st = os.lstat(path)
    if not stat.S_ISDIR(st.st_mode) or stat.S_IMODE(st.st_mode) != 0o700:
        raise Stop("changed loss-account directory")
    entries = list(os.scandir(path))
    for entry in entries:
        custody(entry.path, unprivileged)
        st = entry.stat(follow_symlinks=False)
        loss_record_kind(entry.name)
        if not stat.S_ISREG(st.st_mode) or stat.S_IMODE(st.st_mode) != 0o600:
            raise Stop("unknown loss-account object")
    return [entry.path for entry in entries]


def live_runs(base, unprivileged=False):
    """Live/unknown locks prevent removal; no PID markers or proc scans."""
    live = []
    if not os.path.isdir(base):
        return live
    for user in os.scandir(base):
        if not user.is_dir(follow_symlinks=False):
            live.append(user.path)
            continue
        for run in os.scandir(user.path):
            try:
                if run.name == LOSS_ACCOUNTS and user.name.isdigit():
                    managed_loss_directory(run.path, unprivileged)
                    continue
                if not run.is_dir(follow_symlinks=False):
                    raise OSError
                private = os.path.join(run.path, "private")
                if os.path.islink(private):
                    raise OSError
                fd = os.open(os.path.join(private, "lock"), os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                finally:
                    os.close(fd)
            except (Stop, OSError):
                live.append(run.path)
    return live


def purge_loss_accounts(journal, loss_dirs):
    """Only after disabling the owned entry, explicitly dispose exact managed
    evidence. A journal intention is never an observation or authority."""
    # Explicit write-ahead disposition; only checked managed objects.
    disposition = journal.data.setdefault("loss_account_disposition", [])
    current_files = {file for _, files in loss_dirs for file in files}
    by_path = {item["path"]: item for item in disposition}
    for file in sorted(current_files):
        if file not in by_path:
            item = {"path": file, "state": "purge-intended", "meaning": "evidence deletion, not settlement"}
            disposition.append(item)
            by_path[file] = item
        # Kind comes from the current checked inventory, not old journal
        # claims or record bodies. It grants no settlement/drain authority.
        by_path[file]["kind"] = loss_record_kind(os.path.basename(file))
    for item in disposition:
        if item["state"] == "purge-intended" and item["path"] not in current_files:
            item["state"] = "absent-after-purge-intent; deletion-unconfirmed"
    journal.save()
    # Previous journal paths never grant deletion authority: only the
    # current exact managed inventory can be removed.
    for file in sorted(current_files):
        os.unlink(file)
        by_path[file]["state"] = "purged-not-settled"
        journal.save()
    for path, _ in loss_dirs:
        os.rmdir(path)
        user_dir = os.path.dirname(path)
        if not os.listdir(user_dir):
            os.rmdir(user_dir)
    return [loss_evidence_report(path, files, "purged-not-settled")
            for path, files in loss_dirs]


def uninstall_locked(args, paths, record_path, record):
    test = args.unprivileged_test
    active = live_runs(paths.host(record["run_base"]), test)
    if active:
        print(json.dumps({"stopped": "live-or-unknown-runs", "runs": active, "record_retained": record_path}))
        return 3
    removed, left = [], []
    journal = Record(record_path, record)
    loss_dirs = []
    base = paths.host(record["run_base"])
    if os.path.isdir(base):
        for user in os.scandir(base):
            path = os.path.join(user.path, LOSS_ACCOUNTS)
            if user.name.isdigit() and os.path.lexists(path):
                loss_dirs.append((path, managed_loss_directory(path, test)))
    evidence = [loss_evidence_report(path, files, "preserved across package removal")
                for path, files in loss_dirs]
    purge_pending = args.purge_site
    if purge_pending and not any(os.path.lexists(paths.host(path)) for path in (record["sudoers"], record["sudoers"] + ".partial")):
        evidence = purge_loss_accounts(journal, loss_dirs)
        purge_pending = False
    # Disabling the owned rule first; package removal cannot leave an entry
    # enabled. Changed rules stop removal and retain the record.
    effects = sorted(record["effects"], key=lambda e: (0 if e.get("alternate") == record["sudoers"] or e["path"] == record["sudoers"] else 1,
                                                      -len(e["path"])))
    for effect in effects:
        if effect.get("state") == "removed":
            continue
        if not args.purge_site and effect["kind"] not in ("tree", "private-copy") and effect["path"] not in (record["sudoers"], record["sudoers"] + ".partial"):
            left.append({"path": effect["path"], "why": "purge-site not selected; record retained"})
            continue
        candidates = [effect["path"]]
        if effect.get("alternate") and effect["alternate"] not in candidates:
            candidates.append(effect["alternate"])
        remaining = False
        for path in candidates:
            host = paths.host(path)
            if not os.path.lexists(host):
                continue
            try:
                custody(host, test)
                st = os.lstat(host)
                if effect["kind"] == "private-copy":
                    if not stat.S_ISDIR(st.st_mode) or st.st_mode & 0o077 or (effect.get("ino") is not None and (st.st_ino, st.st_dev) != (effect["ino"], effect["dev"])):
                        raise Stop("changed private copy directory")
                    entries = list(os.scandir(host))
                    if any(e.name != "package.tar.gz" or not e.is_file(follow_symlinks=False) for e in entries):
                        raise Stop("private copy contains an unexpected object")
                    shutil.rmtree(host)
                elif effect["kind"] == "file":
                    if not stat.S_ISREG(st.st_mode) or sha256_file(host) != effect["sha256"]:
                        raise Stop("changed file")
                    os.unlink(host)
                elif effect["kind"] == "tree":
                    if not stat.S_ISDIR(st.st_mode):
                        raise Stop("changed package type")
                    actual = tree_inventory(host)
                    expected = effect["expected"]
                    if (not effect.get("partial") and actual != expected) or any(expected.get(k) != v for k, v in actual.items()):
                        raise Stop("changed package or partial tree")
                    if not shutil.rmtree.avoids_symlink_attacks:
                        raise Stop("safe tree removal unavailable")
                    shutil.rmtree(host)
                else:
                    if not stat.S_ISDIR(st.st_mode) or (effect.get("ino") is not None and (st.st_ino, st.st_dev) != (effect["ino"], effect["dev"])):
                        raise Stop("changed directory")
                    # The record lives in prefix: remove that directory last.
                    if record_path.startswith(host + "/"):
                        child = os.path.relpath(record_path, host).split("/")[0]
                        if set(os.listdir(host)) != {child}:
                            raise Stop("record ancestor not empty")
                        effect["state"] = "record-parent-pending"
                        journal.save()
                        continue
                    os.rmdir(host)
                removed.append(path)
            except (Stop, OSError) as error:
                remaining = True
                left.append({"path": path, "why": str(error) if isinstance(error, Stop) else type(error).__name__})
        if not remaining:
            if effect.get("state") != "record-parent-pending":
                effect["state"] = "removed"
            journal.save()
        elif effect["path"] in (record["sudoers"], record["sudoers"] + ".partial"):
            break
        if purge_pending and not any(os.path.lexists(paths.host(path)) for path in (record["sudoers"], record["sudoers"] + ".partial")):
            evidence = purge_loss_accounts(journal, loss_dirs)
            purge_pending = False
    record["status"] = "removal-incomplete" if left else "removals-finished"
    record["cleanup_left"] = left
    journal.save()
    if not left:
        # Move the record into an existing ancestor before removing its own
        # parent. Keep it until every owned directory removal is finished.
        pending = [e for e in effects if e.get("state") == "record-parent-pending"]
        if pending:
            top_parent = min((paths.host(e["path"]) for e in pending), key=len)
            recovery = os.path.join(os.path.dirname(top_parent), "." + record["id"] + ".removal.json")
            if os.path.lexists(recovery):
                raise Stop("removal recovery record already exists")
            os.rename(record_path, recovery)
            journal.path = recovery
            record_path = recovery
            try:
                for e in sorted(pending, key=lambda e: -len(e["path"])):
                    os.rmdir(paths.host(e["path"]))
                    e["state"] = "removed"
                    journal.save()
            except OSError:
                print(json.dumps({"stopped": "directory-removal-failed", "record_retained": recovery}))
                return 3
        # Other parent directories may still have been left solely because
        # they contain the record's parent: retry only recorded empty dirs.
        os.unlink(record_path)
        removed.append(args.record)
    print(json.dumps({"removed": removed, "left": left, "record_retained": record_path if left else None,
                      "loss_accounts": evidence}, indent=1))
    return 3 if left else 0


def uninstall(args, paths):
    if not args.unprivileged_test and os.geteuid() != 0:
        raise Stop("uninstall runs as root")
    record_path = paths.host(args.record)
    custody(record_path, args.unprivileged_test)
    with open(record_path, encoding="utf-8") as file:
        record = json.load(file)
    if record.get("v") != 2:
        raise Stop("old record schema: inspect manually; no automatic removal")
    # Directory flock also excludes in-flight admission, before a run's
    # private lock exists. It holds through retirement/removal.
    package = paths.host(record["package"]) if record["package"] else None
    package_lock = None
    try:
        if package is not None and os.path.lexists(package):
            custody(package, args.unprivileged_test)
            package_lock = os.open(package, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
            try:
                fcntl.flock(package_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                print(json.dumps({"stopped": "package-in-use-or-admitting", "record_retained": record_path}))
                return 3
        return uninstall_locked(args, paths, record_path, record)
    finally:
        if package_lock is not None:
            os.close(package_lock)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("command", choices=("plan", "install", "uninstall"))
    parser.add_argument("--archive")
    parser.add_argument("--sha256")
    parser.add_argument("--user")
    parser.add_argument("--record")
    parser.add_argument("--purge-site", action="store_true")
    parser.add_argument("--prefix", default="/opt/oulipoly-native")
    parser.add_argument("--etc", default="/etc/oulipoly-native")
    parser.add_argument("--run-base", default="/var/lib/oulipoly-native/runs")
    parser.add_argument("--sudoers", default="/etc/sudoers.d/oulipoly-native")
    parser.add_argument("--dest-root")
    parser.add_argument("--unprivileged-test", action="store_true")
    args = parser.parse_args(argv)
    if args.unprivileged_test and not args.dest_root:
        parser.error("--unprivileged-test needs --dest-root")
    try:
        paths = Paths(args)
        if args.command == "uninstall":
            if not args.record:
                parser.error("uninstall needs --record")
            return uninstall(args, paths)
        if not (args.archive and args.sha256 and args.user):
            parser.error(f"{args.command} needs --archive, --sha256 and --user")
        if args.user == "root" or not args.user:
            raise Stop("--user must be a non-root user")
        return install(args, paths)
    except (Stop, OSError, ValueError, KeyError, TypeError, tarfile.TarError, subprocess.SubprocessError) as stop:
        print(json.dumps({"stopped": str(stop) if isinstance(stop, Stop) else type(stop).__name__}), file=sys.stderr)
        return 3


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
