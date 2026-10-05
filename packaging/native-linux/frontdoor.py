#!/usr/bin/python3 -I
"""Linux native ACP v2 bounded privileged front door. Root-owned assets,
site routes and requester identity determine launch; stdin v1 JSON supplies
one task, cwd/policy/env/deadline, optional inline access-only credential.
No requester credential path or raw native-root fields are read. A Claude
route (`harness: claude`) takes no credential: Claude Code runs as the
requester against the route's store below the requester's home, which this
process names but never reads.

Nonblocking control/output queues keep timers independent of consumers.
Early close/cancel arms a grace-relative kill; absent controls the backstop
is N+2G. Admission and collection are bounded; kernel/FS stalls remain
outside a finite guarantee. Descriptor-relative no-follow credential
cleanup and fd-safe discard never traverse a work-replaced symlink.

Native exit status or 90 refusal, 91 setup failure, 92 kill requested,
93 stop/collection unknown, 94 cleanup/residue failed. No replay.
Registered children (opt-in): the site names child routes, which parent
routes may offer them and the ceilings; the requester picks among them.
An OpenCode parent's children reuse its admitted grant; a Claude parent's
take a separate access-only child-provider grant (never Claude's login).
That grant is staged once per root (private/child-auth.json, each child's
launch copies it) and normal-path retirement removes staged and derived copies, including
children's, under keep or discard when cleanup succeeds. Unknown entry
stop skips retirement; abrupt loss can leave copies until the stale-run
sweep. Access-only expiry bounds their intended use window, not a
universal deletion or provider-validity guarantee.

Access-only staging avoids intentional credential echo, but arbitrary
native/task output is unredacted and can contain secrets. Requester env
uses the accepted denylist and reaches root Rust processes too. Full
contract, limits and prepared ROOT operations: share/README.md.
"""

import ctypes
import fcntl
import json
import os
import pwd
import re
import secrets
import select
import shutil
import signal
import stat
import subprocess
import sys
import time

SITE_CONFIG = "/etc/oulipoly-native/frontdoor.json"
REQUEST_LIMIT = 4 * 1024 * 1024
CONTROL_LIMIT = 4 * 1024 * 1024
MESSAGE_LIMIT = 1024 * 1024
ADMISSION_S = 30
COLLECTION_S = 2
OUTPUT_LIMIT = 8 * 1024 * 1024
MAX_EXPIRY_MS = 253402300799000

EXIT_REFUSED = 90
EXIT_RUN_FAILED = 91
EXIT_KILLED = 92
EXIT_UNKNOWN = 93
EXIT_CLEANUP_FAILED = 94

RUNNER = "bin/oulipoly-agent-runner"
SUPERVISOR = "bin/oulipoly-root-supervisor"
PID1 = "bin/oulipoly-root-pid1"
DEPS = "opencode/deps"
CLAUDE_DEPS = "claude/deps"
NODE = "claude/node/bin/node"
CLAUDE_EXECUTABLE = CLAUDE_DEPS + "/node_modules/@anthropic-ai/claude-agent-sdk-linux-x64/claude"
CLAUDE_EFFORTS = ("low", "medium", "high", "xhigh", "max")
CLAUDE_MODEL = re.compile(r"[A-Za-z0-9._\[\]-]{1,128}")
BASH_TOOL = "agent-bash/bash.ts"
BASH_BIN = "agent-bash/agent-bash"

# Names a requester may not put in the root environment: it reaches the
# owner and root PID 1 (root) as well as the work. Loader and libc names
# glibc itself drops for privileged programs, and names the owner, native
# host or sudo own.
UNSAFE_ENV = frozenset(
    """GCONV_PATH GETCONF_DIR GLIBC_TUNABLES HOSTALIASES LOCALDOMAIN LOCPATH
    MALLOC_TRACE NIS_PATH NLSPATH RESOLV_HOST_CONF RES_OPTIONS TMPDIR TZDIR
    HOME USER LOGNAME SHELL""".split()
)
UNSAFE_PREFIXES = ("LD_", "MALLOC_", "GLIBC_", "OULIPOLY_", "OPENCODE_", "AGENT_BASH_", "SUDO_")
ENV_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]{0,127}")
ENV_LIMIT = 64
ENV_VALUE_LIMIT = 32 * 1024

CREDENTIAL_FIELDS = frozenset({"type", "refresh", "access", "expires", "accountId"})
CREDENTIAL_FILES = (
    "private/auth.json",
    "private/child-auth.json",
    "launch/xdg/data/opencode/auth.json",
    "launch/secret/server-password",
)
# Below each child's launch directory (`launch/children/c<N>`).
CHILD_CREDENTIAL_FILES = ("xdg/data/opencode/auth.json", "secret/server-password")
CHILD_LAUNCH = re.compile(r"c[0-9]{1,9}")
# The owner's ceilings for this first child capability.
CHILD_MAX_STARTS = 4
CHILD_MAX_CONCURRENT = 2

ENTRY_ENV = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"}

# Owners trusted for what root runs or reads. Only a root test harness
# that imports this module (never the sudoers path) may widen it, for an
# unprivileged user namespace where host root shows as the overflow uid.
TRUSTED_OWNERS = frozenset({0})


class Refused(Exception):
    """Refused before any effect."""


class RunFailed(Exception):
    """Refused or failed after the run directory was made."""


# Where this process's lines go (stdout; a test may name another fd).
OUT_FD = 1


def encoded(line):
    return (line if isinstance(line, bytes) else json.dumps(line, sort_keys=True).encode()) + b"\n"


