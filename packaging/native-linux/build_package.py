#!/usr/bin/env python3
"""Build the inert Linux x86_64 native-root package from exact sources.

    build_package.py --build-dir B --runner-repo W
        --agent-bash-repo T --agent-bash-commit C [--allow-dirty]

Everything it writes is under B (cargo home and targets, the npm cache,
the locked dependencies, the Node runtime download, the stage and
`dist/`); W and T are only read (T through `git archive C`). Network is
used only by cargo, `npm ci` (locked public dependencies) and one HTTPS GET
of the pinned official Node release, checked against its pinned sha256.
Nothing is installed, run as root or started beyond the builds; the Claude
Code and Node executables are copied and hashed, never run.

Output: `B/dist/<id>.tar.gz`, `B/dist/<id>.tar.gz.sha256` and the stage
`B/stage/<id>/` with `MANIFEST.json` (source identities, toolchain,
dependency lock, dynamic libraries of every binary, and a sha256 and mode
for every file). The archive is deterministic for the same inputs: sorted
entries, owner `0:0`, mtime the runner commit time, normalized modes.
"""

import argparse
import gzip
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile

HERE = os.path.dirname(os.path.realpath(__file__))
LOCK_DIR = "crates/oulipoly-root-supervisor/native/opencode"
CLAUDE_LOCK_DIR = "crates/oulipoly-root-supervisor/native/claude"
CLAUDE_PLATFORM = "node_modules/@anthropic-ai/claude-agent-sdk-linux-x64"
# Locked optional platform packages `npm ci` installs on Linux x64 that the
# receiver never selects (it names the glibc executable): not staged.
CLAUDE_UNUSED = ("node_modules/@anthropic-ai/claude-agent-sdk-linux-x64-musl",)
# The Node runtime of the Claude receiver: an official release, pinned.
NODE_VERSION = "v24.21.0"
NODE_TARBALL = f"node-{NODE_VERSION}-linux-x64.tar.xz"
NODE_URL = f"https://nodejs.org/dist/{NODE_VERSION}/{NODE_TARBALL}"
NODE_SHA256 = "fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6"
BASH_TOOL = "integrations/opencode/tools/bash.ts"

# (stage path, build product) for the co-located binaries.
BINARIES = (
    ("bin/oulipoly-agent-runner", "target/release/oulipoly-agent-runner"),
    ("bin/oulipoly-root-supervisor", "target/release/oulipoly-root-supervisor"),
    ("bin/oulipoly-root-pid1", "target/release/oulipoly-root-pid1"),
    ("agent-bash/agent-bash", "agent-bash-target/release/agent-bash"),
)
ASSETS = (
    ("libexec/oulipoly-native-frontdoor", "frontdoor.py", 0o755),
    ("bin/oulipoly-native-call", "native_call.py", 0o755),
    ("share/install_package.py", "install_package.py", 0o755),
    ("share/sudoers.template", "sudoers.template", 0o644),
    ("share/frontdoor.example.json", "frontdoor.example.json", 0o644),
    ("share/README.md", "README.md", 0o644),
)


def run(argv, log, **kwargs):
    print("+", " ".join(argv), file=sys.stderr, flush=True)
    with open(log, "ab") as out:
        out.write(("+ " + " ".join(argv) + "\n").encode())
        out.flush()
        done = subprocess.run(argv, stdout=out, stderr=subprocess.STDOUT, **kwargs)
        out.write(("status=" + str(done.returncode) + "\n").encode())
        out.flush()
        done.check_returncode()


def output(argv, **kwargs):
    return subprocess.run(argv, check=True, capture_output=True, text=True, **kwargs).stdout.strip()


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def runner_identity(repo, allow_dirty):
    commit = output(["git", "-C", repo, "rev-parse", "HEAD"])
    dirty = output(["git", "-C", repo, "status", "--porcelain"])
    if dirty and not allow_dirty:
        raise SystemExit("runner repo has uncommitted changes; commit them or pass --allow-dirty")
    time_ = int(output(["git", "-C", repo, "show", "-s", "--format=%ct", "HEAD"]))
    version = None
    with open(os.path.join(repo, "Cargo.toml"), encoding="utf-8") as file:
        section = None
        for line in file:
            line = line.strip()
            if line.startswith("["):
                section = line
            elif section == "[workspace.package]" and line.startswith("version"):
                version = line.split("=", 1)[1].strip().strip('"')
    return {"commit": commit, "dirty": bool(dirty), "commit_time": time_, "version": version}


