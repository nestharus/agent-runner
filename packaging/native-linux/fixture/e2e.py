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
credential (staging, removal, no leak); refusals before any effect; a
native Claude route answered with attributed Bash, and Claude Code ending
mid-turn. For the Claude rows the crate's stand-in executable
(`tests/fixtures/fake-claude.mjs`, run by the packaged Node) is
bind-mounted over the packaged Claude Code executable inside this mount
namespace only: Claude Code itself is never run, and nothing here is
evidence about it, a login or a model.
"""

import argparse
import importlib.machinery
import importlib.util
import json
import os
import pwd
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
FAKE_CLAUDE = os.path.join(HERE, "../../../crates/oulipoly-root-supervisor/tests/fixtures/fake-claude.mjs")
CLAUDE_EXECUTABLE = PKG + "/claude/deps/node_modules/@anthropic-ai/claude-agent-sdk-linux-x64/claude"
CLAUDE_RECORDS = ROOT + "/claude-records"
CLAUDE_STORE = ".claude-e2e-fixture-store"

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
    # The stand-in over the packaged Claude Code executable, here only.
    with open(FAKE_CLAUDE) as file:
        source = file.read()
    fake = ROOT + "/fake-claude"
    with open(fake, "w") as file:
        file.write(f"#!{PKG}/claude/node/bin/node\n{source}")
    os.chmod(fake, 0o755)
    sh("mount", "--bind", fake, CLAUDE_EXECUTABLE)
    os.mkdir(CLAUDE_RECORDS, 0o755)
    os.chown(CLAUDE_RECORDS, WORK_UID, WORK_UID)
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
        "child_limits": {"max_starts": 4, "max_concurrent": 2},
        "child_routes": {"orientation": {"model": "fixture/scripted", "provider": provider, "credential": "required"}},
        "routes": {
            "fixture": {"model": "fixture/scripted", "provider": provider, "credential": "none"},
            "fixture-auth": {"model": "fixture/scripted", "provider": provider, "credential": "required", "children": ["orientation"]},
            "claude-fixture": {"harness": "claude", "model": "claude-opus-5-5", "effort": "medium",
                               "config_dir": CLAUDE_STORE, "credential": "none", "children": ["orientation"]},
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
        for ancestor in ("launch", "launch/xdg"):
            os.chmod(run + "/" + ancestor, 0o755)
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
        # Popen's uid/gid options avoid Python pre-exec/fork handlers in
        # the threaded scripted-model fixture. This helper is no model.
        changed = subprocess.run(
            [sys.executable, "-c", "import os,shutil,sys; shutil.rmtree(sys.argv[1]); os.symlink(sys.argv[2],sys.argv[1])",
             data_dir + "/opencode", outside],
            user=WORK_UID, group=WORK_UID, extra_groups=[],
            env={"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"}, cwd="/", timeout=5)
        if changed.returncode != 0:
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

    # 7. A native Claude route: answered, with attributed Bash, no
    #    credential, the requester's own store only named.
    store = os.path.join(pwd.getpwuid(WORK_UID).pw_dir, CLAUDE_STORE)
    record = CLAUDE_RECORDS + "/answered.jsonl"
    r = call(out, "claude", "RUN id -un; pwd", "--route", "claude-fixture", "--trusted-task", "--deadline", "120",
             "--env", "FAKE_CLAUDE_RECORD=" + record, "--env", "ANTHROPIC_API_KEY=e2e-not-a-key",
             "--env", "CLAUDE_CODE_OAUTH_TOKEN=e2e-not-a-token")
    fd = frontdoor_terminal(r["events"])
    accepted = of(r["events"], "bash-accepted")
    setup = [e for e in r["events"] if e.get("entry") == "setup-completed"]
    admitted = [e for e in r["events"] if e.get("frontdoor") == "admitted"]
    check("claude: answered and closed", r["exit"] == 0 and r["result"]["class"] == "answered" and fd.get("exit") == 87,
          exit=r["exit"], cls=r["result"]["class"], frontdoor_exit=fd.get("exit"))
    check("claude: answer is the attributed Bash result",
          r["answer"] is not None and r["answer"].startswith("DONE Root v1 work ended: exited with code 0"),
          answer=r["answer"])
    check("claude: one attributed Bash as the requester",
          len(accepted) == 1 and accepted[0].get("argv") == ["bash", "-lc", "id -un; pwd"],
          accepted=[a.get("argv") for a in accepted])
    check("claude: admitted without a credential", bool(admitted) and admitted[0].get("credential") is None
          and admitted[0].get("harness") == "claude", admitted=admitted[:1])
    launch = setup[0]["launch"] if setup else {}
    check("claude: stdio receiver launch names the requester's own store",
          bool(setup) and setup[0].get("harness") == "claude" and launch.get("endpoint") == "stdio"
          and launch.get("config_dir") == store and launch.get("policy", {}).get("bash") == "trusted-task",
          launch=launch)
    try:
        with open(record) as file:
            seen = [json.loads(line) for line in file if line.strip()]
    except OSError as error:
        seen = [{"error": type(error).__name__}]
    started = next((v for v in seen if "argv" in v), {})
    argv, env = started.get("argv", []), started.get("env", {})
    check("claude: Claude Code launch as constructed (stand-in's view)",
          env.get("CLAUDE_CONFIG_DIR") == store and "ANTHROPIC_API_KEY" not in env
          and "CLAUDE_CODE_OAUTH_TOKEN" not in env and "--setting-sources=" in argv and "--strict-mcp-config" in argv
          and argv[argv.index("--model") + 1] == "claude-opus-5-5" and argv[argv.index("--effort") + 1] == "medium"
          and argv[argv.index("--permission-mode") + 1] == "dontAsk",
          argv=argv, env=env)
    check("claude: the store was only named, never made", not os.path.exists(store))
    check("claude: run retired", fd.get("retire", {}).get("run_removed") is True)
    check("claude: nothing left", clean(), left=leftovers())

    # 8. Claude Code ends mid-turn: a visible no-answer, bounded, no cancel.
    r = call(out, "claude-exit", "hello", "--route", "claude-fixture", "--trusted-task", "--deadline", "120",
             "--env", "FAKE_CLAUDE_SCENARIO=crash")
    fd = frontdoor_terminal(r["events"])
    ends = [e for e in r["events"] if e.get("event") == "turn-end"]
    check("claude-exit: no-answer with a _claude_exited turn end, without cancel",
          r["result"]["class"] == "no-answer" and ends and ends[0].get("stop_reason") == "_claude_exited"
          and "cancel" not in r["result"]["sends"] and r["elapsed_s"] < 60,
          cls=r["result"]["class"], ends=ends, sends=r["result"]["sends"], elapsed_s=r["elapsed_s"],
          frontdoor_exit=fd.get("exit"))
    check("claude-exit: nothing left", clean(), left=leftovers())

    # Registered child use on BOTH native parent kinds. Published OpenCode
    # discovers the real explore.ts; Claude's executable is a fakepeer,
    # while its receiver and Agent SDK are the published package assets.
    for kind, parent, grant_options in (
        ("sol", "fixture-auth", ["--credential-opencode-auth", source, "--credential-provider", "fixture"]),
        ("claude", "claude-fixture", ["--child-credential-opencode-auth", source, "--child-credential-provider", "fixture"]),
    ):
        before = len(model.requests)
        sdk_record = CLAUDE_RECORDS + "/child-" + kind + ".jsonl"
        r = call(out, "child-" + kind, "EXPLORE where is child admission wired?",
                 "--route", parent, "--trusted-task", "--deadline", "120",
                 "--child-route", "orientation", "--child-max-starts", "4", "--child-max-concurrent", "2",
                 "--env", "FAKE_CLAUDE_RECORD=" + sdk_record, *grant_options)
        fd = frontdoor_terminal(r["events"])
        children = of(r["events"], "child-result")
        accepted = of(r["events"], "child-accepted")
        check(kind + " child: admitted once, answered and observed end",
              r["exit"] == 0 and len(accepted) == len(children) == 1
              and children[0].get("outcome") == "answered"
              and children[0].get("lifecycle", {}).get("end") == "observed"
              and children[0].get("end", {}).get("namespace", {}).get("drained") is True,
              cls=r["result"]["class"], accepted=accepted, results=children)
        check(kind + " child: parent-only answer carries orientation and lifecycle",
              r["answer"] is not None and "ORIENTATION:" in r["answer"]
              and "Lifecycle status: end observed" in r["answer"]
              and "root budget: still charged (release pending)" in r["answer"], answer=r["answer"])
        if kind == "sol":
            offered = [tool for q in model.requests[before:] for tool in (q.get("body", {}).get("tools") or [])
                       if tool.get("function", {}).get("name") == "explore"]
            check("sol child: real OpenCode discovered the explore custom tool",
                  bool(offered) and "orientation question" in offered[0]["function"].get("description", ""),
                  offered=offered)
        else:
            shutil.copyfile(sdk_record, os.path.join(r["out"], "sdk-peer.jsonl"))
            with open(sdk_record) as file:
                peer = [json.loads(line) for line in file if line.strip()]
            listed = next((v["tools_list"] for v in peer if "tools_list" in v), {})
            check("claude child: published SDK MCP list and callback result",
                  any(tool.get("name") == "explore" for tool in listed.get("mcp_response", {}).get("result", {}).get("tools", []))
                  and any("Lifecycle status: end observed" in str(v.get("tool_result", "")) for v in peer),
                  tools_list=listed)
        with open(os.path.join(r["out"], "children.json")) as file:
            inventory = json.load(file)
        check(kind + " child: site and owner ceilings are 4/2",
              bool(accepted) and accepted[0].get("starts", {}).get("max") == 4
              and accepted[0].get("concurrent", {}).get("max") == 2,
              owner_summary=inventory["owner_summary"])
        removed = fd.get("retire", {}).get("credentials", {}).get("files", [])
        check(kind + " child: staged grant and derived copies normally retired",
              fd.get("retire", {}).get("run_removed") is True
              and any(x["path"].endswith("/private/child-auth.json") and x["result"] == "removed" for x in removed)
              and any("/launch/children/c" in x["path"] and x["path"].endswith("/auth.json")
                      and x["result"] == "removed" for x in removed), credential_cleanup=removed)
        check(kind + " child: nothing left", clean(), left=leftovers())

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
    with open(os.path.join(args.out, "scripted-model-requests.json"), "w") as file:
        json.dump(model.requests, file, indent=1, sort_keys=True)
    with open(os.path.join(args.out, "summary.json"), "w") as file:
        json.dump(summary, file, indent=1, sort_keys=True)
    for c in checks:
        print({True: "PASS ", False: "FAIL ", None: "OBSERVED "}[c["ok"]] + c["check"])
    print("passed" if summary["passed"] else "FAILED", summary["elapsed_s"])
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
