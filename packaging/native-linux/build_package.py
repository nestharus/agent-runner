#!/usr/bin/env python3
"""Build the inert Linux x86_64 native-root package from exact sources.

    build_package.py --build-dir B --runner-repo W
        --agent-bash-repo T --agent-bash-commit C [--allow-dirty]

Everything it writes is under B (cargo home and targets, the npm cache,
the locked dependencies, the stage and `dist/`); W and T are only read
(T through `git archive C`). Network is used only by cargo and `npm ci`,
for locked public dependencies. Nothing is installed, run as root or
started beyond the builds.

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
        subprocess.run(argv, check=True, stdout=out, stderr=subprocess.STDOUT, **kwargs)


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


def install_deps(build, runner_repo, log):
    lock = os.path.join(runner_repo, LOCK_DIR, "package-lock.json")
    lock_hash = sha256(lock)
    deps = os.path.join(build, "deps-" + lock_hash[:12])
    if not os.path.isdir(deps):
        partial = deps + ".partial"
        shutil.rmtree(partial, ignore_errors=True)
        os.makedirs(partial)
        for name in ("package.json", "package-lock.json"):
            shutil.copy2(os.path.join(runner_repo, LOCK_DIR, name), partial)
        npmrc = os.path.join(build, "npmrc")
        open(npmrc, "a").close()
        run(["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund",
             "--cache", os.path.join(build, "npm-cache"), "--userconfig", npmrc],
            log, cwd=partial)
        os.rename(partial, deps)
    return lock_hash, deps


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
        "toolchain": toolchain,
        "runtime_requirements": {
            "python": "/usr/bin/python3 (3.12 or newer: os.unshare) for the front door and caller",
            "sudo": "a sudoers rule from share/sudoers.template",
            "kernel": "PID and mount namespaces",
        },
        "dynamic_libraries": {target: dynamic_deps(os.path.join(stage, target)) for target, _ in BINARIES},
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