def agent_bash_source(build, repo, commit):
    full = output(["git", "-C", repo, "rev-parse", "--verify", commit + "^{commit}"])
    source = os.path.join(build, "src", "agent-bash-" + full[:12])
    if not os.path.isdir(source):
        partial = source + ".partial"
        shutil.rmtree(partial, ignore_errors=True)
        os.makedirs(partial)
        archive = subprocess.Popen(["git", "-C", repo, "archive", full], stdout=subprocess.PIPE)
        subprocess.run(["tar", "-x", "-C", partial], stdin=archive.stdout, check=True)
        archive.stdout.close()
        if archive.wait() != 0:
            raise SystemExit("git archive of agent-bash failed")
        os.rename(partial, source)
    return full, source


def build_binaries(build, runner_repo, bash_source, log):
    env = dict(os.environ, CARGO_HOME=os.path.join(build, "cargo-home"))
    run(["cargo", "build", "--release", "--locked", "-p", "oulipoly-root-supervisor",
         "--bin", "oulipoly-root-supervisor", "--bin", "oulipoly-root-pid1"],
        log, cwd=runner_repo, env=dict(env, CARGO_TARGET_DIR=os.path.join(build, "target")))
    # Default features only: never `age319-closed-fresh` or a fixture feature.
    run(["cargo", "build", "--release", "--locked", "-p", "oulipoly-agent-runner",
         "--bin", "oulipoly-agent-runner"],
        log, cwd=runner_repo, env=dict(env, CARGO_TARGET_DIR=os.path.join(build, "target")))
    run(["cargo", "build", "--release", "--locked", "--bin", "agent-bash"],
        log, cwd=bash_source, env=dict(env, CARGO_TARGET_DIR=os.path.join(build, "agent-bash-target")))


def install_deps(build, runner_repo, log, lock_dir=LOCK_DIR):
    lock = os.path.join(runner_repo, lock_dir, "package-lock.json")
    lock_hash = sha256(lock)
    deps = os.path.join(build, "deps-" + lock_hash[:12])
    if not os.path.isdir(deps):
        partial = deps + ".partial"
        shutil.rmtree(partial, ignore_errors=True)
        os.makedirs(partial)
        for name in ("package.json", "package-lock.json"):
            shutil.copy2(os.path.join(runner_repo, lock_dir, name), partial)
        npmrc = os.path.join(build, "npmrc")
        open(npmrc, "a").close()
        run(["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund",
             "--cache", os.path.join(build, "npm-cache"), "--userconfig", npmrc],
            log, cwd=partial)
        os.rename(partial, deps)
    return lock_hash, deps


def fetch_node(build, log):
    """The pinned Node runtime: `bin/node` and its LICENSE, from the
    official release tarball checked against NODE_SHA256."""
    out = os.path.join(build, "node-" + NODE_SHA256[:12])
    if os.path.isdir(out):
        return out
    tarball = os.path.join(build, "node-dist", NODE_TARBALL)
    if not os.path.isfile(tarball) or sha256(tarball) != NODE_SHA256:
        os.makedirs(os.path.dirname(tarball), exist_ok=True)
        run(["curl", "-fsS", "--proto", "=https", "-o", tarball + ".partial", NODE_URL], log)
        os.replace(tarball + ".partial", tarball)
    if sha256(tarball) != NODE_SHA256:
        raise SystemExit(f"{NODE_TARBALL} does not match its pinned sha256")
    partial = out + ".partial"
    shutil.rmtree(partial, ignore_errors=True)
    os.makedirs(os.path.join(partial, "bin"))
    prefix = f"node-{NODE_VERSION}-linux-x64/"
    with tarfile.open(tarball) as tar:
        for name, target in (("bin/node", "bin/node"), ("LICENSE", "LICENSE")):
            member = tar.getmember(prefix + name)
            if not member.isfile():
                raise SystemExit(f"{NODE_TARBALL}: {name} is not a regular file")
            with tar.extractfile(member) as source, open(os.path.join(partial, target), "wb") as sink:
                shutil.copyfileobj(source, sink)
    os.chmod(os.path.join(partial, "bin/node"), 0o755)
    os.rename(partial, out)
    return out


