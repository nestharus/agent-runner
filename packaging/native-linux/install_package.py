#!/usr/bin/python3 -I
"""Install, plan or uninstall one native-root package (run as root).

    install_package.py plan|install --archive A --sha256 HEX --user NAME
        [--prefix /opt/oulipoly-native] [--etc /etc/oulipoly-native]
        [--run-base /var/lib/oulipoly-native/runs]
        [--sudoers /etc/sudoers.d/oulipoly-native]
    install_package.py uninstall --record <prefix>/<id>.install.json [--purge-site]

`install` checks the archive digest, extracts it member by member (regular
files, directories and in-package relative symlinks only) into a fresh
`<prefix>/.<id>.partial`, root-owned with the archive's normalized modes,
checks every file against `MANIFEST.json`, and renames it to
`<prefix>/<id>`. It then makes the run base (`0711`) if absent, writes the
site config from `share/frontdoor.example.json` (only if absent: an
existing one is left and reported), and writes the sudoers rule for
exactly this package's front door and `--user`, checked by `visudo -cf`
before it is put in place (`0440`; an existing file is refused). Every
path it created, and its digest, goes to `<prefix>/<id>.install.json`.

`plan` prints the same effects and changes nothing. `uninstall` removes
exactly what the record names, where it is unchanged: the sudoers file,
the package directory and the record; with `--purge-site`, also the site
config it wrote and the run base if empty. Nothing is started; no service
is enabled. The old `/usr/local/libexec/oulipoly` path is refused as a
prefix.

`--dest-root DIR` (tests) puts every path under DIR; with
`--unprivileged-test` ownership is left to the caller and `visudo` is
skipped when absent.
"""

import argparse
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tarfile

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
        return os.path.join(self.dest, path.lstrip("/")) if self.dest != "/" else path


def members(tar, package_id):
    """The archive's members, refused unless each is a directory, regular
    file or in-package relative symlink below `package_id/`."""
    checked = []
    for member in tar.getmembers():
        name = os.path.normpath(member.name)
        if name != package_id and not name.startswith(package_id + "/"):
            raise Stop(f"archive member {member.name!r} is outside {package_id}")
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
        return first.name.strip("/")


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
        elif sha256_file(path) != entry["sha256"]:
            raise Stop(f"{rel}: digest differs from MANIFEST.json")
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


def make_dirs(path, mode, unprivileged, created):
    missing = []
    current = path
    while not os.path.lexists(current):
        missing.append(current)
        current = os.path.dirname(current)
    for directory in reversed(missing):
        os.mkdir(directory, 0o755)
        if not unprivileged:
            os.chown(directory, 0, 0)
        os.chmod(directory, 0o755)
        created.append(directory)
    if missing:
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
            {"create": os.path.join(paths.prefix, package_id + ".install.json"), "mode": "0644"},
        ],
        "not_done": ["no service, no old canonical path, no Broker, no existing file replaced, nothing started"],
    }