def emit(line):
    """Best effort bounded write, including admission/failure terminals."""
    old = os.get_blocking(OUT_FD)
    end = time.monotonic() + 0.25
    data = encoded(line)
    try:
        os.set_blocking(OUT_FD, False)
        while data:
            if time.monotonic() >= end:
                return False
            if not select.select([], [OUT_FD], [], max(0, end - time.monotonic()))[1]:
                return False
            try:
                written = os.write(OUT_FD, data)
                data = data[written:]
            except BlockingIOError:
                pass
        return True
    except OSError:
        return False
    finally:
        try:
            os.set_blocking(OUT_FD, old)
        except OSError:
            pass


# Custody: what root runs or reads must be root's alone.


def check_owned(path):
    """`path` and every directory above it: owned by a trusted owner (root), not writable
    by group or others, no symlink at `path` itself."""
    path = os.path.abspath(path)
    st = os.lstat(path)
    if stat.S_ISLNK(st.st_mode):
        raise Refused(f"custody: {path} is a symlink")
    for start in (path, os.path.realpath(path)):
        current = start
        while True:
            st = os.lstat(current)
            if st.st_uid not in TRUSTED_OWNERS:
                raise Refused(f"custody: {current} is not owned by root")
            if st.st_mode & 0o022 and not stat.S_ISLNK(st.st_mode):
                raise Refused(f"custody: {current} is writable by group or others")
            if current == "/":
                break
            current = os.path.dirname(current)


def check_tree(root):
    """Every entry below `root`: owned by a trusted owner (root), not writable by group or
    others; a symlink must resolve inside `root`."""
    real_root = os.path.realpath(root)
    for directory, dirs, files in os.walk(root):
        for name in dirs + files:
            path = os.path.join(directory, name)
            st = os.lstat(path)
            if st.st_uid not in TRUSTED_OWNERS:
                raise Refused(f"custody: {path} is not owned by root")
            if stat.S_ISLNK(st.st_mode):
                target = os.path.realpath(path)
                if target != real_root and not target.startswith(real_root + "/"):
                    raise Refused(f"custody: {path} leaves the package")
            elif st.st_mode & 0o022:
                raise Refused(f"custody: {path} is writable by group or others")


def package_root():
    return os.path.dirname(os.path.dirname(os.path.realpath(__file__)))


def check_package(root):
    check_owned(root)
    check_tree(root)
    for name in (RUNNER, SUPERVISOR, PID1, BASH_BIN, NODE, CLAUDE_EXECUTABLE):
        path = os.path.join(root, name)
        if not os.path.isfile(path) or not os.access(path, os.X_OK):
            raise Refused(f"package: no executable {name}")
    for name in (BASH_TOOL, DEPS + "/package-lock.json", CLAUDE_DEPS + "/package-lock.json"):
        if not os.path.isfile(os.path.join(root, name)):
            raise Refused(f"package: no {name}")


# Site configuration: the administrator's, root-owned.


def load_site(path):
    try:
        check_owned(path)
        with open(path, "rb") as file:
            site = json.loads(file.read(REQUEST_LIMIT))
    except FileNotFoundError:
        raise Refused(f"site config {path} is absent") from None
    except (OSError, ValueError) as error:
        raise Refused(f"site config {path}: {type(error).__name__}") from None
    known = {
        "v", "allowed_users", "run_base", "max_deadline_s", "cancel_grace_s",
        "credential_margin_s", "allow_keep", "default_path", "routes",
        "child_routes", "child_limits",
    }
    if not isinstance(site, dict) or site.get("v") != 1 or set(site) - known:
        raise Refused("site config: not a v1 config")
    users = site.get("allowed_users")
    if not isinstance(users, list) or not all(isinstance(user, str) and user for user in users):
        raise Refused("site config: allowed_users")
    if not isinstance(site.get("run_base"), str) or not site["run_base"].startswith("/"):
        raise Refused("site config: run_base")
    for name, default in (("max_deadline_s", 7200), ("cancel_grace_s", 30), ("credential_margin_s", 300)):
        value = site.setdefault(name, default)
        if type(value) is not int or value < 1:
            raise Refused(f"site config: {name}")
    if type(site.setdefault("allow_keep", False)) is not bool:
        raise Refused("site config: allow_keep")
    if not isinstance(site.setdefault("default_path", "/usr/local/bin:/usr/bin:/bin"), str):
        raise Refused("site config: default_path")
    routes = site.get("routes")
    if not isinstance(routes, dict) or not routes:
        raise Refused("site config: routes")
    for name, route in routes.items():
        if not (opencode_route(route) or claude_route(route)):
            raise Refused(f"site config: route {name}")
    check_site_children(site)
    return site


def check_site_children(site):
    """`child_routes` (OpenCode routes a child may run on), each parent
    route's `children` (the child routes it may offer) and `child_limits`
    (at most the owner's ceilings)."""
    child_routes = site.setdefault("child_routes", {})
    if not isinstance(child_routes, dict):
        raise Refused("site config: child_routes")
    for name, route in child_routes.items():
        if (
            not isinstance(name, str)
            or not re.fullmatch(r"[A-Za-z0-9._-]{1,64}", name)
            or not opencode_route(route)
            or "children" in route
        ):
            raise Refused(f"site config: child route {name}")
    limits = site.setdefault("child_limits", {})
    if not isinstance(limits, dict) or set(limits) - {"max_starts", "max_concurrent"}:
        raise Refused("site config: child_limits")
    for key, ceiling in (("max_starts", CHILD_MAX_STARTS), ("max_concurrent", CHILD_MAX_CONCURRENT)):
        value = limits.setdefault(key, ceiling)
        if type(value) is not int or not 1 <= value <= ceiling:
            raise Refused(f"site config: child_limits.{key} must be 1..{ceiling}")
    for name, route in site["routes"].items():
        offered = route.get("children", [])
        if not isinstance(offered, list) or not all(isinstance(child, str) and child in child_routes for child in offered):
            raise Refused(f"site config: route {name} children")


