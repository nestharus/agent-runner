#!/usr/bin/env python3
"""Proof driver. Run INSIDE a user+net namespace (only loopback), e.g.

  unshare --user --map-current-user --keep-caps --net sh -c \
    'ip link set lo up && exec setpriv --inh-caps=-all --bounding-set=-all \
     python3 run_proof.py <build_dir> <mode> <client_bin>'

Modes:
  serve           ACP session/new + submit against `opencode serve`.
  serve-existing  seed a native conversation, restart, ACP session/resume + submit (no PTY).
  tui-existing    seed a native conversation, start the installed TUI on it
                  (`--session <id>`), ACP session/resume + submit, observe the PTY.

Fresh owned state only: HOME/XDG/project live under <build_dir>/run-*/,
env is rebuilt from scratch (no inherited API keys), no credentials exist.
Every spawned process is its own session and is killed and reaped here.
The PTY is judged on the screen rebuilt by a terminal emulator (pyte from
<build_dir>/pydeps), not on raw byte grep.
"""

import json
import os
import pty
import re
import select
import shutil
import signal
import subprocess
import sys
import time
import traceback
import urllib.request

OPENCODE = "/home/nes/.opencode/bin/opencode"
HERE = os.path.dirname(os.path.abspath(__file__))
PORT = 4797
ROWS, COLS = 40, 140
NO_MODEL = {"providerID": "proof-none", "modelID": "none"}


def log(*parts):
    print("[driver]", *parts, flush=True)


def wait_for(pred, seconds, what, pump=None):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if pump:
            pump(0.2)
        if pred():
            return True
        if not pump:
            time.sleep(0.2)
    log(f"TIMEOUT waiting for {what}")
    return False


def http(method, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}{path}", method=method, data=data,
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as res:
        raw = res.read()
        return res.status, (json.loads(raw) if raw else None)


def server_up(project, last_err):
    try:
        http("GET", f"/session?directory={project}")
        return True
    except Exception as e:
        last_err[0] = repr(e)
        return False


def messages(sid, project):
    status, msgs = http("GET", f"/session/{sid}/message?directory={project}")
    rows = []
    for item in msgs:
        texts = [p.get("text") for p in item["parts"] if p.get("type") == "text"]
        rows.append((item["info"]["id"], item["info"]["role"], texts))
    return status, rows


