"""Offline end-to-end fixture of the packaged front door and caller, run by
`run_e2e.sh` as root of a fresh unprivileged user namespace (the invoking
user mapped to 0, subordinate ids to 1..65536) with its own network
(loopback only), mount and PID namespaces. Never run as host root.

Inside: a fresh tmpfs over /srv holds the run base, site config, project
and credential source; the built stage is bind-mounted read-only as the
package. The packaged caller drives the packaged front door (through a
test-root loader that only widens the owners trusted for the host's own
`/`, which shows as the overflow uid here; nothing else is changed), the
real runner, owner, root PID 1, agent-bash and OpenCode, against a
scripted loopback model. The work runs as user `nes` (uid 1000 inside, a
subordinate uid outside). Everything made under /tmp ends with the
namespace.

Scenarios: trusted-task Bash answered; allow-list denial; deadline
cancel of a running Bash; abandonment by stdin EOF; an inline access-only
credential (staging, removal, no leak); refusals before any effect.
"""

import argparse
import importlib.machinery
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.realpath(__file__))
sys.path.insert(0, HERE)
from scripted_model import ScriptedModel  # noqa: E402

ROOT = "/srv/oe2e"
PKG = ROOT + "/pkg"
SITE = ROOT + "/etc/frontdoor.json"
RUNS = ROOT + "/runs"
PROJECT = ROOT + "/project"
WRAPPER = ROOT + "/frontdoor-test-root.py"
MARKER = "e2e-fake-access-token-marker"
WORK_UID = 1000

LOADER = f"""import importlib.machinery, importlib.util, os, sys
loader = importlib.machinery.SourceFileLoader("frontdoor", "{PKG}/libexec/oulipoly-native-frontdoor")
spec = importlib.util.spec_from_loader("frontdoor", loader)
frontdoor = importlib.util.module_from_spec(spec)
loader.exec_module(frontdoor)
# The host's own / and /home show as the overflow uid in this namespace.
frontdoor.TRUSTED_OWNERS = frozenset({{0, 65534}})
sys.exit(frontdoor.run(sys.argv, os.environ))
"""


def sh(*argv):
    subprocess.run(argv, check=True)


def in_user_namespace():
    with open("/proc/self/uid_map") as file:
        for line in file:
            inner, outer, count = (int(x) for x in line.split())
            if inner == 0 and outer == 0:
                return False
    return os.getuid() == 0


def prepare(stage):
    sh("ip", "link", "set", "lo", "up")
    # The fixture's own root (its ancestors must be root's alone), and a
    # normal sticky /tmp for the work.
    sh("mount", "-t", "tmpfs", "-o", "mode=0755,size=256m", "tmpfs", "/srv")
    sh("mount", "-t", "tmpfs", "-o", "mode=1777,size=256m", "tmpfs", "/tmp")
    for path, mode in ((ROOT, 0o755), (PKG, 0o755), (ROOT + "/etc", 0o755), (RUNS, 0o711), (ROOT + "/private", 0o700)):
        os.mkdir(path, mode)
        os.chmod(path, mode)
    sh("mount", "--bind", stage, PKG)
    sh("mount", "-o", "remount,bind,ro", PKG)
    os.mkdir(PROJECT, 0o755)
    os.chown(PROJECT, WORK_UID, WORK_UID)
    with open(WRAPPER, "w") as file:
        file.write(LOADER)


def site(base_url):
    provider = {"fixture": {
        "npm": "@ai-sdk/openai-compatible", "name": "fixture",
        "options": {"baseURL": base_url, "apiKey": "fixture-not-a-credential"},
        "models": {"scripted": {"name": "scripted", "tool_call": True}},
    }}
    config = {
        "v": 1, "allowed_users": ["nes"], "run_base": RUNS, "max_deadline_s": 600,
        "cancel_grace_s": 10, "credential_margin_s": 60, "allow_keep": False,
        "default_path": "/usr/local/bin:/usr/bin:/bin",
        "routes": {
            "fixture": {"model": "fixture/scripted", "provider": provider, "credential": "none"},
            "fixture-auth": {"model": "fixture/scripted", "provider": provider, "credential": "required"},
        },
    }
    with open(SITE, "w") as file:
        json.dump(config, file)
    os.chmod(SITE, 0o644)


def leftovers():
    """What is left in the run base, and which processes are left in this
    fixture's own PID namespace besides this one."""
    runs = []
    for directory, dirs, files in os.walk(RUNS):
        runs += [os.path.relpath(os.path.join(directory, n), RUNS) for n in dirs + files]
    procs = sorted(int(p) for p in os.listdir("/proc") if p.isdigit() and int(p) != os.getpid())
    return {"runs": runs, "processes": procs}