def opencode_route(route):
    return (
        isinstance(route, dict)
        and not set(route) - {"model", "provider", "credential", "children"}
        and isinstance(route.get("model"), str)
        and "/" in route["model"]
        and isinstance(route.get("provider"), dict)
        and route["model"].split("/", 1)[0] in route["provider"]
        and route.get("credential") in ("required", "none")
    )


def claude_route(route):
    """A native Claude Code route: model, effort, and the requester's own
    Claude configuration directory relative to its home. No credential."""
    store = route.get("config_dir") if isinstance(route, dict) else None
    return (
        isinstance(route, dict)
        and set(route) - {"children"} == {"harness", "model", "effort", "config_dir", "credential"}
        and route["harness"] == "claude"
        and isinstance(route["model"], str)
        and CLAUDE_MODEL.fullmatch(route["model"]) is not None
        and route["effort"] in CLAUDE_EFFORTS
        and route["credential"] == "none"
        and isinstance(store, str)
        and store
        and "\0" not in store
        and not store.startswith("/")
        and not os.path.normpath(store).startswith("..")
        and os.path.normpath(store) != "."
    )


# The requester and its request.


def requester(site, environ):
    if os.geteuid() != 0 or os.getuid() != 0:
        raise Refused("not running as root")
    uid = environ.get("SUDO_UID", "")
    if not uid.isdigit():
        raise Refused("no requester (SUDO_UID)")
    try:
        user = pwd.getpwuid(int(uid))
    except KeyError:
        raise Refused("requester uid has no passwd entry") from None
    if user.pw_uid == 0:
        raise Refused("requester is root")
    if user.pw_name not in site["allowed_users"]:
        raise Refused(f"requester {user.pw_name} is not allowed")
    return user


def check_request(request, site, now):
    """Returns the checked request; refuses anything outside its shape."""
    if not isinstance(request, dict):
        raise Refused("request is not an object")
    known = {"v", "route", "message", "cwd", "bash", "env", "deadline_s", "credential", "retention",
             "children", "child_credential"}
    if request.get("v") != 1:
        raise Refused("request: v must be 1")
    unknown = set(request) - known
    if unknown:
        raise Refused("request: unknown fields")
    try:
        json.dumps(request, ensure_ascii=False).encode("utf-8")
    except UnicodeError:
        raise Refused("request: invalid Unicode") from None
    route_name = request.get("route")
    route = site["routes"].get(route_name) if isinstance(route_name, str) else None
    if route is None:
        raise Refused("request: route is not a site route")
    message = request.get("message")
    if not isinstance(message, str) or not message.strip() or len(message.encode()) > MESSAGE_LIMIT:
        raise Refused("request: message")
    cwd = request.get("cwd")
    if not isinstance(cwd, str) or not cwd.startswith("/") or "\0" in cwd or len(cwd) > 4096:
        raise Refused("request: cwd must be absolute")
    bash = request.get("bash")
    if bash == {"authority": "trusted-task"}:
        policy = {"bash_authority": "trusted-task"}
    elif (
        isinstance(bash, dict)
        and set(bash) == {"allow"}
        and isinstance(bash["allow"], list)
        and 0 < len(bash["allow"]) <= 256
        and all(isinstance(command, str) and command.strip() for command in bash["allow"])
    ):
        policy = {"bash_allow": bash["allow"]}
    else:
        raise Refused("request: bash must be {authority: trusted-task} or {allow: [commands]}")
    deadline = request.get("deadline_s")
    if type(deadline) is not int or not 0 < deadline <= site["max_deadline_s"]:
        raise Refused(f"request: deadline_s must be 1..{site['max_deadline_s']}")
    retention = request.get("retention", "discard")
    if retention not in ("discard", "keep") or (retention == "keep" and not site["allow_keep"]):
        raise Refused("request: retention")
    credential = check_credential(route, request.get("credential"), deadline, site, now)
    children = check_children(route, request, credential, deadline, site, now)
    return {
        "route_name": route_name,
        "route": route,
        "message": message,
        "cwd": os.path.normpath(cwd),
        "policy": policy,
        "extra_env": request.get("env", {}),
        "deadline": deadline,
        "retention": retention,
        "credential": credential,
        "children": children,
    }


def check_children(route, request, credential, deadline, site, now):
    """The requester's opt-in to registered children, within what the site
    lets this parent route offer, and the children's one grant; or None.
    `credential` is the parent's checked one (an OpenCode parent's
    children reuse it). Never Claude's login."""
    asked = request.get("children")
    if asked is None:
        if "child_credential" in request:
            raise Refused("request: child_credential without children")
        return None
    if not isinstance(asked, dict) or set(asked) - {"routes", "max_starts", "max_concurrent"}:
        raise Refused("request: children must be {routes, max_starts?, max_concurrent?}")
    names = asked.get("routes")
    offered = route.get("children", [])
    if (
        not isinstance(names, list)
        or not names
        or len(set(names)) != len(names)
        or not all(isinstance(name, str) and name in offered for name in names)
    ):
        raise Refused("request: children.routes must name child routes this route offers")
    limits = {}
    for key in ("max_starts", "max_concurrent"):
        ceiling = site["child_limits"][key]
        value = asked.get(key, ceiling)
        if type(value) is not int or not 1 <= value <= ceiling:
            raise Refused(f"request: children.{key} must be 1..{ceiling}")
        limits[key] = value
    child_routes = {name: site["child_routes"][name] for name in names}
    needs = {name: child["credential"] == "required" for name, child in child_routes.items()}
    providers = {child["model"].split("/", 1)[0] for name, child in child_routes.items() if needs[name]}
    if len(providers) > 1:
        raise Refused("request: children's credential-requiring routes must share one provider")
    grant = None
    if providers:
        (provider,) = providers
        if route.get("harness") == "claude":
            if "child_credential" not in request:
                raise Refused("request: a Claude parent's children need child_credential (access-only, the child provider's)")
            pseudo = {"model": provider + "/child", "credential": "required"}
            grant = check_credential(pseudo, request["child_credential"], deadline, site, now)
            grant = (grant[0], dict(grant[1], source="separate-child-grant"))
        else:
            if "child_credential" in request:
                raise Refused("request: an OpenCode parent's children reuse its own grant; no child_credential")
            if credential is None or set(credential[0]) != {provider}:
                raise Refused(f"request: children need this route's own {provider} grant to reuse")
            grant = (credential[0], dict(credential[1], source="reused-parent-grant"))
    elif "child_credential" in request:
        raise Refused("request: these child routes take no credential")
    return {"routes": child_routes, **limits, "grant": grant}