def elf_needed(path):
    """Dynamic dependencies read from the ELF header (`readelf -d`): the
    file is not run, unlike `ldd`."""
    lines = output(["readelf", "-d", path]).splitlines()
    return sorted(line.split("[", 1)[1].rstrip("]") for line in lines if "(NEEDED)" in line)


def normalize(stage):
    for directory, dirs, files in os.walk(stage):
        os.chmod(directory, 0o755)
        for name in files:
            path = os.path.join(directory, name)
            if os.path.islink(path):
                continue
            mode = os.stat(path).st_mode
            os.chmod(path, 0o755 if mode & 0o111 else 0o644)


def manifest_files(stage):
    files = {}
    for directory, dirs, names in os.walk(stage):
        dirs.sort()
        for name in sorted(names + [d for d in dirs if os.path.islink(os.path.join(directory, d))]):
            path = os.path.join(directory, name)
            rel = os.path.relpath(path, stage)
            if rel == "MANIFEST.json":
                continue
            if os.path.islink(path):
                files[rel] = {"symlink": os.readlink(path)}
            else:
                st = os.stat(path)
                files[rel] = {"sha256": sha256(path), "mode": oct(st.st_mode & 0o7777), "size": st.st_size}
    return files


def dynamic_deps(path):
    try:
        lines = output(["ldd", path]).splitlines()
    except subprocess.CalledProcessError:
        return ["not a dynamic executable"]
    return sorted(line.split("(")[0].strip() for line in lines if "linux-vdso" not in line)


def claude_code_identity(stage):
    """The packaged Claude Code executable as published (never run)."""
    platform = os.path.join(stage, "claude", "deps", CLAUDE_PLATFORM)
    with open(os.path.join(platform, "package.json"), encoding="utf-8") as file:
        package = json.load(file)
    return {
        "package": package["name"],
        "version": package["version"],
        "executable": f"claude/deps/{CLAUDE_PLATFORM}/claude",
        "sha256": sha256(os.path.join(platform, "claude")),
        "modified": "no",
    }


