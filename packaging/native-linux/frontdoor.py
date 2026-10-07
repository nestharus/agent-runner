#!/usr/bin/python3 -I
"""Linux native ACP v2 bounded privileged front door.

Root-owned assets, registered-provider site routes and requester identity
determine launch; stdin v1 JSON supplies task, cwd, policy, env and deadline.
The entry negotiates describe, policy.evaluate and resident.prepare with
external adapters. Their settings/environment/credentials stay adapter-owned.
Registered children are explicit offers with bounded starts and concurrency.

Nonblocking control/output queues keep timers independent of consumers.
Accepted owner close/durable cancel arms a grace-relative kill. Instance stop
arms directly; absent acceptance the independent deadline is the backstop.
Admission and collection are bounded; kernel/FS stalls remain outside
a finite guarantee. Run discard uses descriptor-safe removal.
Native exit status or 90 refusal, 91 setup failure, 92 kill requested,
93 stop/collection unknown, 94 cleanup/residue failed. No replay.
Physical entry wait permits package run cleanup only. Discard removes the
run and store after drain, reporting the retained logical account or unknown
with do-not-replay. Keep preserves diagnostics. Neither removal nor closed/7
(native87) establishes logical retirement.
Controls: cancel/close/send/inspect lines and session_control/v2 `request`
records, whose requester must be this attested requester (`uid:<n>`).
Stdin `{"v":1,"op":"discover"}` instead lists this requester's roots as v2
`root_entry` records: derived addressing, not ownership or capacity.
Task output is unredacted. Full contract and limits: share/README.md.
"""

import ctypes
from collections import deque
import fcntl
import hashlib
import json
import os
import pwd
import re
import secrets
import select
import shutil
import signal
import socket
import stat
import struct
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

EXIT_REFUSED = 90
EXIT_RUN_FAILED = 91
EXIT_KILLED = 92
EXIT_UNKNOWN = 93
EXIT_CLEANUP_FAILED = 94

# Live roots (request `live: true`): this front door detaches from its
# opening caller and stays the root's one owning supervisor until close,
# cancel or its deadline. Later callers of the same requester address it
# through its socket in the run directory.
LIVE_SOCKET = "live.sock"
MAX_LIVE_ROOTS = 4
LIVE_RESERVATION = "live-reservation"
HELLO_S = 5
HELLO_LIMIT = 4096

RUNNER = "bin/oulipoly-agent-runner"
SUPERVISOR = "bin/oulipoly-root-supervisor"
PID1 = "bin/oulipoly-root-pid1"
ROOT_CHILD = "bin/oulipoly-root-child"
BASH_BIN = "agent-bash/agent-bash"

# session_control/v2 (agent-provider-contract): record line bound and the
# requester name the root owner answers for this front door's requester.
CONTROL_PROTOCOL = "oulipoly.session_control/v2"
CONTROL_RECORD_LIMIT = 32768
# Final logical account survives in the package terminal even under discard.
ROOT_TERMINAL = "root-terminal.json"


def control_requester(uid):
    return f"uid:{uid}"


# Names a requester may not put in the root environment: it reaches the
# owner and root PID 1 (root) as well as the work. Loader and libc names
# glibc itself drops for privileged programs, and names the owner, native
# host or sudo own.
UNSAFE_ENV = frozenset(
    """GCONV_PATH GETCONF_DIR GLIBC_TUNABLES HOSTALIASES LOCALDOMAIN LOCPATH
    MALLOC_TRACE NIS_PATH NLSPATH RESOLV_HOST_CONF RES_OPTIONS TMPDIR TZDIR
    HOME USER LOGNAME SHELL""".split()
)
UNSAFE_PREFIXES = ("LD_", "MALLOC_", "GLIBC_", "OULIPOLY_", "AGENT_BASH_", "SUDO_")
ENV_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]{0,127}")
ENV_LIMIT = 64
ENV_VALUE_LIMIT = 32 * 1024

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
    for name in (RUNNER, SUPERVISOR, PID1, ROOT_CHILD, BASH_BIN):
        path = os.path.join(root, name)
        if not os.path.isfile(path) or not os.access(path, os.X_OK):
            raise Refused(f"package: no executable {name}")
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
        "allow_keep", "default_path", "routes",
        "child_routes", "child_limits",
    }
    if not isinstance(site, dict) or site.get("v") != 1 or set(site) - known:
        raise Refused("site config: not a v1 config")
    users = site.get("allowed_users")
    if not isinstance(users, list) or not all(isinstance(user, str) and user for user in users):
        raise Refused("site config: allowed_users")
    if not isinstance(site.get("run_base"), str) or not site["run_base"].startswith("/"):
        raise Refused("site config: run_base")
    for name, default in (("max_deadline_s", 7200), ("cancel_grace_s", 30)):
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
        if not provider_route(route):
            raise Refused(f"site config: route {name}: expected registered provider shape")
    check_site_children(site)
    return site