def clean():
    """Only the requester's run-base directory is left, and no process."""
    return leftovers() == {"runs": ["1000"], "processes": []}


def call(out, name, prompt, *extra, timeout=180):
    prompt_file = os.path.join(ROOT, f"prompt-{name}.md")
    with open(prompt_file, "w") as file:
        file.write(prompt)
    target = os.path.join(out, name)
    argv = [sys.executable, "-I", PKG + "/bin/oulipoly-native-call", "--prompt-file", prompt_file, "--cwd", PROJECT,
            "--out", target, "--frontdoor", WRAPPER, "--direct-requester-uid", str(WORK_UID),
            "--direct-site-config", SITE, *extra]
    started = time.monotonic()
    done = subprocess.run(argv, timeout=timeout)
    with open(os.path.join(target, "result.json")) as file:
        result = json.load(file)
    events = []
    events_path = os.path.join(target, "events.jsonl")
    if os.path.exists(events_path):
        with open(events_path) as file:
            events = [json.loads(line) for line in file if line.strip()]
    answer = None
    if os.path.exists(os.path.join(target, "final.md")):
        with open(os.path.join(target, "final.md")) as file:
            answer = file.read()
    return {"exit": done.returncode, "elapsed_s": round(time.monotonic() - started, 1), "result": result,
            "events": events, "answer": answer, "out": target}


def direct(lines, timeout=120):
    """The front door run directly with these stdin lines, then stdin EOF."""
    data = b"".join(json.dumps(line).encode() + b"\n" for line in lines)
    done = subprocess.run([sys.executable, WRAPPER, "--site-config", SITE, "run"], input=data, capture_output=True,
                          env={"PATH": "/usr/bin:/bin", "SUDO_UID": str(WORK_UID)}, timeout=timeout)
    return done.returncode, [json.loads(line) for line in done.stdout.splitlines()]


def of(events, name):
    return [e for e in events if e.get("event") == name]


def frontdoor_terminal(events):
    terminals = [e for e in events if e.get("frontdoor") == "terminal"]
    return terminals[-1] if terminals else {}