class Host:
    """One owned opencode process group (serve, or TUI on a PTY)."""

    def __init__(self, argv, project, env, run, name, tty):
        self.out = bytearray()
        self.master = None
        self.name = name
        if tty:
            self.master, slave = pty.openpty()
            os.set_blocking(self.master, False)
            import fcntl, struct, termios
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
            self.proc = subprocess.Popen(argv, cwd=project, env=env, stdin=slave, stdout=slave, stderr=slave,
                                         start_new_session=True)
            os.close(slave)
        else:
            self.logf = open(f"{run}/{name}.log", "wb")
            self.proc = subprocess.Popen(argv, cwd=project, env=env, stdout=self.logf, stderr=subprocess.STDOUT,
                                         start_new_session=True)
        log(name, "argv", argv, "pid", self.proc.pid, "pgid", os.getpgid(self.proc.pid))

    def pump(self, t):
        if self.master is None:
            time.sleep(t)
            return
        r, _, _ = select.select([self.master], [], [], t)
        if r:
            try:
                self.out.extend(os.read(self.master, 65536))
            except OSError:
                pass

    def screen(self):
        import pyte
        screen = pyte.Screen(COLS, ROWS)
        stream = pyte.ByteStream(screen)
        stream.feed(bytes(self.out))
        return [line.rstrip() for line in screen.display]

    def stop(self):
        try:
            os.killpg(self.proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(self.proc.pid, signal.SIGKILL)
            self.proc.wait(timeout=10)
        try:
            os.killpg(self.proc.pid, signal.SIGKILL)  # stragglers in the owned group
            log(self.name, "killed stragglers in owned pgid", self.proc.pid)
        except ProcessLookupError:
            log(self.name, "owned pgid", self.proc.pid, "empty after host exit")
        log(self.name, "exit", self.proc.returncode)
        if self.master is not None:
            os.close(self.master)
            self.master = None


def find_row(lines, needle):
    for i, line in enumerate(lines):
        if needle in line:
            return i
    return -1


def save_screen(run, tag, host):
    lines = host.screen()
    with open(f"{run}/pty-{tag}.raw", "wb") as f:
        f.write(host.out)
    with open(f"{run}/screen-{tag}.txt", "w") as f:
        f.write("\n".join(lines) + "\n")
    log(f"rendered screen [{tag}] ({len(host.out)} raw bytes):")
    for i, line in enumerate(lines):
        print(f"  {i:02d}|{line}", flush=True)
    return lines


def seed_conversation(env, project, run, history):
    """A previous, separate opencode process creates the conversation the
    person already has. Native routes only; noReply so no model turn."""
    seed_env = {k: v for k, v in env.items() if k != "OULIPOLY_ACP_V2_SOCKET"}
    host = Host([OPENCODE, "serve", "--port", str(PORT), "--hostname", "127.0.0.1"], project, seed_env, run,
                "seed-serve", tty=False)
    try:
        last_err = [None]
        if not wait_for(lambda: server_up(project, last_err), 150, "seed server"):
            log("last http error", last_err[0])
            return None
        status, created = http("POST", f"/session?directory={project}", {})
        sid = created["id"]
        log("seed native POST /session", status, sid, "title", json.dumps(created.get("title")))
        for text in history:
            status, _ = http("POST", f"/session/{sid}/prompt_async?directory={project}",
                             {"parts": [{"type": "text", "text": text}], "noReply": True, "model": NO_MODEL})
            log("seed native POST prompt_async", status, text)
            if not wait_for(lambda: [t for _, _, t in messages(sid, project)[1]][-1:] == [[text]], 20,
                            f"seed message {text} persisted"):
                return None
        log("seed native readback:", messages(sid, project)[1])
        return sid
    finally:
        host.stop()


def run_client(client_bin, sock, project, token, sid, host, run):
    """True only on: client exit 0, ACK, ACK id == native user message with
    exactly the token, history kept before it, and (TUI) token rendered."""
    argv = [client_bin, sock, project, token] + ([sid] if sid else [])
    client = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    log("client argv", argv, "pid", client.pid)
    print(client.stdout.readline(), end="", flush=True)  # initialize
    line = client.stdout.readline()
    print(line, end="", flush=True)  # session/new or session/resume
    m = re.match(r"session/(new|resume): (ses_\S+)", line)
    if not m or (sid and m.group(2) != sid):
        rest, err = client.communicate(input="", timeout=20)
        print(rest, err, flush=True)
        log("client exit", client.returncode, "(no usable session)")
        return False
    sid = m.group(2)
    try:
        rest, err = client.communicate(input="go\n", timeout=60)
    except subprocess.TimeoutExpired:
        client.kill()
        rest, err = client.communicate()
        log("client TIMEOUT, killed")
    print(rest, end="", flush=True)
    if err:
        print("client stderr:", err, flush=True)
    log("client exit", client.returncode)
    ack = re.search(r'Accepted\(Acceptance \{ message_id: "([^"]+)"', rest)
    status, rows = messages(sid, project)
    log("native readback GET /session/{id}/message", status)
    for row in rows:
        log("  native message", *row)
    exact = bool(ack) and any(i == ack.group(1) and r == "user" and t == [token] for i, r, t in rows)
    log("native assistant messages:", [r for _, r, _ in rows].count("assistant"))
    log("ACK present:", bool(ack), "| ACK id == native user message with exact text:", exact)
    ok = client.returncode == 0 and exact
    if host.master is not None:
        seen = wait_for(lambda: find_row(host.screen(), token) >= 0, 20, "token on rendered TUI screen", host.pump)
        wait_for(lambda: False, 2, "post-token settle (fixed 2s pump)", host.pump)
        lines = save_screen(run, "post-submit", host)
        log("token on rendered screen:", seen, "| at row", find_row(lines, token))
        ok = ok and seen and find_row(lines, token) >= 0
    return ok, rows


def main():
    build, mode, client_bin = sys.argv[1], sys.argv[2], sys.argv[3]
    assert mode in ("serve", "serve-existing", "tui-existing"), mode
    sys.path.insert(0, os.path.join(build, "pydeps"))
    run = os.path.join(build, f"run-{mode}-{int(time.time())}")
    home, project, sock = f"{run}/home", f"{run}/project", f"{run}/acp-v2.sock"
    for d in (home, project, f"{run}/xdg/config", f"{run}/xdg/data", f"{run}/xdg/cache", f"{run}/xdg/state"):
        os.makedirs(d, mode=0o700)
    plugin = os.path.join(build, "deps", "acp-v2-endpoint.ts")  # beside deps/node_modules
    shutil.copyfile(os.path.join(HERE, "acp-v2-endpoint.ts"), plugin)
    with open(f"{project}/README.txt", "w") as f:
        f.write("fresh proof project\n")
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": home,
        "XDG_CONFIG_HOME": f"{run}/xdg/config",
        "XDG_DATA_HOME": f"{run}/xdg/data",
        "XDG_CACHE_HOME": f"{run}/xdg/cache",
        "XDG_STATE_HOME": f"{run}/xdg/state",
        "TERM": "xterm-256color",
        "OPENCODE_CONFIG_CONTENT": json.dumps({"plugin": [f"file://{plugin}"], "autoupdate": False, "share": "disabled"}),
        "OPENCODE_DISABLE_AUTOUPDATE": "1",
        "OPENCODE_DISABLE_DEFAULT_PLUGINS": "1",
        "OPENCODE_DISABLE_PROJECT_CONFIG": "1",
        "OPENCODE_DISABLE_CLAUDE_CODE": "1",
        "OPENCODE_DISABLE_LSP_DOWNLOAD": "1",
        "OPENCODE_DISABLE_SHARE": "1",
        "OULIPOLY_ACP_V2_SOCKET": sock,
        "OULIPOLY_ACP_V2_LOG": f"{run}/acp-v2-endpoint.log",
        "OULIPOLY_ACP_V2_PROOF_NO_REPLY": "1",
        "OPENCODE_LOG_LEVEL": "DEBUG",
    }
    stamp = int(time.time())
    token = f"OULIPOLY_INSERT_{stamp}"
    history = [f"PRIOR_HISTORY_ONE_{stamp}", f"PRIOR_HISTORY_TWO_{stamp}"]
    log("mode", mode, "run dir", run, "uid", os.getuid(), "pid", os.getpid())
    log("net devices (/proc/net/dev)", [l.split(":")[0].strip() for l in open("/proc/net/dev").read().splitlines()[2:]])
    log("token", token, "history", history)

    def deadline(signum, frame):
        raise TimeoutError("driver overall deadline")

    signal.signal(signal.SIGALRM, deadline)
    signal.alarm(420)
    rc = 1
    host = None
    try:
        sid = None
        if mode != "serve":
            sid = seed_conversation(env, project, run, history)
            if not sid:
                log("PROOF RESULT FAIL (seed)")
                return
        if mode == "tui-existing":
            argv = [OPENCODE, "--port", str(PORT), "--hostname", "127.0.0.1", "--session", sid, project]
            host = Host(argv, project, env, run, "tui", tty=True)
        else:
            argv = [OPENCODE, "serve", "--port", str(PORT), "--hostname", "127.0.0.1"]
            host = Host(argv, project, env, run, "serve", tty=False)
        last_err = [None]
        if not wait_for(lambda: server_up(project, last_err), 150, "native server", host.pump):
            log("last http error", last_err[0])
            return
        log("native server answered GET /session")
        if not wait_for(lambda: os.path.exists(sock), 60, "plugin socket", host.pump):
            return
        log("plugin socket exists", sock)
        if host.master is not None:
            shown = wait_for(lambda: find_row(host.screen(), history[-1]) >= 0, 40,
                             "prior history on rendered TUI screen", host.pump)
            wait_for(lambda: False, 2, "pre-attach settle (fixed 2s pump)", host.pump)
            lines = save_screen(run, "pre-attach", host)
            log("prior history on rendered screen before attach:", shown,
                "| rows", [find_row(lines, h) for h in history],
                "| token on screen before attach (countercase, expect False):", find_row(lines, token) >= 0)
            if not shown:
                log("PROOF RESULT FAIL (TUI did not show the existing conversation)")
                return
        ok, rows = run_client(client_bin, sock, project, token, sid, host, run)
        if sid:
            texts = [t for _, _, t in rows]
            kept = texts[:len(history)] == [[h] for h in history] and texts[len(history):len(history) + 1] == [[token]]
            log("history kept and new message appended after it in the same native session:", kept)
            ok = ok and kept
        rc = 0 if ok else 1
        log("PROOF RESULT", "PASS" if ok else "FAIL")
    except BaseException:
        log("DRIVER EXCEPTION (not a proof result)")
        traceback.print_exc(file=sys.stdout)
        sys.stdout.flush()
    finally:
        if host is not None:
            host.stop()
        for path in (f"{run}/acp-v2-endpoint.log", f"{run}/seed-serve.log", f"{run}/serve.log"):
            if os.path.exists(path):
                print("====", path, flush=True)
                print(open(path, errors="replace").read()[-4000:], flush=True)
        for root, _, files in os.walk(f"{run}/xdg/data"):
            for name in sorted(files):
                if name.endswith(".log"):
                    print("==== opencode log", os.path.join(root, name), flush=True)
                    print(open(os.path.join(root, name), errors="replace").read()[-5000:], flush=True)
        signal.alarm(0)
        sys.exit(rc)


if __name__ == "__main__":
    main()