def install(args, paths):
    unprivileged = args.unprivileged_test
    if sha256_file(args.archive) != args.sha256:
        raise Stop("archive digest differs from --sha256")
    package_id = package_id_of(args.archive)
    if args.command == "plan":
        print(json.dumps(plan(args, paths, package_id), indent=1))
        return 0
    if not unprivileged and os.geteuid() != 0:
        raise Stop("install runs as root")
    sudoers = paths.host(paths.sudoers)
    if not os.path.isdir(os.path.dirname(sudoers)):
        raise Stop(f"{os.path.dirname(paths.sudoers)} is not a directory")
    if os.path.lexists(sudoers) or os.path.lexists(sudoers + ".partial"):
        raise Stop(f"{paths.sudoers} exists; not replaced")
    created = []
    prefix = paths.host(paths.prefix)
    make_dirs(prefix, 0o755, unprivileged, created)
    final = os.path.join(prefix, package_id)
    record_path = final + ".install.json"
    if os.path.lexists(final) or os.path.lexists(record_path):
        raise Stop(f"{final} is already installed")
    partial = os.path.join(prefix, "." + package_id + ".partial")
    if os.path.lexists(partial):
        raise Stop(f"{partial} is left from an earlier attempt; inspect and remove it")
    extract(args.archive, package_id, partial, unprivileged)
    manifest = verify(partial)
    os.rename(partial, final)
    created.append(final)
    record = {"id": package_id, "archive_sha256": args.sha256, "package": os.path.join(paths.prefix, package_id),
              "manifest_runner": manifest["runner"], "manifest_agent_bash": manifest["agent_bash"],
              "created": [], "files": {}, "left": []}

    run_base = paths.host(paths.run_base)
    if os.path.lexists(run_base):
        record["left"].append({"path": paths.run_base, "why": "exists"})
    else:
        make_dirs(run_base, 0o711, unprivileged, created)

    etc = paths.host(paths.etc)
    config = os.path.join(etc, "frontdoor.json")
    if os.path.lexists(config):
        record["left"].append({"path": os.path.join(paths.etc, "frontdoor.json"), "why": "exists"})
    else:
        make_dirs(etc, 0o755, unprivileged, created)
        data = site_config(os.path.join(final, "share/frontdoor.example.json"), args.user, paths.run_base)
        write_new(config, data, 0o644, unprivileged)
        record["files"][os.path.join(paths.etc, "frontdoor.json")] = sha256_bytes(data)

    rule = sudoers_rule(os.path.join(final, "share/sudoers.template"), os.path.join(paths.prefix, package_id, FRONTDOOR), args.user)
    staged = sudoers + ".partial"
    write_new(staged, rule, 0o440, unprivileged)
    if os.path.exists(VISUDO):
        checked = subprocess.run([VISUDO, "-cf", staged], capture_output=True, text=True)
        if checked.returncode != 0:
            os.unlink(staged)
            raise Stop(f"visudo refused the rule: {checked.stdout.strip()} {checked.stderr.strip()}")
        record["sudoers_checked"] = "visudo -cf"
    elif not unprivileged:
        os.unlink(staged)
        raise Stop(f"no {VISUDO}; the rule is not installed unchecked")
    else:
        record["sudoers_checked"] = "skipped (unprivileged test, no visudo)"
    os.rename(staged, sudoers)
    record["files"][paths.sudoers] = sha256_bytes(rule)
    record["created"] = [
        "/" + os.path.relpath(path, paths.dest) if paths.dest != "/" else path for path in created if path != final
    ]
    write_new(record_path, (json.dumps(record, indent=1, sort_keys=True) + "\n").encode(), 0o644, unprivileged)
    print(json.dumps({"installed": record["package"], "record": os.path.join(paths.prefix, package_id + ".install.json"),
                      "frontdoor": os.path.join(record["package"], FRONTDOOR)}))
    return 0


def uninstall(args, paths):
    if not args.unprivileged_test and os.geteuid() != 0:
        raise Stop("uninstall runs as root")
    record_path = paths.host(args.record)
    with open(record_path, encoding="utf-8") as file:
        record = json.load(file)
    removed, left = [], []
    sudoers = [path for path in record["files"] if path.startswith("/etc/sudoers") or path == paths.sudoers]
    for path in sudoers:
        host = paths.host(path)
        if os.path.lexists(host) and sha256_file(host) == record["files"][path]:
            os.unlink(host)
            removed.append(path)
        else:
            left.append({"path": path, "why": "absent or changed"})
    package = paths.host(record["package"])
    if os.path.isdir(package) and not os.path.islink(package):
        shutil.rmtree(package)
        removed.append(record["package"])
    os.unlink(record_path)
    removed.append(args.record)
    if args.purge_site:
        for path, digest in record["files"].items():
            if path in sudoers:
                continue
            host = paths.host(path)
            if os.path.lexists(host) and sha256_file(host) == digest:
                os.unlink(host)
                removed.append(path)
            else:
                left.append({"path": path, "why": "absent or changed"})
        for path in sorted(record["created"], key=len, reverse=True):
            host = paths.host(path)
            try:
                os.rmdir(host)
                removed.append(path)
            except OSError:
                left.append({"path": path, "why": "not empty or absent"})
    print(json.dumps({"removed": removed, "left": left}, indent=1))
    return 0


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
    except Stop as stop:
        print(json.dumps({"stopped": str(stop)}), file=sys.stderr)
        return 3


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