def check_credential(route, credential, deadline, site, now):
    """An access-only OAuth entry for the route's provider, fresh enough
    for the deadline and the site margin, or None. Reasons never carry any
    part of it."""
    provider = route["model"].split("/", 1)[0]
    if route["credential"] == "none":
        if credential is not None:
            raise Refused("request: this route takes no credential")
        return None
    if not isinstance(credential, dict) or set(credential) != {provider}:
        raise Refused(f"request: credential must be one {provider} entry")
    entry = credential[provider]
    if not isinstance(entry, dict) or set(entry) - CREDENTIAL_FIELDS:
        raise Refused("request: credential has unknown fields")
    if entry.get("type") != "oauth":
        raise Refused("request: credential must be oauth")
    if entry.get("refresh") != "":
        raise Refused("request: credential carries a refresh grant; only access-only is accepted")
    access = entry.get("access")
    if not isinstance(access, str) or not access or len(access) > 16384 or any(c.isspace() for c in access):
        raise Refused("request: credential access")
    expires = entry.get("expires")
    if type(expires) is not int or not 0 < expires <= MAX_EXPIRY_MS:
        raise Refused("request: credential expires")
    if "accountId" in entry and (not isinstance(entry["accountId"], str) or not entry["accountId"]):
        raise Refused("request: credential accountId")
    remaining = expires // 1000 - int(now)
    needed = deadline + 2 * site["cancel_grace_s"] + COLLECTION_S + site["credential_margin_s"]
    if remaining < needed:
        raise Refused(f"request: credential expires in {remaining}s, needs {needed}s")
    return {provider: dict(entry)}, {
        "provider": provider,
        "expires_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(expires // 1000)),
        "remaining_s": remaining,
        "refresh": "absent",
    }


def root_env(user, site, extra):
    if not isinstance(extra, dict) or len(extra) > ENV_LIMIT:
        raise Refused(f"request: env must be an object of at most {ENV_LIMIT} names")
    env = {
        "HOME": user.pw_dir,
        "USER": user.pw_name,
        "LOGNAME": user.pw_name,
        "SHELL": user.pw_shell or "/bin/sh",
        "PATH": site["default_path"],
        "LANG": "C.UTF-8",
    }
    for name, value in extra.items():
        if not ENV_NAME.fullmatch(name) or name in UNSAFE_ENV or name.startswith(UNSAFE_PREFIXES):
            raise Refused(f"request: env name {name!r} is not passable")
        if not isinstance(value, str) or "\0" in value or len(value) > ENV_VALUE_LIMIT:
            raise Refused(f"request: env {name} value")
        env[name] = value
    return env


def check_cwd_as(user, cwd, timeout=ADMISSION_S):
    """The requester itself can enter and read `cwd` (checked in a child
    with the requester's identity, never with root's)."""
    pid = os.fork()
    if pid == 0:
        try:
            os.setgroups(os.getgrouplist(user.pw_name, user.pw_gid))
            os.setgid(user.pw_gid)
            os.setuid(user.pw_uid)
            ok = stat.S_ISDIR(os.stat(cwd).st_mode) and os.access(cwd, os.R_OK | os.X_OK)
            os._exit(0 if ok else 1)
        except BaseException:
            os._exit(2)
    end = time.monotonic() + timeout
    while True:
        collected, status = os.waitpid(pid, os.WNOHANG)
        if collected:
            break
        if time.monotonic() >= end:
            os.kill(pid, signal.SIGKILL)
            # No blocking wait on a possible kernel stall.
            os.waitpid(pid, os.WNOHANG)
            raise Refused("request: cwd check timed out; stop unknown")
        time.sleep(0.01)
    if os.waitstatus_to_exitcode(status) != 0:
        raise Refused("request: cwd is not a directory the requester can enter and read")


# The run directory.