def write_archive(stage, name, out, mtime):
    entries = []
    for directory, dirs, files in os.walk(stage):
        dirs.sort()
        for item in sorted(dirs + files):
            entries.append(os.path.join(directory, item))
    entries.sort(key=lambda path: os.path.relpath(path, stage))

    def clean(info):
        info.uid = info.gid = 0
        info.uname = info.gname = "root"
        info.mtime = mtime
        return info

    with open(out, "wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar:
            tar.add(stage, arcname=name, recursive=False, filter=clean)
            for path in entries:
                tar.add(path, arcname=os.path.join(name, os.path.relpath(path, stage)), recursive=False, filter=clean)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--build-dir", required=True)
    parser.add_argument("--runner-repo", required=True)
    parser.add_argument("--agent-bash-repo", required=True)
    parser.add_argument("--agent-bash-commit", required=True)
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args(argv)
    build = os.path.realpath(args.build_dir)
    runner_repo = os.path.realpath(args.runner_repo)
    os.makedirs(os.path.join(build, "logs"), exist_ok=True)
    log = os.path.join(build, "logs", "build-package.log")

    runner = runner_identity(runner_repo, args.allow_dirty)
    bash_commit, bash_source = agent_bash_source(build, args.agent_bash_repo, args.agent_bash_commit)
    build_binaries(build, runner_repo, bash_source, log)
    lock_hash, deps = install_deps(build, runner_repo, log)
    claude_lock_hash, claude_deps = install_deps(build, runner_repo, log, CLAUDE_LOCK_DIR)
    node = fetch_node(build, log)

    package_id = f"oulipoly-native-linux-x86_64-{runner['commit'][:12]}-{bash_commit[:12]}"
    if runner["dirty"]:
        package_id += "-dirty"
    stage = os.path.join(build, "stage", package_id)
    shutil.rmtree(stage, ignore_errors=True)
    os.makedirs(stage)
    for target, product in BINARIES:
        os.makedirs(os.path.join(stage, os.path.dirname(target)), exist_ok=True)
        shutil.copy2(os.path.join(build, product), os.path.join(stage, target))
    shutil.copy2(os.path.join(bash_source, BASH_TOOL), os.path.join(stage, "agent-bash", "bash.ts"))
    shutil.copytree(deps, os.path.join(stage, "opencode", "deps"), symlinks=True)
    unused = {os.path.join(claude_deps, path) for path in CLAUDE_UNUSED}
    shutil.copytree(claude_deps, os.path.join(stage, "claude", "deps"), symlinks=True,
                    ignore=lambda directory, names: [n for n in names if os.path.join(directory, n) in unused])
    shutil.copytree(node, os.path.join(stage, "claude", "node"), symlinks=True)
    for target, source, mode in ASSETS:
        os.makedirs(os.path.join(stage, os.path.dirname(target)), exist_ok=True)
        shutil.copyfile(os.path.join(HERE, source), os.path.join(stage, target))
        os.chmod(os.path.join(stage, target), mode)
    normalize(stage)

    toolchain = {
        name: output(argv_) for name, argv_ in (
            ("cargo", ["cargo", "--version"]), ("rustc", ["rustc", "--version"]),
            ("node", ["node", "--version"]), ("npm", ["npm", "--version"]),
            ("python_at_build", [sys.executable, "--version"]),
        )
    }
    manifest = {
        "format": 1,
        "id": package_id,
        "platform": "linux-x86_64",
        "runner": {**runner, "repo": "nestharus/agent-runner", "profile": "release", "features": "default"},
        "agent_bash": {"commit": bash_commit, "repo": "nestharus/agent-bash-tool"},
        "opencode_lock_sha256": lock_hash,
        "claude_lock_sha256": claude_lock_hash,
        "claude_code": claude_code_identity(stage),
        "claude_deps_not_staged": list(CLAUDE_UNUSED),
        "node": {"version": NODE_VERSION, "url": NODE_URL, "tarball_sha256": NODE_SHA256,
                 "files": ["bin/node", "LICENSE"]},
        "toolchain": toolchain,
        "runtime_requirements": {
            "python": "/usr/bin/python3 (3.12 or newer: os.unshare) for the front door and caller",
            "sudo": "a sudoers rule from share/sudoers.template",
            "kernel": "PID and mount namespaces",
            "claude": "the packaged Node runtime runs the receiver; Claude Code runs as the requester with its own login in the route's store",
        },
        "dynamic_libraries": {
            **{target: dynamic_deps(os.path.join(stage, target)) for target, _ in BINARIES},
            **{target: elf_needed(os.path.join(stage, target))
               for target in ("claude/node/bin/node", f"claude/deps/{CLAUDE_PLATFORM}/claude")},
        },
        "files": manifest_files(stage),
    }
    with open(os.path.join(stage, "MANIFEST.json"), "w", encoding="utf-8") as file:
        json.dump(manifest, file, indent=1, sort_keys=True)
        file.write("\n")
    os.chmod(os.path.join(stage, "MANIFEST.json"), 0o644)

    dist = os.path.join(build, "dist")
    os.makedirs(dist, exist_ok=True)
    archive = os.path.join(dist, package_id + ".tar.gz")
    write_archive(stage, package_id, archive, runner["commit_time"])
    digest = sha256(archive)
    with open(archive + ".sha256", "w", encoding="utf-8") as file:
        file.write(f"{digest}  {os.path.basename(archive)}\n")
    print(json.dumps({"id": package_id, "archive": archive, "sha256": digest, "stage": stage}))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