def scenarios(out, model):
    checks = []

    def check(name, ok, **evidence):
        checks.append({"check": name, "ok": bool(ok), **evidence})

    def observe(name, **evidence):
        """Recorded, not asserted: native behaviour this fixture shows but
        this package does not own."""
        checks.append({"check": name, "ok": None, **evidence})

    # The fixed cleanup class, under real uid separation in this user
    # namespace. Inner root is host nes; this is not host-root evidence.
    loader = importlib.machinery.SourceFileLoader("cleanup_frontdoor", PKG + "/libexec/oulipoly-native-frontdoor")
    spec = importlib.util.spec_from_loader("cleanup_frontdoor", loader)
    cleanup = importlib.util.module_from_spec(spec)
    loader.exec_module(cleanup)
    cleanup.TRUSTED_OWNERS = frozenset({0, 65534})
    outside = ROOT + "/private/outside"
    os.mkdir(outside, 0o700)
    outside_auth = outside + "/auth.json"
    with open(outside_auth, "w") as file:
        file.write("fake-root-owned-outside-file")
    os.chmod(outside_auth, 0o600)
    user_dir = RUNS + "/1000"
    os.mkdir(user_dir, 0o711)
    os.chmod(user_dir, 0o711)
    for mode in ("keep", "discard", "sweep"):
        run = user_dir + "/cleanup-" + mode
        for sub in ("private", "launch/xdg/data/opencode", "launch/secret"):
            os.makedirs(run + "/" + sub, exist_ok=True)
        os.chmod(run, 0o711)
        with open(run + "/private/lock", "w"):
            pass
        with open(run + "/private/retention", "w") as file:
            file.write("discard" if mode == "sweep" else mode)
        for rel in cleanup.CREDENTIAL_FILES:
            with open(run + "/" + rel, "w") as file:
                file.write("fake-run-credential")
        data_dir = run + "/launch/xdg/data"
        os.chown(data_dir, WORK_UID, WORK_UID)
        os.chown(data_dir + "/opencode", WORK_UID, WORK_UID)
        os.chown(data_dir + "/opencode/auth.json", WORK_UID, WORK_UID)
        child = os.fork()
        if child == 0:
            try:
                os.setgroups([])
                os.setgid(WORK_UID)
                os.setuid(WORK_UID)
                shutil.rmtree(data_dir + "/opencode")
                os.symlink(outside, data_dir + "/opencode")
                os._exit(0)
            except BaseException:
                os._exit(1)
        _, status = os.waitpid(child, 0)
        if os.waitstatus_to_exitcode(status) != 0:
            raise RuntimeError("work-side symlink fixture failed")
        result = cleanup.sweep(user_dir)[0] if mode == "sweep" else cleanup.retire(run, mode)
        check("cleanup: " + mode + " preserves namespace-root outside file",
              os.path.exists(outside_auth) and os.stat(outside_auth).st_uid == 0 and not result["ok"],
              owner="namespace uid 0 (host nes)", result=result)
        # Remove only these just-created fixture objects, using fd-safe
        # rmtree; no privileged traversal through the replaced ancestor.
        shutil.rmtree(run)
    os.unlink(outside_auth)
    os.rmdir(outside)

    base = ["--route", "fixture"]

    # 1. Trusted-task Bash runs as the requester, in its cwd and environment.
    r = call(out, "trusted", "RUN id -un; id -u; pwd; echo mark=$E2E_MARK", *base, "--trusted-task",
             "--env", "E2E_MARK=m1", "--deadline", "120")
    accepted, ended = of(r["events"], "bash-accepted"), of(r["events"], "bash-ended")
    fd = frontdoor_terminal(r["events"])
    check("trusted: answered and closed", r["exit"] == 0 and r["result"]["class"] == "answered" and fd.get("exit") == 87,
          exit=r["exit"], cls=r["result"]["class"], frontdoor_exit=fd.get("exit"))
    check("trusted: answer is the work's own id, cwd and env",
          r["answer"] is not None and all(s in r["answer"] for s in ("nes", "1000", PROJECT, "mark=m1")), answer=r["answer"])
    check("trusted: one attributed Bash, ended code 0",
          len(accepted) == 1 and len(ended) == 1 and ended[0].get("status") == "code:0",
          accepted=[a.get("argv") for a in accepted], ended=[e.get("status") for e in ended])
    check("trusted: run retired", fd.get("retire", {}).get("run_removed") is True and fd["retire"]["credentials"]["ok"])
    check("trusted: nothing left", clean(), left=leftovers())
    setup = [e for e in r["events"] if e.get("entry") == "setup-completed"]
    check("trusted: policy and workload reported",
          bool(setup) and setup[0]["launch"]["policy"]["bash"] == "trusted-task" and setup[0]["workload"]["user"] == "nes"
          and setup[0]["workload"]["isolation"] == "host-root-pidns",
          policy=setup[0]["launch"]["policy"] if setup else None, workload=setup[0]["workload"] if setup else None)

    # 2. A finite allow-list: an unlisted command is a native denial.
    allow = os.path.join(ROOT, "allow.json")
    with open(allow, "w") as file:
        json.dump(["echo allowed"], file)
    r = call(out, "allow-list", "RUN id -un", *base, "--allow-file", allow, "--deadline", "120")
    check("allow-list: unlisted command never reaches the ingress",
          len(of(r["events"], "bash-accepted")) == 0 and r["result"]["class"] in ("answered", "no-answer"),
          cls=r["result"]["class"], accepted=len(of(r["events"], "bash-accepted")), answer=r["answer"])
    check("allow-list: nothing left", clean(), left=leftovers())

    # 3. The caller's deadline cancels a running, registered Bash.
    r = call(out, "deadline", "RUN sleep 300; echo late", *base, "--trusted-task", "--deadline", "15")
    fd = frontdoor_terminal(r["events"])
    accepted = of(r["events"], "bash-accepted")
    check("deadline: cancelled without replay, ended well before the sleep",
          r["result"]["class"] == "cancelled" and r["elapsed_s"] < 60 and list(r["result"]["sends"]) == ["cancel"]
          and fd.get("exit") in (82, 92),
          cls=r["result"]["class"], elapsed_s=r["elapsed_s"], frontdoor_exit=fd.get("exit"))
    check("deadline: the sleep was accepted before the cancel", len(accepted) == 1,
          accepted=[a.get("argv") for a in accepted])
    observe("deadline: did the owner end by itself after the caller's cancel (else the front door killed it)",
            owner_ended_itself=not fd.get("killed"), entry_status=fd.get("entry_status"),
            cancel_requested=of(r["events"], "cancel-requested"), bash_ended=[e.get("status") for e in of(r["events"], "bash-ended")],
            exited=[e.get("status") for e in of(r["events"], "exited")], owner_terminal=of(r["events"], "terminal")[-1:])
    check("deadline: nothing left", clean(), left=leftovers())

    # 4. Abandonment: the requester's stdin ends right after the request.
    code, lines = direct([{"v": 1, "route": "fixture", "message": "RUN sleep 300", "cwd": PROJECT,
                           "bash": {"authority": "trusted-task"}, "deadline_s": 120}])
    fd = frontdoor_terminal(lines)
    cancels = [l for l in lines if l.get("frontdoor") == "cancel"]
    check("abandonment: cancelled or killed, never left running",
          code in (82, 92) and bool(cancels) and "abandoned" in cancels[0]["why"],
          code=code, cancel=cancels[:1], killed=fd.get("killed"))
    check("abandonment: nothing left", clean(), left=leftovers())

    # 5. An inline access-only credential: staged root-only, removed at
    #    setup-completed, launch copies removed after; never in capture.
    source = os.path.join(ROOT, "private", "auth.json")
    with open(source, "w") as file:
        json.dump({"fixture": {"type": "oauth", "refresh": "e2e-fake-refresh-grant", "access": MARKER,
                               "expires": int(time.time() + 3600) * 1000}}, file)
    os.chmod(source, 0o600)
    r = call(out, "credential", "RUN echo authed", "--route", "fixture-auth", "--trusted-task", "--deadline", "120",
             "--credential-opencode-auth", source, "--credential-provider", "fixture", "--credential-margin", "60")
    fd = frontdoor_terminal(r["events"])
    staged = [e for e in r["events"] if e.get("frontdoor") == "staged-credential"]
    files = {}
    for record in fd.get("retire", {}).get("credentials", {}).get("files", []):
        for rel in ("private/auth.json", "launch/xdg/data/opencode/auth.json", "launch/secret/server-password"):
            if record["path"].endswith("/" + rel):
                files[rel] = record["result"]
    setup = [e for e in r["events"] if e.get("entry") == "setup-completed"]
    check("credential: staged copy removed at setup-completed", staged and staged[0]["result"] == "removed", staged=staged)
    check("credential: launch copies removed after the root ended",
          fd.get("retire", {}).get("credentials", {}).get("ok") is True
          and sorted(files.values()) == ["absent", "removed", "removed"], files=files)
    check("credential: setup placed it (auth enabled)", bool(setup) and setup[0]["launch"].get("auth") is not None,
          auth=setup[0]["launch"].get("auth") if setup else None)
    leaked = []
    for directory, dirs, names in os.walk(r["out"]):
        for name in names:
            with open(os.path.join(directory, name), "rb") as file:
                data = file.read()
            if MARKER.encode() in data or b"e2e-fake-refresh-grant" in data:
                leaked.append(name)
    check("credential: no token or refresh in the caller's capture", not leaked, leaked=leaked)
    check("credential: outcome recorded (native auth semantics for this provider not asserted)", True,
          cls=r["result"]["class"], answer=r["answer"], frontdoor_exit=fd.get("exit"),
          model_saw_authorization=sorted({str(q.get("authorization"))[:16] for q in model.requests}))
    check("credential: nothing left", clean(), left=leftovers())

    # 6. Refusals before any effect.
    good = {"v": 1, "route": "fixture", "message": "x", "cwd": PROJECT, "bash": {"authority": "trusted-task"}, "deadline_s": 60}
    for name, request in (
        ("loader env", dict(good, env={"LD_PRELOAD": "/tmp/x.so"})),
        ("unknown route", dict(good, route="nope")),
        ("raw native field", dict(good, store="/tmp/s")),
        ("cwd the requester cannot enter", dict(good, cwd=ROOT + "/private")),
        ("credential on a credential-free route", dict(good, credential={"fixture": {}})),
        ("refresh grant", dict(good, route="fixture-auth", credential={"fixture": {
            "type": "oauth", "refresh": "grant", "access": "a", "expires": int(time.time() + 3600) * 1000}})),
    ):
        code, lines = direct([request])
        fd = frontdoor_terminal(lines)
        check(f"refused: {name}", code == 90 and fd.get("effects") == "none" and leftovers()["runs"] in ([], ["1000"]),
              code=code, reason=fd.get("reason"))
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    if not in_user_namespace():
        print("refused: run only as root of an unprivileged user namespace (run_e2e.sh)", file=sys.stderr)
        return 2
    os.makedirs(args.out, exist_ok=False)
    prepare(os.path.realpath(args.stage))
    model = ScriptedModel()
    site(model.base_url)
    started = time.monotonic()
    try:
        checks = scenarios(args.out, model)
    finally:
        model.close()
    summary = {"checks": checks, "passed": all(c["ok"] is not False for c in checks), "elapsed_s": round(time.monotonic() - started, 1),
               "model_requests": len(model.requests)}
    with open(os.path.join(args.out, "summary.json"), "w") as file:
        json.dump(summary, file, indent=1, sort_keys=True)
    for c in checks:
        print({True: "PASS ", False: "FAIL ", None: "OBSERVED "}[c["ok"]] + c["check"])
    print("passed" if summary["passed"] else "FAILED", summary["elapsed_s"])
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
