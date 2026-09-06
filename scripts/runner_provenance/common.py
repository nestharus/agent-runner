"""File identity and fail-closed primitives for the bounded Linux installer."""

import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess


class Rejected(ValueError):
    """An input or precondition does not support the requested operation."""


def require(condition, message):
    if not condition:
        raise Rejected(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def fingerprint(path):
    path = Path(path)
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode), f"not a regular file: {path}")
    with path.open("rb") as stream:
        sha = hashlib.file_digest(stream, "sha256").hexdigest()
    return {"sha256": sha, "size": info.st_size}


def canonical_path(path):
    path = Path(os.path.abspath(path))
    require(path == path.resolve(), f"symlink path not admitted: {path}")
    return path


def write_new(path, data, mode=0o600):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    with os.fdopen(fd, "wb") as stream:
        os.fchmod(stream.fileno(), mode)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def write_json(path, value):
    write_new(path, json_bytes(value))


def pinned_json(path, expected):
    data = Path(path).read_bytes()
    require(digest(data) == expected, "independent digest mismatch")
    return json.loads(data)


def command(argv, cwd, log, env=None):
    """Retain complete subprocess output and status, including failures."""
    with Path(log).open("xb") as output:
        output.write(json_bytes({"command": [str(a) for a in argv], "cwd": str(cwd)}))
        output.flush()
        result = subprocess.run(
            argv, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT, check=False
        )
        output.write(f"\nEXIT={result.returncode}\n".encode())
    require(result.returncode == 0, f"command failed; see {log}")


def git(repo, *args):
    result = subprocess.run(
        [
            "/usr/bin/git",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-C",
            str(repo),
            *args,
        ],
        env=git_environment(),
        capture_output=True,
        check=True,
    )
    return result.stdout.decode().strip()


def git_environment():
    return {
        "PATH": "/usr/bin:/bin",
        "HOME": "/nonexistent",
        "LC_ALL": "C",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_TERMINAL_PROMPT": "0",
        "GIT_NO_REPLACE_OBJECTS": "1",
        # Source inspection must not refresh/write the index, even on rejection.
        "GIT_OPTIONAL_LOCKS": "0",
    }


def source_identity(repo, expected_commit):
    require(
        git(repo, "rev-parse", "--show-toplevel") == str(Path(repo).resolve()),
        "source must be repository root",
    )
    head = git(repo, "rev-parse", "HEAD")
    require(head == expected_commit, "source commit mismatch (use exact full HEAD)")
    require(
        not git(repo, "status", "--porcelain=v1", "--untracked-files=all"),
        "dirty source",
    )
    require(not git(repo, "ls-files", "-v").startswith("S "), "sparse source")
    entries = git(repo, "ls-files", "--stage", "-z").split("\0")
    entries = [row for row in entries if row]
    require(
        all(row.startswith(("100644 ", "100755 ", "120000 ")) for row in entries),
        "submodules are not supported",
    )
    for row in entries:
        if row.startswith("120000 "):
            link = Path(repo) / row.split("\t", 1)[1]
            require(
                link.resolve().is_relative_to(Path(repo).resolve()),
                "symlink escapes source",
            )
    # Index flags can hide modified tracked bytes from status; reject all such flags.
    flags = git(repo, "ls-files", "-v").splitlines()
    require(all(row.startswith("H ") for row in flags), "hidden index flags")
    return {
        "commit": head,
        "tree": git(repo, "rev-parse", "HEAD^{tree}"),
        "clean": True,
    }


def tree_digest(root):
    """Content identity of a public, trusted input tree (no secret directories)."""
    root = Path(root)
    sha = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        info = path.lstat()
        relative = str(path.relative_to(root))
        if path.is_symlink():
            value = {"path": relative, "link": os.readlink(path)}
        elif path.is_file():
            value = {
                "path": relative,
                "mode": stat.S_IMODE(info.st_mode),
                **fingerprint(path),
            }
        else:
            require(path.is_dir(), f"special file in input tree: {path}")
            value = {"path": relative, "directory": True}
        sha.update(json_bytes(value))
    return sha.hexdigest()