def stale_runs(user_dir):
    """Runs below `user_dir` whose front door is gone (lock free)."""
    found = []
    try:
        names = sorted(os.listdir(user_dir))
    except FileNotFoundError:
        return found
    for name in names:
        run = os.path.join(user_dir, name)
        lock = os.path.join(run, "private", "lock")
        try:
            check_owned(run)
            check_owned(os.path.join(run, "private"))
            fd = os.open(lock, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
        except (Refused, OSError):
            continue
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            os.close(fd)
            continue
        found.append((run, fd))
    return found


def unlink_beneath(run, relative):
    """No privileged traversal through work-replaceable symlinks. Each
    directory is pinned by an fd; unlink affects only its own directory."""
    fd = os.open(run, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        parts = relative.split("/")
        for part in parts[:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
            os.close(fd)
            fd = child
        os.unlink(parts[-1], dir_fd=fd)
    finally:
        os.close(fd)


def child_credential_files(run):
    """Each child launch's credential files (relative), found without
    following a symlink; an unlistable children directory is a failure."""
    names, failed = [], []
    try:
        fd = os.open(run, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            for part in ("launch", "children"):
                child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
                os.close(fd)
                fd = child
            entries = os.listdir(fd)
        finally:
            os.close(fd)
    except FileNotFoundError:
        return names, failed
    except OSError as error:
        return names, [{"path": os.path.join(run, "launch/children"), "result": "FAILED", "error": type(error).__name__}]
    for entry in sorted(entries):
        if CHILD_LAUNCH.fullmatch(entry):
            names += [f"launch/children/{entry}/{name}" for name in CHILD_CREDENTIAL_FILES]
    return names, failed


def remove_credentials(run):
    child_files, records = child_credential_files(run)
    for name in CREDENTIAL_FILES + tuple(child_files):
        path = os.path.join(run, name)
        try:
            unlink_beneath(run, name)
            records.append({"path": path, "result": "removed"})
        except FileNotFoundError:
            records.append({"path": path, "result": "absent"})
        except OSError as error:
            records.append({"path": path, "result": "FAILED", "error": type(error).__name__})
    return {"ok": all(record["result"] != "FAILED" for record in records), "files": records}


def retire(run, retention):
    """Removes credential files, then the run if it is discarded."""
    credentials = remove_credentials(run)
    removed = None
    if retention == "discard" and credentials["ok"]:
        try:
            if not shutil.rmtree.avoids_symlink_attacks:
                raise OSError("descriptor-safe tree removal unavailable")
            shutil.rmtree(run)
            removed = True
        except OSError as error:
            removed = type(error).__name__
    return {"ok": credentials["ok"] and (retention != "discard" or removed is True),
            "credentials": credentials, "run_removed": removed}


def sweep(user_dir):
    results = []
    for run, fd in stale_runs(user_dir):
        try:
            with open(os.path.join(run, "private", "retention"), encoding="utf-8") as file:
                retention = file.read().strip()
        except OSError:
            retention = "keep"
        result = retire(run, "discard" if retention == "discard" else "keep")
        os.close(fd)
        results.append({"run": run, **result})
    return results


def make_run(site, user):
    base = site["run_base"]
    check_owned(base)
    user_dir = os.path.join(base, str(user.pw_uid))
    try:
        os.mkdir(user_dir, 0o711)
    except FileExistsError:
        pass
    check_owned(user_dir)
    swept = sweep(user_dir)
    run_id = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()) + "-" + secrets.token_hex(6)
    run = os.path.join(user_dir, run_id)
    os.mkdir(run, 0o711)
    os.chmod(run, 0o711)
    os.mkdir(os.path.join(run, "private"), 0o700)
    lock = os.open(os.path.join(run, "private", "lock"), os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    return run_id, run, lock, swept


def write_private(path, value):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as file:
        file.write(value if isinstance(value, str) else json.dumps(value))


def children_request(package, run, checked):
    children = checked.get("children")
    if children is None:
        return {}
    value = {
        "routes": {name: {"model": child["model"], "provider": child["provider"]} for name, child in children["routes"].items()},
        "opencode": {
            "deps": os.path.join(package, DEPS),
            "agent_bash_tool": os.path.join(package, BASH_TOOL),
            "agent_bash_bin": os.path.join(package, BASH_BIN),
        },
        "max_starts": children["max_starts"],
        "max_concurrent": children["max_concurrent"],
    }
    if children["grant"] is not None:
        value["auth"] = os.path.join(run, "private", "child-auth.json")
    return {"children": value}


def entry_request(package, run, user, checked, env, authenticated):
    route = checked["route"]
    common = {
        "store": os.path.join(run, "store"),
        "launch_dir": os.path.join(run, "launch"),
        "cwd": checked["cwd"],
        "env": env,
        "messages": [checked["message"]],
        "outage_closure_cap": 1,
        "delivery_attempt_cap": 1,
        "workload": {"isolation": "host-root", "user": user.pw_name},
        **children_request(package, run, checked),
    }
    if route.get("harness") == "claude":
        # Named only: the requester's own store, never read here.
        return dict(common, claude={
            "deps": os.path.join(package, CLAUDE_DEPS),
            "node": os.path.join(package, NODE),
            "agent_bash_bin": os.path.join(package, BASH_BIN),
            "model": route["model"],
            "effort": route["effort"],
            "config_dir": os.path.normpath(os.path.join(user.pw_dir, route["config_dir"])),
            **checked["policy"],
        })
    opencode = {
        "deps": os.path.join(package, DEPS),
        "agent_bash_tool": os.path.join(package, BASH_TOOL),
        "agent_bash_bin": os.path.join(package, BASH_BIN),
        "model": route["model"],
        "provider": route["provider"],
        **checked["policy"],
    }
    if authenticated:
        opencode["auth"] = os.path.join(run, "private", "auth.json")
    return dict(common, opencode=opencode)


# Containment: the entry is PID 1 of a fresh PID and mount namespace.

_libc = ctypes.CDLL(None, use_errno=True)
_PR_SET_PDEATHSIG = 1
_MS_NOSUID, _MS_NODEV, _MS_NOEXEC, _MS_REC, _MS_PRIVATE = 2, 4, 8, 16384, 1 << 18


def _check(result, what):
    if result != 0:
        error = ctypes.get_errno()
        raise OSError(error, f"{what}: {os.strerror(error)}")


def contained(alive_r, alive_w):
    """The entry child's pre-exec: die with this process, private mounts,
    its own /proc."""

    def pre_exec():
        try:
            os.close(alive_w)
        except OSError:
            pass
        _check(_libc.prctl(_PR_SET_PDEATHSIG, int(signal.SIGKILL), 0, 0, 0), "prctl")
        # This process may have died before the setting took effect: the
        # pipe's only remaining writer is its.
        if select.select([alive_r], [], [], 0)[0]:
            os._exit(1)
        os.unshare(os.CLONE_NEWNS)
        _check(_libc.mount(None, b"/", None, _MS_REC | _MS_PRIVATE, None), "mount private")
        _check(_libc.mount(b"proc", b"/proc", b"proc", _MS_NOSUID | _MS_NODEV | _MS_NOEXEC, None), "mount proc")

    return pre_exec


def start_entry(package, request_path):
    """Starts the entry as PID 1 of a new PID namespace. Only one process
    can be started after this (the namespace's init)."""
    os.unshare(os.CLONE_NEWPID)
    alive_r, alive_w = os.pipe()
    try:
        entry = subprocess.Popen(
            [os.path.join(package, RUNNER), "native-root", "--request", request_path],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            env=ENTRY_ENV,
            cwd="/",
            preexec_fn=contained(alive_r, alive_w),
            close_fds=True,
        )
    finally:
        os.close(alive_r)
    return entry, alive_w


# Control relay.


def control(line):
    """The control line if it is one the entry accepts, else None."""
    try:
        value = json.loads(line)
    except ValueError:
        return None
    if value in ({"cmd": "cancel"}, {"cmd": "close"}):
        return value
    if (
        isinstance(value, dict)
        and value.get("cmd") == "send"
        and set(value) <= {"cmd", "text", "ref"}
        and isinstance(value.get("text"), str)
        and value["text"].strip()
        and isinstance(value.get("ref", ""), str)
    ):
        return value
    return None


class Relay:
    """Relays entry stdout to stdout and requester controls to the entry,
    and owns the deadline and abandonment."""

    def __init__(self, entry, run, deadline_s, grace_s, clock=time.monotonic):
        self.entry = entry
        self.run = run
        self.clock = clock
        self.started = clock()
        self.deadline = self.started + deadline_s
        self.grace = grace_s
        self.cancel_at = None
        self.kill_at = None
        self.killed = False
        self.why = None
        self.stdout_gone = False
        self.stdin_open = True
        self.stdin_buffer = b""
        self.out_buffer = b""
        self.out_eof = False
        self.staged_removed = None
        self.signals = []
        self.output = b""
        self.controls = b""
        self.collection_errors = []
        os.set_blocking(OUT_FD, False)
        os.set_blocking(self.entry.stdin.fileno(), False)

    def queue_output(self, line):
        if self.stdout_gone:
            return
        data = encoded(line)
        if len(self.output) + len(data) > OUTPUT_LIMIT:
            self.stdout_gone = True
            self.output = b""
            self.collection_errors.append("output-backpressure")
            self.abandon("stdout not draining")
        else:
            self.output += data

    def say(self, fields):
        self.queue_output(fields)

    def flush(self):
        os.set_blocking(OUT_FD, False)
        for fd, attribute in ((OUT_FD, "output"), (self.entry.stdin.fileno(), "controls")):
            data = getattr(self, attribute)
            if not data:
                continue
            try:
                count = os.write(fd, data)
                setattr(self, attribute, data[count:])
            except BlockingIOError:
                pass
            except OSError:
                setattr(self, attribute, b"")
                if attribute == "output":
                    self.stdout_gone = True
                    self.abandon("stdout-gone")

    def to_entry(self, value):
        data = encoded(value)
        if len(self.controls) + len(data) > CONTROL_LIMIT:
            return False
        self.controls += data
        return True

    def cancel(self, why):
        if self.cancel_at is None:
            self.cancel_at = self.clock()
            self.kill_at = self.cancel_at + self.grace
            self.why = why
            sent = self.to_entry({"cmd": "cancel"})
            self.say({"frontdoor": "cancel", "why": why, "sent": sent, "kill_after_s": self.grace})

    def abandon(self, why):
        self.cancel(f"abandoned: {why}")

    def kill(self):
        if not self.killed and self.entry.poll() is None:
            # The namespace's init, this process's own unreaped child.
            os.kill(self.entry.pid, signal.SIGKILL)
            self.killed = True
            self.say({"frontdoor": "killed", "why": self.why})

    def entry_line(self, line):
        self.queue_output(line)
        if self.staged_removed is None and b'"setup-completed"' in line:
            try:
                value = json.loads(line)
            except ValueError:
                value = None
            if isinstance(value, dict) and value.get("entry") == "setup-completed":
                self.remove_staged()

    def remove_staged(self):
        path = os.path.join(self.run, CREDENTIAL_FILES[0])
        try:
            unlink_beneath(self.run, CREDENTIAL_FILES[0])
            self.staged_removed = "removed"
        except FileNotFoundError:
            self.staged_removed = "absent"
        except OSError as error:
            self.staged_removed = type(error).__name__
        self.say({"frontdoor": "staged-credential", "result": self.staged_removed})

    def requester_line(self, line):
        value = control(line)
        if value is None:
            self.say({"frontdoor": "control-refused", "reason": "not cancel, close or send"})
        else:
            if value["cmd"] in ("cancel", "close"):
                # Native close/cancel may never complete. Arm before I/O.
                if self.cancel_at is None:
                    self.cancel_at = self.clock()
                    self.kill_at = self.cancel_at + self.grace
                    self.why = "requester " + value["cmd"]
            if not self.to_entry(value):
                self.say({"frontdoor": "control-undelivered", "cmd": value["cmd"]})
                self.abandon("control backpressure")

    def step(self, stdin_fd, timeout=0.2):
        self.flush()
        while b"\n" in self.stdin_buffer:
            line, self.stdin_buffer = self.stdin_buffer.split(b"\n", 1)
            self.requester_line(line)
        now = self.clock()
        if self.signals:
            self.abandon(f"signal {self.signals[0]}")
        if self.cancel_at is None and now >= self.deadline:
            self.cancel("deadline")
        if self.kill_at is not None and now >= self.kill_at:
            self.kill()
        readers = [] if self.out_eof else [self.entry.stdout.fileno()]
        if self.stdin_open:
            readers.append(stdin_fd)
        if not readers:
            time.sleep(min(timeout, 0.05))
            return
        try:
            ready = select.select(readers, [], [], timeout)[0]
        except InterruptedError:
            return
        if self.entry.stdout.fileno() in ready:
            chunk = os.read(self.entry.stdout.fileno(), 65536)
            if not chunk:
                self.out_eof = True
                if self.out_buffer:
                    self.entry_line(self.out_buffer)
                    self.out_buffer = b""
            else:
                self.out_buffer += chunk
                if len(self.out_buffer) > OUTPUT_LIMIT:
                    self.out_buffer = b""
                    self.collection_errors.append("entry-line-too-long")
                    self.abandon("unterminated entry output")
                while b"\n" in self.out_buffer:
                    line, self.out_buffer = self.out_buffer.split(b"\n", 1)
                    self.entry_line(line)
        if stdin_fd in ready:
            chunk = os.read(stdin_fd, 65536)
            if not chunk:
                self.stdin_open = False
                if self.entry.poll() is None:
                    self.abandon("requester stdin closed")
            else:
                self.stdin_buffer += chunk
                while b"\n" in self.stdin_buffer:
                    line, self.stdin_buffer = self.stdin_buffer.split(b"\n", 1)
                    self.requester_line(line)
                if len(self.stdin_buffer) > CONTROL_LIMIT:
                    self.stdin_buffer = b""
                    self.say({"frontdoor": "control-refused", "reason": "control line too long"})

    def run_to_end(self, stdin_fd):
        ended_at = None
        while True:
            self.step(stdin_fd, timeout=0.02)
            now = self.clock()
            status = self.entry.poll()
            if self.out_eof and status is not None:
                self.flush()
                return status
            if (self.out_eof or status is not None) and ended_at is None:
                ended_at = now
            if ended_at is not None and now >= ended_at + self.grace:
                self.why = self.why or "entry/output ended without complete collection"
                self.kill()
                self.collection_errors.append("entry-collection-incomplete")
                return self.entry.poll()
            if self.kill_at is not None and now >= self.kill_at + COLLECTION_S:
                self.collection_errors.append("stop-or-eof-not-observed")
                return self.entry.poll()

    def finish_output(self):
        end = self.clock() + COLLECTION_S
        while self.output and self.clock() < end:
            self.flush()
            time.sleep(0.01)
        return not self.output and not self.stdout_gone


def entry_exit(status, killed):
    if killed:
        return EXIT_KILLED
    if status is None or status < 0:
        return EXIT_UNKNOWN
    return status


def read_first_line(fd, limit, timeout=ADMISSION_S):
    """The first line from `fd` and whatever followed it, read unbuffered."""
    data = b""
    end = time.monotonic() + timeout
    while b"\n" not in data:
        remaining = end - time.monotonic()
        if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
            raise Refused("request admission timed out")
        if len(data) > limit:
            raise Refused("request line too long")
        chunk = os.read(fd, 65536)
        if not chunk:
            raise Refused("request line not terminated")
        data += chunk
    line, rest = data.split(b"\n", 1)
    if len(line) > limit:
        raise Refused("request line too long")
    return line, rest


def parse_argv(argv):
    if argv[1:] == ["run"]:
        return SITE_CONFIG
    # The sudoers rule pins `run`; only a root caller can name another
    # config, and it must still be root's own.
    if len(argv) == 4 and argv[1] == "--site-config" and argv[3] == "run":
        return argv[2]
    return None


def admit(argv, environ, stdin_fd, now):
    """Every check before any effect: (package, site, user, checked, env,
    leftover stdin)."""
    site_path = parse_argv(argv)
    if site_path is None:
        raise Refused("usage: run")
    package = package_root()
    check_package(package)
    site = load_site(site_path)
    user = requester(site, environ)
    line, rest = read_first_line(stdin_fd, REQUEST_LIMIT)
    now = time.time()
    try:
        request = json.loads(line)
    except ValueError:
        raise Refused("request is not JSON") from None
    checked = check_request(request, site, now)
    env = root_env(user, site, checked["extra_env"])
    check_cwd_as(user, checked["cwd"])
    if checked["credential"] is not None:
        checked["credential"] = check_credential(checked["route"], checked["credential"][0], checked["deadline"], site, time.time())
    return package, site, user, checked, env, rest


def run_locked(argv, environ, stdin_fd=0):
    try:
        package, site, user, checked, env, rest = admit(argv, environ, stdin_fd, time.time())
    except Refused as refusal:
        emit({"frontdoor": "terminal", "stage": "refused", "reason": str(refusal), "effects": "none"})
        return EXIT_REFUSED
    except (OSError, ValueError, TypeError, OverflowError) as error:
        emit({"frontdoor": "terminal", "stage": "refused", "reason": type(error).__name__, "effects": "none"})
        return EXIT_REFUSED

    credential, credential_public = checked["credential"] or (None, None)
    checked["credential"] = None
    children = checked["children"]
    child_grant, child_public = (children or {}).get("grant") or (None, None)
    if children is not None:
        children["grant"] = (None, child_public) if child_grant is not None else None
    try:
        run_id, run_dir, lock, swept = make_run(site, user)
    except (Refused, OSError) as error:
        emit({"frontdoor": "terminal", "stage": "run-failed", "reason": type(error).__name__, "effects": "possible"})
        return EXIT_RUN_FAILED
    for record in swept:
        emit({"frontdoor": "swept", **record})
    entry = None
    relay = None
    try:
        try:
            write_private(os.path.join(run_dir, "private", "retention"), checked["retention"])
            if credential is not None:
                write_private(os.path.join(run_dir, "private", "auth.json"), credential)
            if child_grant is not None:
                # One snapshot for the root's life; retired with the run.
                write_private(os.path.join(run_dir, "private", "child-auth.json"), child_grant)
                child_grant = None
            request_path = os.path.join(run_dir, "private", "request.json")
            write_private(request_path, entry_request(package, run_dir, user, checked, env, credential is not None))
            credential = None
        except OSError as error:
            raise RunFailed(f"run setup: {type(error).__name__}") from None
        emit({
            "frontdoor": "admitted",
            "run": run_id,
            "run_dir": run_dir,
            "requester": {"user": user.pw_name, "uid": user.pw_uid},
            "route": checked["route_name"],
            "harness": checked["route"].get("harness", "opencode"),
            "model": checked["route"]["model"],
            "bash": "trusted-task" if "bash_authority" in checked["policy"] else {"allow": checked["policy"]["bash_allow"]},
            "env_names": sorted(env),
            "credential": credential_public,
            "children": None if children is None else {
                "routes": sorted(children["routes"]),
                "models": {name: child["model"] for name, child in children["routes"].items()},
                "max_starts": children["max_starts"],
                "max_concurrent": children["max_concurrent"],
                "depth": 1,
                "credential": child_public,
                "credential_copies": "private/child-auth.json once; each child's launch copies it; normal-path cleanup on known entry end; unknown stop or abrupt loss defers to sweep",
            },
            "deadline_s": checked["deadline"],
            "cancel_grace_s": site["cancel_grace_s"],
            "retention": checked["retention"],
            "containment": "entry is PID 1 of a new PID and mount namespace and dies with this process",
        })
        if credential_public is not None:
            # Setup elapsed since admission; recheck just before native launch.
            with open(os.path.join(run_dir, "private", "auth.json")) as file:
                check_credential(checked["route"], json.load(file), checked["deadline"], site, time.time())
        if child_public is not None:
            with open(os.path.join(run_dir, "private", "child-auth.json")) as file:
                staged = json.load(file)
            check_credential({"model": child_public["provider"] + "/child", "credential": "required"}, staged, checked["deadline"], site, time.time())
        try:
            entry, alive = start_entry(package, request_path)
        except (OSError, subprocess.SubprocessError) as error:
            raise RunFailed(f"entry not started: {type(error).__name__}") from None
        relay = Relay(entry, run_dir, checked["deadline"] + site["cancel_grace_s"], site["cancel_grace_s"])
        relay.stdin_buffer = rest
        for number in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
            signal.signal(number, lambda number, frame: relay.signals.append(number))
        status = relay.run_to_end(stdin_fd)
        os.close(alive)
        code = entry_exit(status, relay.killed)
        retired = retire(run_dir, checked["retention"]) if status is not None else {"ok": False, "stop": "unknown", "run_removed": False}
        if relay.collection_errors:
            code = EXIT_UNKNOWN
        if status is not None and (not retired["ok"] or any(not record["ok"] for record in swept)):
            code = EXIT_CLEANUP_FAILED
        relay.say({
            "frontdoor": "terminal",
            "stage": "ended",
            "run": run_id,
            "entry_status": status,
            "killed": relay.killed,
            "cancel": relay.why,
            "staged_credential": relay.staged_removed,
            "retire": retired,
            "swept": swept,
            "collection_errors": relay.collection_errors,
            "exit": code,
            "retry": "do-not-replay",
        })
        if not relay.finish_output():
            return EXIT_UNKNOWN
        return code
    except BaseException as failure:
        killed = False
        status = None
        if entry is not None:
            if entry.poll() is None:
                os.kill(entry.pid, signal.SIGKILL)
                killed = True
            end = time.monotonic() + COLLECTION_S
            while entry.poll() is None and time.monotonic() < end:
                time.sleep(0.01)
            status = entry.poll()
        retired = retire(run_dir, checked["retention"]) if entry is None or status is not None else {"ok": False, "stop": "unknown"}
        stage = "run-failed" if isinstance(failure, RunFailed) else "front-door-failed"
        emit({
            "frontdoor": "terminal",
            "stage": stage,
            "run": run_id,
            "reason": str(failure) if isinstance(failure, RunFailed) else type(failure).__name__,
            "entry_status": status,
            "killed": killed,
            "retire": retired,
            "effects": "possible",
            "retry": "do-not-replay",
        })
        if status is None and entry is not None:
            return EXIT_UNKNOWN
        if not retired["ok"]:
            return EXIT_CLEANUP_FAILED
        if entry is None:
            return EXIT_RUN_FAILED
        return EXIT_KILLED if killed else EXIT_UNKNOWN
    finally:
        os.close(lock)


def run(argv, environ, stdin_fd=0):
    package_lock = None
    try:
        package = package_root()
        check_owned(package)
        package_lock = os.open(package, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        fcntl.flock(package_lock, fcntl.LOCK_SH | fcntl.LOCK_NB)
        return run_locked(argv, environ, stdin_fd)
    except (Refused, OSError, ValueError, TypeError) as error:
        emit({"frontdoor": "terminal", "stage": "refused", "reason": type(error).__name__, "effects": "none"})
        return EXIT_REFUSED
    finally:
        if package_lock is not None:
            os.close(package_lock)


if __name__ == "__main__":
    sys.exit(run(sys.argv, os.environ))