def check_site_children(site):
    """`child_routes` (registered-provider routes a child may
    run on), each parent route's `children` (the child routes it may offer)
    and `child_limits` (at most the owner's ceilings)."""
    child_routes = site.setdefault("child_routes", {})
    if not isinstance(child_routes, dict):
        raise Refused("site config: child_routes")
    for name, route in child_routes.items():
        if (
            not isinstance(name, str)
            or not re.fullmatch(r"[A-Za-z0-9._-]{1,64}", name)
            or not provider_route(route)
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


def provider_route(route):
    """A registered external provider route: the provider's absolute
    executable (root-owned custody is checked at each admission), its
    `settings` (provider/v1 `policy.evaluate` params, opaque here), an
    optional absolute `config_root` and `env` for the provider's own
    operations. No credential."""
    if not isinstance(route, dict) or route.get("harness") != "provider":
        return False
    executable = route.get("executable")
    settings = route.get("settings")
    env = route.get("env", {})
    config_root = route.get("config_root", "/")
    return (
        not set(route) - {"harness", "executable", "settings", "config_root", "env", "children"}
        and {"harness", "executable", "settings"} <= set(route)
        and isinstance(executable, str)
        and executable.startswith("/")
        and "\0" not in executable
        and os.path.normpath(executable) == executable
        and isinstance(settings, dict)
        and {"settings_id", "mode", "model"} <= set(settings)
        and not set(settings) - {"settings_id", "mode", "model", "launch"}
        and isinstance(config_root, str)
        and config_root.startswith("/")
        and isinstance(env, dict)
        and all(
            isinstance(name, str) and ENV_NAME.fullmatch(name) and not name.startswith("OULIPOLY_")
            and isinstance(value, str) and "\0" not in value
            for name, value in env.items()
        )
    )


def check_provider_executable(route):
    """A provider route's executable: root's custody (the file and every
    directory above it) and an executable regular file. Checked at each
    admission, before any effect; the entry checks it again and binds it to
    what it runs."""
    if route.get("harness") != "provider":
        return
    path = route["executable"]
    try:
        check_owned(path)
        st = os.stat(path)
    except OSError as error:
        raise Refused(f"provider: {path}: {type(error).__name__}") from None
    if not stat.S_ISREG(st.st_mode) or not st.st_mode & 0o111:
        raise Refused(f"provider: {path} is not an executable file")


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
    known = {"v", "route", "message", "cwd", "bash", "env", "deadline_s", "retention",
             "children", "live"}
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
    live = request.get("live", False)
    if type(live) is not bool:
        raise Refused("request: live must be true or false")
    children = check_children(route, request, site)
    return {
        "route_name": route_name,
        "route": route,
        "message": message,
        "cwd": os.path.normpath(cwd),
        "policy": policy,
        "extra_env": request.get("env", {}),
        "deadline": deadline,
        "retention": retention,
        "children": children,
        "live": live,
    }


def check_children(route, request, site):
    """Requester opt-in within the registered child offers and ceilings."""
    asked = request.get("children")
    if asked is None:
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
    return {"routes": child_routes, **limits}


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


def root_terminal(run):
    """Last retained owner account, or explicit unknown; no replay inference."""
    try:
        fd = os.open(os.path.join(run, "private", ROOT_TERMINAL),
                     os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
        with os.fdopen(fd, "r", encoding="utf-8") as file:
            if not stat.S_ISREG(os.fstat(file.fileno()).st_mode):
                raise ValueError("not regular")
            value = json.loads(file.read(OUTPUT_LIMIT + 1))
            if not isinstance(value, dict):
                raise ValueError("not an account")
            return value
    except (OSError, ValueError):
        return {"knowledge": "unknown", "session_control": {"retirement": {
            "eligible": False, "blocking": ["no final owner account available"]}}}


def retire(run, retention):
    """Remove package scratch after physical entry termination. Logical
    retirement stays in the returned account; removal never settles it.
    Keep preserves the store and adapter diagnostics. This layer knows no
    adapter credential paths or semantics. Caller establishes physical drain.
    """
    removed = None
    account = root_terminal(run)
    if retention == "discard":
        try:
            if not shutil.rmtree.avoids_symlink_attacks:
                raise OSError("descriptor-safe tree removal unavailable")
            shutil.rmtree(run)
            removed = True
        except OSError as error:
            removed = type(error).__name__
    return {"ok": retention != "discard" or removed is True, "run_removed": removed,
            "root_terminal": account, "retry": "do-not-replay",
            "meaning": "physical run disposition after drain; logical retirement is separately reported"}


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


def child_route_request(package, child):
    """Registered child with opaque settings/env and the generic Bash requester."""
    registered = {
        "executable": child["executable"],
        "settings": child["settings"],
        "env": child.get("env", {}),
        "agent_bash_bin": os.path.join(package, BASH_BIN),
    }
    if "config_root" in child:
        registered["config_root"] = child["config_root"]
    return {"registered": registered}


def children_request(package, run, checked):
    children = checked.get("children")
    if children is None:
        return {}
    value = {
        "routes": {name: child_route_request(package, child) for name, child in children["routes"].items()},
        "max_starts": children["max_starts"],
        "max_concurrent": children["max_concurrent"],
    }
    return {"children": value}


def entry_request(package, run, user, checked, env):
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
    provider = {
        "executable": route["executable"],
        "settings": route["settings"],
        "env": route.get("env", {}),
        "agent_bash_bin": os.path.join(package, BASH_BIN),
        **checked["policy"],
    }
    if "config_root" in route:
        provider["config_root"] = route["config_root"]
    if common.get("children", {}).get("routes"):
        provider["root_child_bin"] = os.path.join(package, ROOT_CHILD)
    return dict(common, provider=provider)


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
    if value in ({"cmd": "cancel"}, {"cmd": "close"}, {"cmd": "inspect"}):
        return value
    if isinstance(value, dict) and value.get("cmd") == "inspect" \
            and set(value) == {"cmd", "inspection_key"} \
            and isinstance(value["inspection_key"], str) \
            and 1 <= len(value["inspection_key"]) <= 128 \
            and all(33 <= ord(c) <= 126 for c in value["inspection_key"]):
        return value
    if (
        isinstance(value, dict)
        and value.get("kind") == "request"
        and value.get("protocol") == CONTROL_PROTOCOL
        and "cmd" not in value
        and len(line) <= CONTROL_RECORD_LIMIT
    ):
        # The owner validates the record; this layer attests its requester.
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
        # Background Bash completions the owner still owes its live harness
        # (its `async-owed` events). A requester close arms the kill grace
        # only once none is owed; the deadline bounds the wait regardless.
        self.owed_async = 0
        # This front door's attested requester, for session_control records.
        self.requester_uid = None
        # The owner's described retirement eligibility, from its terminal.
        self.retirement = None
        self.close_waiting = False
        self.why = None
        self.stdout_gone = False
        self.stdin_open = True
        self.stdin_buffer = b""
        self.out_buffer = b""
        self.out_eof = False
        self.signals = []
        self.output = b""
        self.controls = b""
        self.collection_errors = []
        self.client_setup()
        os.set_blocking(self.entry.stdin.fileno(), False)

    def client_setup(self):
        os.set_blocking(OUT_FD, False)

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

    def arm_close(self):
        if self.cancel_at is None:
            self.cancel_at = self.clock()
            self.kill_at = self.cancel_at + self.grace
            self.why = "requester close"

    def note_owner_terminal(self, line):
        """Keep the final logical account for diagnostics and disposition."""
        try:
            value = json.loads(line)
        except ValueError:
            return
        if not (isinstance(value, dict) and value.get("event") == "terminal" and "child" not in value):
            return
        retirement = (value.get("session_control") or {}).get("retirement") or {}
        self.retirement = retirement.get("eligible") is True
        account = {key: value[key] for key in ("status", "control", "session_control") if key in value}
        if "session_control" not in account:
            account["knowledge"] = "unknown"
        path = os.path.join(self.run, "private", ROOT_TERMINAL)
        try:
            write_private(path + ".next", account)
            os.replace(path + ".next", path)
        except OSError:
            self.collection_errors.append("root-terminal-account-unavailable")

    def entry_line(self, line):
        self.note_owner_terminal(line)
        self.queue_output(line)
        try:
            value = json.loads(line)
        except ValueError:
            value = None
        if isinstance(value, dict) and "child" not in value:
            if value.get("event") == "close-requested":
                # This is the owner effect after durable acknowledgment.
                self.close_waiting = self.owed_async > 0
                if not self.close_waiting:
                    self.arm_close()
            elif value.get("event") == "cancel-requested" and value.get("by") == "session-control":
                if self.cancel_at is None:
                    self.cancel_at = self.clock()
                    self.kill_at = self.cancel_at + self.grace
                    self.why = "requester durable cancel"
        if b'"async-owed"' in line:
            try:
                value = json.loads(line)
            except ValueError:
                value = None
            if isinstance(value, dict) and value.get("event") == "async-owed" and "child" not in value \
                    and type(value.get("owed_async")) is int:
                self.owed_async = value["owed_async"]
                if self.owed_async == 0 and self.close_waiting:
                    self.close_waiting = False
                    self.arm_close()
    def requester_line(self, line):
        value = control(line)
        if value is None:
            self.say({"frontdoor": "control-refused", "reason": "not cancel, close, send, inspect or a session_control request"})
        else:
            if value.get("kind") == "request":
                if self.requester_uid is None or value.get("requester") != control_requester(self.requester_uid):
                    self.say({"frontdoor": "control-refused", "reason": "requester-not-attested",
                              "request_key": value.get("request_key") if isinstance(value.get("request_key"), str) else None})
                    return
                command = None
            else:
                command = value["cmd"]
            if command == "cancel":
                # Explicit owner-instance stop; no durable control claim.
                if self.cancel_at is None:
                    self.cancel_at = self.clock()
                    self.kill_at = self.cancel_at + self.grace
                    self.why = "requester " + command
            if not self.to_entry(value):
                self.say({"frontdoor": "control-undelivered", "cmd": command or value.get("kind")})
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
            self.read_entry()
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

    def read_entry(self):
        chunk = os.read(self.entry.stdout.fileno(), 65536)
        if not chunk:
            self.out_eof = True
            if self.out_buffer:
                self.entry_line(self.out_buffer)
                self.out_buffer = b""
            return
        self.out_buffer += chunk
        if len(self.out_buffer) > OUTPUT_LIMIT:
            self.out_buffer = b""
            self.collection_errors.append("entry-line-too-long")
            self.abandon("unterminated entry output")
        while b"\n" in self.out_buffer:
            line, self.out_buffer = self.out_buffer.split(b"\n", 1)
            self.entry_line(line)

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


def peer_uid(conn):
    _pid, uid, _gid = struct.unpack("3i", conn.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")))
    return uid


def listen_live(run, uid):
    """The live root's address: a socket in the root-owned run directory,
    owned by the requester and mode 0600; every connection's peer is
    still checked (SO_PEERCRED) against the requester's uid."""
    path = os.path.join(run, LIVE_SOCKET)
    if len(os.fsencode(path)) >= 108:
        raise RunFailed("live socket path too long")
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM | socket.SOCK_CLOEXEC)
    try:
        listener.bind(path)
        os.chmod(path, 0o600)
        if os.geteuid() == 0:
            os.chown(path, uid, -1, follow_symlinks=False)
        listener.listen(4)
    except BaseException:
        listener.close()
        raise
    return listener, path


def live_roots(user_dir):
    """Live sockets or in-progress reservations with a held run lock.
    A dead owner's lock is free and no longer counts."""
    try:
        names = os.listdir(user_dir)
    except FileNotFoundError:
        return 0
    count = 0
    for name in names:
        run = os.path.join(user_dir, name)
        if not (os.path.lexists(os.path.join(run, LIVE_SOCKET))
                or os.path.lexists(os.path.join(run, "private", LIVE_RESERVATION))):
            continue
        try:
            fd = os.open(os.path.join(run, "private", "lock"), os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
        except OSError:
            continue
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            count += 1
        finally:
            os.close(fd)
    return count


def make_live_run(site, user):
    """Serialize check/allocation per requester, and reserve the slot before
    releasing the guard. A locked reservation counts even before listen/fork.
    Dead reservations are lock-free and swept like dead live sockets."""
    base = site["run_base"]
    check_owned(base)
    user_dir = os.path.join(base, str(user.pw_uid))
    try:
        os.mkdir(user_dir, 0o711)
    except FileExistsError:
        pass
    check_owned(user_dir)
    guard = os.open(user_dir, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        try:
            fcntl.flock(guard, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise Refused("live roots: requester admission busy") from None
        if live_roots(user_dir) >= MAX_LIVE_ROOTS:
            raise Refused(f"live roots: {MAX_LIVE_ROOTS} already held by this requester")
        allocated = make_run(site, user)
        _, run, lock, _ = allocated
        try:
            write_private(os.path.join(run, "private", LIVE_RESERVATION), "reserved")
        except OSError:
            retire(run, "discard")
            os.close(lock)
            raise
        return allocated
    finally:
        os.close(guard)


class LiveRelay(Relay):
    """The live root's relay. No requester stdin: callers attach through
    the root's socket one at a time; a caller leaving (EOF or `detach`)
    is not abandonment. While no caller is attached, records wait in a
    bounded backlog (overflow is dropped and counted, never silently).
    Close, cancel and the deadline keep the per-call relay's meaning."""

    def __init__(self, entry, run, deadline_s, grace_s, listener, uid, run_id, token, clock=time.monotonic):
        self.listener = listener
        self.uid = uid
        self.run_id = run_id
        self.token = token
        self.client = None
        self.client_in = b""
        self.waiting = None
        self.dropped = 0
        self.dropped_total = 0
        self.attaches = 0
        self.refusals = []
        super().__init__(entry, run, deadline_s, grace_s, clock)
        self.output = deque()
        self.output_bytes = 0
        self.send_offset = 0
        self.interrupted_total = 0
        self.interrupted_prefix_bytes = 0
        self.dropped_bytes_total = 0
        # Runtime-only accounting witness across attachments; no answer bodies.
        self.account_events = []
        self.account_bytes = 0
        self.account_errors = []
        self.requester_uid = uid

    def entry_line(self, line):
        try:
            value = json.loads(line)
        except ValueError:
            value = None
        if isinstance(value, dict) and "child" not in value and (
                value.get("event") in ("async-owed", "bash-async-completion-admitted", "ack", "turn-end", "terminal")
                or value.get("entry") == "terminal"):
            size = len(encoded(value))
            if self.account_bytes + size <= OUTPUT_LIMIT:
                self.account_events.append(value)
                self.account_bytes += size
            elif not self.account_errors:
                self.account_errors.append("live-account-limit")
        super().entry_line(line)

    def client_setup(self):
        self.listener.setblocking(False)

    def drop_record(self, data, interrupted=False):
        self.dropped += 1
        self.dropped_total += 1
        self.dropped_bytes_total += len(data)
        if interrupted:
            self.interrupted_total += 1
            self.interrupted_prefix_bytes += self.send_offset

    def queue_output(self, line, force=False):
        data = encoded(line)
        try:
            value = json.loads(data)
            if not isinstance(value, dict):
                raise ValueError
        except ValueError:
            self.drop_record(data)
            self.collection_errors.append("invalid-entry-record")
            return
        if not force and self.output_bytes + len(data) > OUTPUT_LIMIT:
            self.drop_record(data)
            return
        self.output.append(data)
        self.output_bytes += len(data)

    def say(self, fields):
        self.queue_output(fields, force=True)

    def flush(self):
        if self.controls:
            try:
                count = os.write(self.entry.stdin.fileno(), self.controls)
                self.controls = self.controls[count:]
            except BlockingIOError:
                pass
            except OSError:
                self.controls = b""
        if self.client is not None and self.output:
            try:
                record = self.output[0]
                count = self.client.send(record[self.send_offset:])
                if count == 0:
                    self.detach("caller-gone")
                    return
                self.send_offset += count
                if self.send_offset == len(record):
                    self.output.popleft()
                    self.output_bytes -= len(record)
                    self.send_offset = 0
            except BlockingIOError:
                pass
            except OSError:
                self.detach("caller-gone")

    def refuse_conn(self, conn, reason):
        self.refusals.append(reason)
        try:
            conn.send(encoded({"frontdoor": "attach-refused", "reason": reason, "retry": "do-not-replay"}))
        except OSError:
            pass
        conn.close()

    def accept(self):
        try:
            conn, _ = self.listener.accept()
        except (BlockingIOError, InterruptedError):
            return
        conn.setblocking(False)
        try:
            uid = peer_uid(conn)
        except OSError:
            return self.refuse_conn(conn, "peer-unknown")
        if uid != self.uid:
            return self.refuse_conn(conn, "foreign-requester")
        if self.client is not None or self.waiting is not None:
            return self.refuse_conn(conn, "busy")
        self.waiting = (conn, b"", self.clock())

    def hello(self, data):
        conn, buffer, at = self.waiting
        if data == b"":
            self.waiting = None
            conn.close()
            return
        buffer += data
        if b"\n" not in buffer:
            if len(buffer) > HELLO_LIMIT:
                self.waiting = None
                self.refuse_conn(conn, "hello-too-long")
            else:
                self.waiting = (conn, buffer, at)
            return
        line, rest = buffer.split(b"\n", 1)
        self.waiting = None
        try:
            value = json.loads(line)
        except ValueError:
            value = None
        attach = value.get("attach") if isinstance(value, dict) and value.get("v") == 1 and set(value) == {"v", "attach"} else None
        if not isinstance(attach, dict) or set(attach) != {"run", "token"} \
                or not all(isinstance(attach[k], str) for k in ("run", "token")):
            return self.refuse_conn(conn, "hello-malformed")
        if attach["run"] != self.run_id:
            return self.refuse_conn(conn, "not-this-root")
        if not secrets.compare_digest(attach["token"].encode(), self.token.encode()):
            return self.refuse_conn(conn, "handle-token")
        self.attaches += 1
        record = {
            "frontdoor": "attached", "run": self.run_id, "attach": self.attaches,
            "backlog_bytes": self.output_bytes, "backlog_dropped": self.dropped,
            "backlog_dropped_total": self.dropped_total,
            "interrupted_records_total": self.interrupted_total,
            "interrupted_prefix_bytes": self.interrupted_prefix_bytes,
            "dropped_record_bytes_total": self.dropped_bytes_total,
            "closing": self.cancel_at is not None or self.close_waiting,
            "meaning": "attached to the one live owner; records since the previous caller left follow, then live records",
        }
        self.dropped = 0
        data = encoded(record)
        self.output.appendleft(data)
        self.output_bytes += len(data)
        self.client = conn
        self.client_in = rest

    def detach(self, why):
        if self.client is None:
            return
        try:
            self.client.close()
        except OSError:
            pass
        self.client = None
        self.client_in = b""
        # Discard the entire interrupted record, including its old prefix.
        # A new attachment must never start with a previous caller's byte tail.
        if self.send_offset:
            data = self.output.popleft()
            self.drop_record(data, interrupted=True)
            self.output_bytes -= len(data)
            self.send_offset = 0
        # Whole unsent records stay for the next caller.
        self.say({"frontdoor": "detached", "why": why, "root": "live" if self.entry.poll() is None else "ending"})

    def client_lines(self):
        while self.client is not None and b"\n" in self.client_in:
            line, self.client_in = self.client_in.split(b"\n", 1)
            try:
                value = json.loads(line)
            except ValueError:
                value = None
            if value == {"cmd": "detach"}:
                self.detach("caller detach")
            else:
                self.requester_line(line)
        if self.client is not None and len(self.client_in) > CONTROL_LIMIT:
            self.client_in = b""
            self.say({"frontdoor": "control-refused", "reason": "control line too long"})

    def step(self, stdin_fd=None, timeout=0.2):
        self.flush()
        now = self.clock()
        if self.signals:
            self.abandon(f"signal {self.signals[0]}")
        if self.cancel_at is None and now >= self.deadline:
            self.cancel("deadline")
        if self.kill_at is not None and now >= self.kill_at:
            self.kill()
        if self.waiting is not None and now >= self.waiting[2] + HELLO_S:
            conn = self.waiting[0]
            self.waiting = None
            self.refuse_conn(conn, "hello-timeout")
        readers = [] if self.out_eof else [self.entry.stdout.fileno()]
        if self.listener is not None:
            readers.append(self.listener.fileno())
        if self.client is not None:
            readers.append(self.client.fileno())
        if self.waiting is not None:
            readers.append(self.waiting[0].fileno())
        writers = [self.client.fileno()] if self.client is not None and self.output else []
        try:
            ready = select.select(readers, writers, [], timeout)[0]
        except InterruptedError:
            return
        if not self.out_eof and self.entry.stdout.fileno() in ready:
            self.read_entry()
        if self.listener is not None and self.listener.fileno() in ready:
            self.accept()
        if self.waiting is not None and self.waiting[0].fileno() in ready:
            try:
                self.hello(self.waiting[0].recv(HELLO_LIMIT))
            except BlockingIOError:
                pass
            except OSError:
                conn = self.waiting[0]
                self.waiting = None
                conn.close()
        if self.client is not None and self.client.fileno() in ready:
            try:
                chunk = self.client.recv(65536)
            except BlockingIOError:
                chunk = None
            except OSError:
                chunk = b""
            if chunk == b"":
                self.detach("caller-eof")
            elif chunk:
                self.client_in += chunk
        self.client_lines()

    def stop_listening(self, path):
        """No new caller once the entry ended; a later address is absent."""
        if self.listener is not None:
            try:
                os.unlink(path)
            except OSError:
                pass
            self.listener.close()
            self.listener = None
        if self.waiting is not None:
            conn = self.waiting[0]
            self.waiting = None
            self.refuse_conn(conn, "root-ending")

    def finish_output(self):
        end = self.clock() + COLLECTION_S
        while self.output and self.client is not None and self.clock() < end:
            self.flush()
            time.sleep(0.01)
        delivered = not self.output
        if self.client is not None:
            try:
                self.client.close()
            except OSError:
                pass
            self.client = None
        return delivered


def live_daemon(package, site, checked, run_id, run_dir, request_path, user, listener, socket_path, token, ready_w):
    """The detached owning supervisor of one live root. Returns its exit."""
    for number in (signal.SIGHUP, signal.SIGINT):
        signal.signal(number, signal.SIG_IGN)
    try:
        entry, alive = start_entry(package, request_path)
    except (OSError, subprocess.SubprocessError) as error:
        os.write(ready_w, b"failed " + type(error).__name__.encode() + b"\n")
        listener.close()
        retire(run_dir, checked["retention"])
        return EXIT_RUN_FAILED
    relay = LiveRelay(entry, run_dir, checked["deadline"] + site["cancel_grace_s"], site["cancel_grace_s"],
                      listener, user.pw_uid, run_id, token)
    signal.signal(signal.SIGTERM, lambda number, frame: relay.signals.append(number))
    os.write(ready_w, b"live\n")
    os.close(ready_w)
    try:
        status = relay.run_to_end(None)
    except BaseException as failure:
        relay.collection_errors.append("live-relay-failed:" + type(failure).__name__)
        if entry.poll() is None:
            os.kill(entry.pid, signal.SIGKILL)
            relay.killed = True
        end = time.monotonic() + COLLECTION_S
        while entry.poll() is None and time.monotonic() < end:
            time.sleep(0.01)
        status = entry.poll()
    relay.stop_listening(socket_path)
    os.close(alive)
    code = entry_exit(status, relay.killed)
    retired = retire(run_dir, checked["retention"]) if status is not None else {"ok": False, "stop": "unknown", "run_removed": False}
    if relay.collection_errors:
        code = EXIT_UNKNOWN
    if status is not None and not retired["ok"]:
        code = EXIT_CLEANUP_FAILED
    relay.say({
        "frontdoor": "terminal",
        "stage": "ended",
        "run": run_id,
        "live": {"attaches": relay.attaches, "backlog_dropped_total": relay.dropped_total,
                 "refusals": relay.refusals,
                 "interrupted_records_total": relay.interrupted_total,
                 "interrupted_prefix_bytes": relay.interrupted_prefix_bytes,
                 "dropped_record_bytes_total": relay.dropped_bytes_total},
        "account_events": relay.account_events,
        "account_errors": relay.account_errors,
        "entry_status": status,
        "killed": relay.killed,
        "cancel": relay.why,
        "retire": retired,
        "collection_errors": relay.collection_errors,
        "exit": code,
        "retry": "do-not-replay",
    })
    relay.finish_output()
    return code


def open_live(package, site, checked, run_id, run_dir, request_path, user):
    """Forks the detached owning supervisor (its own session, no caller
    stdio), waits for its readiness and answers the opening caller with
    the root's handle. This process then ends; the root does not."""
    token = secrets.token_hex(16)
    listener, socket_path = listen_live(run_dir, user.pw_uid)
    ready_r, ready_w = os.pipe()
    pid = os.fork()
    if pid == 0:
        code = EXIT_UNKNOWN
        try:
            os.close(ready_r)
            os.setsid()
            null = os.open(os.devnull, os.O_RDWR)
            for fd in (0, 1, 2):
                os.dup2(null, fd)
            os.close(null)
            if OUT_FD > 2:
                # The opening caller's channel must end with this call.
                os.close(OUT_FD)
            code = live_daemon(package, site, checked, run_id, run_dir, request_path, user, listener,
                               socket_path, token, ready_w)
        except BaseException:
            pass
        finally:
            os._exit(code & 0xFF)
    os.close(ready_w)
    listener.close()
    data = b""
    end = time.monotonic() + ADMISSION_S
    while b"\n" not in data and time.monotonic() < end:
        if not select.select([ready_r], [], [], max(0.0, end - time.monotonic()))[0]:
            break
        chunk = os.read(ready_r, 256)
        if not chunk:
            break
        data += chunk
    os.close(ready_r)
    if data != b"live\n":
        emit({"frontdoor": "terminal", "stage": "run-failed", "run": run_id,
              "reason": "live root not started: " + (data.decode(errors="replace").strip() or "no readiness"),
              "effects": "possible", "retry": "do-not-replay"})
        return EXIT_UNKNOWN if not data else EXIT_RUN_FAILED
    emit({
        "frontdoor": "terminal",
        "stage": "live-opened",
        "run": run_id,
        "supervisor_pid": pid,
        "handle": {"v": 1, "run": run_id, "socket": socket_path, "token": token, "uid": user.pw_uid},
        "deadline_s": checked["deadline"],
        "meaning": "the root's owning supervisor runs detached from this call; this exit is not the root's end; close, cancel or the deadline ends it",
        "exit": 0,
        "retry": "do-not-replay",
    })
    return 0


def run_lock_held(run):
    """Whether a front door holds this run's lock now (its owner is live)."""
    try:
        fd = os.open(os.path.join(run, "private", "lock"), os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError:
        return False
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        return True
    finally:
        os.close(fd)
    return False


def describe_root(package, store, uid):
    """The owner binary's read-only `root_entry` for one store, or why not."""
    try:
        result = subprocess.run(
            [os.path.join(package, SUPERVISOR), "--describe", store,
             "--requester", control_requester(uid), "--describer", "frontdoor"],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env=ENTRY_ENV, cwd="/", timeout=ADMISSION_S, check=False, close_fds=True)
    except (OSError, subprocess.SubprocessError) as error:
        return None, type(error).__name__
    try:
        value = json.loads(result.stdout.decode().strip().splitlines()[-1])
    except (ValueError, IndexError, UnicodeError):
        return None, "describe output unreadable"
    if result.returncode == 0 and isinstance(value, dict) and value.get("kind") == "root_entry":
        return value, None
    return None, str(value.get("reason", "describe refused")) if isinstance(value, dict) else "describe refused"


def discover(package, site, user):
    """Lists this requester's roots under this front door's run base: one
    `root_entry` (session_control/v2) per root store, read without claiming
    it, with whether this front door holds it live. Derived and rebuildable;
    not ownership, admission, scheduling or a capacity reservation. The live
    cap is per requester at this front door, not a global reservation."""
    base = site["run_base"]
    check_owned(base)
    user_dir = os.path.join(base, str(user.pw_uid))
    try:
        names = sorted(os.listdir(user_dir))
        check_owned(user_dir)
    except FileNotFoundError:
        names = []
    roots = 0
    for name in names:
        run = os.path.join(user_dir, name)
        store = os.path.join(run, "store")
        try:
            check_owned(run)
            check_owned(os.path.join(run, "private"))
        except (Refused, OSError):
            continue
        if not os.path.isdir(store) or os.path.islink(store):
            continue
        roots += 1
        entry, reason = describe_root(package, store, user.pw_uid)
        live = run_lock_held(run) and os.path.lexists(os.path.join(run, LIVE_SOCKET))
        emit({
            "frontdoor": "root",
            "run": name,
            "live": live,
            "socket": os.path.join(run, LIVE_SOCKET) if live else None,
            "front_door_holds_run": run_lock_held(run),
            "last_root_terminal": root_terminal(run),
            "entry": entry,
            "describe_error": reason,
        })
    emit({
        "frontdoor": "terminal",
        "stage": "discovered",
        "requester": control_requester(user.pw_uid),
        "roots": roots,
        "live_roots": live_roots(user_dir),
        "live_cap": {"per_requester": MAX_LIVE_ROOTS, "scope": "this requester at this front door; not a global reservation"},
        "meaning": "descriptive addresses of this requester's root stores; an entry's authority is the store's last record, not proof it is current; attaching a live root still needs its handle",
        "exit": 0,
    })
    return 0


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
    if request == {"v": 1, "op": "discover"}:
        return package, site, user, None, None, rest
    checked = check_request(request, site, now)
    check_provider_executable(checked["route"])
    for child in (checked["children"] or {}).get("routes", {}).values():
        check_provider_executable(child)
    env = root_env(user, site, checked["extra_env"])
    check_cwd_as(user, checked["cwd"])
    return package, site, user, checked, env, rest


def harness_report(route):
    """Executable and opaque settings digest for build/audit provenance only."""
    settings = json.dumps(route["settings"], sort_keys=True, separators=(",", ":")).encode()
    return {"harness": "provider", "provider": {
        "executable": route["executable"],
        "settings_sha256": hashlib.sha256(settings).hexdigest(),
        "resolved_by": "the entry: provider describe, policy.evaluate, resident.prepare",
    }}


def run_locked(argv, environ, stdin_fd=0):
    try:
        package, site, user, checked, env, rest = admit(argv, environ, stdin_fd, time.time())
    except Refused as refusal:
        emit({"frontdoor": "terminal", "stage": "refused", "reason": str(refusal), "effects": "none"})
        return EXIT_REFUSED
    except (OSError, ValueError, TypeError, OverflowError) as error:
        emit({"frontdoor": "terminal", "stage": "refused", "reason": type(error).__name__, "effects": "none"})
        return EXIT_REFUSED
    if checked is None:
        try:
            return discover(package, site, user)
        except (Refused, OSError) as error:
            emit({"frontdoor": "terminal", "stage": "refused", "reason": str(error) if isinstance(error, Refused) else type(error).__name__, "effects": "none"})
            return EXIT_REFUSED

    children = checked["children"]
    try:
        allocate = make_live_run if checked["live"] else make_run
        run_id, run_dir, lock, swept = allocate(site, user)
    except Refused as refusal:
        if checked["live"]:
            emit({"frontdoor": "terminal", "stage": "refused", "reason": str(refusal), "effects": "none"})
            return EXIT_REFUSED
        emit({"frontdoor": "terminal", "stage": "run-failed", "reason": type(refusal).__name__, "effects": "possible"})
        return EXIT_RUN_FAILED
    except OSError as error:
        emit({"frontdoor": "terminal", "stage": "run-failed", "reason": type(error).__name__, "effects": "possible"})
        return EXIT_RUN_FAILED
    for record in swept:
        emit({"frontdoor": "swept", **record})
    entry = None
    relay = None
    try:
        try:
            write_private(os.path.join(run_dir, "private", "retention"), checked["retention"])
            request_path = os.path.join(run_dir, "private", "request.json")
            write_private(request_path, entry_request(package, run_dir, user, checked, env))
        except OSError as error:
            raise RunFailed(f"run setup: {type(error).__name__}") from None
        emit({
            "frontdoor": "admitted",
            "run": run_id,
            "run_dir": run_dir,
            "requester": {"user": user.pw_name, "uid": user.pw_uid},
            "route": checked["route_name"],
            **harness_report(checked["route"]),
            "bash": "trusted-task" if "bash_authority" in checked["policy"] else {"allow": checked["policy"]["bash_allow"]},
            "env_names": sorted(env),
            "children": None if children is None else {
                "routes": sorted(children["routes"]),
                "providers": {name: harness_report(child)["provider"] for name, child in children["routes"].items()
                              if child.get("harness") == "provider"},
                "max_starts": children["max_starts"],
                "max_concurrent": children["max_concurrent"],
                "depth": 1,
            },
            "deadline_s": checked["deadline"],
            "cancel_grace_s": site["cancel_grace_s"],
            "retention": checked["retention"],
            "containment": "entry is PID 1 of a new PID and mount namespace and dies with this process"
                           + ("; live: this process detaches as the root's owning supervisor" if checked["live"] else ""),
            "live": checked["live"],
        })
        if checked["live"]:
            return open_live(package, site, checked, run_id, run_dir, request_path, user)
        try:
            entry, alive = start_entry(package, request_path)
        except (OSError, subprocess.SubprocessError) as error:
            raise RunFailed(f"entry not started: {type(error).__name__}") from None
        relay = Relay(entry, run_dir, checked["deadline"] + site["cancel_grace_s"], site["cancel_grace_s"])
        relay.requester_uid = user.pw_uid
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
