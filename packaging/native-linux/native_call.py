#!/usr/bin/python3 -I
"""Run one Linux native ACP v2 task through its installed front door.

    oulipoly-native-call --route NAME --prompt-file FILE --cwd DIR --out DIR
        (--trusted-task | --allow-file FILE) [--deadline SECONDS]
        [--env NAME=VALUE ...] [--retention discard|keep]
        [--credential-opencode-auth FILE --credential-provider ID
         | --credential-codex-profile DIR]
        [--child-route NAME ... [--child-max-starts N] [--child-max-concurrent N]
         [--child-credential-codex-profile DIR
          | --child-credential-opencode-auth FILE --child-credential-provider ID]]

Explicit caller-selected access-only source; no search/refresh/copyback.
Nonblocking admission/control and bounded terminal/EOF/exit collection;
unknown stop is incomplete, without pretending a privileged child ended.
Output is captured without a redactor and can contain arbitrary secrets.
Answered/0 means linked text plus complete transport, not task correctness.
The answer and the automatic close are the parent's only: events marked
as a registered child's (`child`) never supply either. Child results and
lifecycles are kept separately (children.json); they are what the owner
reported, not proof any child or its Bash drained beyond what they say.
--child-credential-* is a Claude parent's separate access-only grant for
the child provider; never Claude's own login.
Exit classes and runtime/credential bounds: share/README.md.
"""

import argparse
import base64
import json
import os
import selectors
import signal
import stat
import subprocess
import sys
import time

EOF_GRACE_S = 30
STOP_GRACE_S = 65
LINE_LIMIT = 8 * 1024 * 1024
CAPTURE_LIMIT = 64 * 1024 * 1024
MAX_EXPIRY_MS = 253402300799000
# The front door's default site bound; a site may admit less (it refuses).
MAX_DEADLINE_S = 7200
FRONTDOOR = "libexec/oulipoly-native-frontdoor"


class LocalRefusal(Exception):
    """Refused before anything was started."""


def read_private(path):
    """The caller's own regular file, read without following a final
    symlink or blocking on a FIFO."""
    st = os.lstat(path)
    if not stat.S_ISREG(st.st_mode):
        raise LocalRefusal("credential source is not a regular file")
    if st.st_uid != os.getuid():
        raise LocalRefusal("credential source is not the caller's own file")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    try:
        opened = os.fstat(fd)
        if not stat.S_ISREG(opened.st_mode) or opened.st_uid != os.getuid():
            raise LocalRefusal("credential source changed or is not the caller's own regular file")
        chunks = []
        while True:
            chunk = os.read(fd, 65536)
            if not chunk:
                break
            chunks.append(chunk)
            if sum(map(len, chunks)) > 1024 * 1024:
                raise LocalRefusal("credential source is too large")
        return b"".join(chunks)
    finally:
        os.close(fd)


def access_only(provider, access, expires_ms, account):
    entry = {"type": "oauth", "refresh": "", "access": access, "expires": expires_ms}
    if account:
        entry["accountId"] = account
    return {provider: entry}


def from_opencode_auth(path, provider):
    try:
        entries = json.loads(read_private(path))
        entry = entries[provider]
        if entry.get("type") != "oauth":
            raise LocalRefusal(f"credential {provider} entry is not oauth")
        access, expires = entry["access"], entry["expires"]
        account = entry.get("accountId")
        if not isinstance(access, str) or not access or type(expires) is not int:
            raise ValueError
    except LocalRefusal:
        raise
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        # No message: it could quote the source.
        raise LocalRefusal(f"credential source schema ({type(error).__name__})") from None
    return access_only(provider, access, expires, account if isinstance(account, str) else None)


def from_codex_profile(directory):
    try:
        tokens = json.loads(read_private(os.path.join(directory, "auth.json")))["tokens"]
        access = tokens["access_token"]
        if not isinstance(access, str) or access.count(".") != 2:
            raise ValueError
        payload = access.split(".")[1]
        claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
        exp = claims["exp"]
        if type(exp) is not int or exp <= 0:
            raise ValueError
        account = tokens.get("account_id")
        if not isinstance(account, str) or not account:
            account = claims.get("https://api.openai.com/auth", {}).get("chatgpt_account_id")
        if not isinstance(account, str) or not account:
            raise ValueError
    except LocalRefusal:
        raise
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        raise LocalRefusal(f"credential source schema ({type(error).__name__})") from None
    return access_only("openai", access, exp * 1000, account)


def credential_public(credential, now):
    if credential is None:
        return None
    (provider, entry), = credential.items()
    if type(entry["expires"]) is not int or not 0 < entry["expires"] <= MAX_EXPIRY_MS:
        raise LocalRefusal("credential expiry is out of range")
    return {
        "provider": provider,
        "expires_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(entry["expires"] // 1000)),
        "remaining_s": entry["expires"] // 1000 - int(now),
        "refresh": "not-sent",
    }


def check_fresh(credential, deadline, margin, now):
    (_, entry), = credential.items()
    remaining = entry["expires"] // 1000 - int(now)
    if remaining < deadline + margin:
        raise LocalRefusal(f"credential expires in {remaining}s; the deadline needs {deadline + margin}s")


def parse_args(argv):
    parser = argparse.ArgumentParser(prog="oulipoly-native-call", description=__doc__.split("\n\n")[0])
    parser.add_argument("--route", required=True)
    parser.add_argument("--prompt-file", required=True)
    parser.add_argument("--cwd", required=True)
    parser.add_argument("--out", required=True)
    policy = parser.add_mutually_exclusive_group(required=True)
    policy.add_argument("--trusted-task", action="store_true")
    policy.add_argument("--allow-file")
    parser.add_argument("--deadline", type=int, default=1800)
    parser.add_argument("--env", action="append", default=[])
    parser.add_argument("--retention", choices=("discard", "keep"), default="discard")
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--credential-opencode-auth")
    source.add_argument("--credential-codex-profile")
    parser.add_argument("--credential-provider")
    parser.add_argument("--credential-margin", type=int, default=600)
    parser.add_argument("--child-route", action="append", default=[])
    parser.add_argument("--child-max-starts", type=int)
    parser.add_argument("--child-max-concurrent", type=int)
    child = parser.add_mutually_exclusive_group()
    child.add_argument("--child-credential-opencode-auth")
    child.add_argument("--child-credential-codex-profile")
    parser.add_argument("--child-credential-provider")
    parser.add_argument("--frontdoor")
    parser.add_argument("--sudo", default="/usr/bin/sudo")
    # Test seam only: run the front door directly (the caller is root in a
    # test user namespace) and name the requester it would get from sudo.
    parser.add_argument("--direct-requester-uid", type=int, help=argparse.SUPPRESS)
    parser.add_argument("--direct-site-config", help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.credential_opencode_auth and not args.credential_provider:
        parser.error("--credential-opencode-auth needs --credential-provider")
    if args.credential_provider and not args.credential_opencode_auth:
        parser.error("--credential-provider goes with --credential-opencode-auth")
    if args.credential_margin < 0:
        parser.error("--credential-margin must be nonnegative")
    if not 1 <= args.deadline <= MAX_DEADLINE_S:
        parser.error(f"--deadline must be 1..{MAX_DEADLINE_S}")
    if args.child_credential_opencode_auth and not args.child_credential_provider:
        parser.error("--child-credential-opencode-auth needs --child-credential-provider")
    if args.child_credential_provider and not args.child_credential_opencode_auth:
        parser.error("--child-credential-provider goes with --child-credential-opencode-auth")
    child_options = (args.child_max_starts, args.child_max_concurrent,
                     args.child_credential_opencode_auth, args.child_credential_codex_profile)
    if not args.child_route and any(value is not None for value in child_options):
        parser.error("--child-* options need --child-route")
    for name in ("child_max_starts", "child_max_concurrent"):
        value = getattr(args, name)
        if value is not None and not 1 <= value <= 4:
            parser.error(f"--{name.replace('_', '-')} out of range")
    return args


def build_request(args, now):
    """(request, public request); LocalRefusal before anything starts."""
    try:
        with open(args.prompt_file, encoding="utf-8") as file:
            message = file.read()
    except (OSError, UnicodeError) as error:
        raise LocalRefusal(f"prompt file: {type(error).__name__}") from None
    if not message.strip():
        raise LocalRefusal("prompt file is empty")
    if args.trusted_task:
        bash = {"authority": "trusted-task"}
    else:
        try:
            with open(args.allow_file, encoding="utf-8") as file:
                allow = json.load(file)
        except (OSError, ValueError) as error:
            raise LocalRefusal(f"allow file: {type(error).__name__}") from None
        if not isinstance(allow, list) or not all(isinstance(command, str) for command in allow):
            raise LocalRefusal("allow file must be a JSON list of whole commands")
        bash = {"allow": allow}
    env = {}
    for item in args.env:
        name, sep, value = item.partition("=")
        if not sep or not name:
            raise LocalRefusal("--env is not NAME=VALUE")
        env[name] = value
    credential = None
    if args.credential_opencode_auth:
        credential = from_opencode_auth(args.credential_opencode_auth, args.credential_provider)
    elif args.credential_codex_profile:
        credential = from_codex_profile(args.credential_codex_profile)
    if credential is not None:
        check_fresh(credential, args.deadline, args.credential_margin, now)
    child_credential = None
    if args.child_credential_opencode_auth:
        child_credential = from_opencode_auth(args.child_credential_opencode_auth, args.child_credential_provider)
    elif args.child_credential_codex_profile:
        child_credential = from_codex_profile(args.child_credential_codex_profile)
    if child_credential is not None:
        check_fresh(child_credential, args.deadline, args.credential_margin, now)
    request = {
        "v": 1,
        "route": args.route,
        "message": message,
        "cwd": os.path.abspath(args.cwd),
        "bash": bash,
        "env": env,
        "deadline_s": args.deadline,
        "retention": args.retention,
    }
    try:
        json.dumps(request, ensure_ascii=False).encode("utf-8")
    except UnicodeError:
        raise LocalRefusal("request: invalid Unicode") from None
    if args.child_route:
        children = {"routes": list(args.child_route)}
        if args.child_max_starts is not None:
            children["max_starts"] = args.child_max_starts
        if args.child_max_concurrent is not None:
            children["max_concurrent"] = args.child_max_concurrent
        request["children"] = children
    public = dict(request, credential=credential_public(credential, now))
    if args.child_route:
        public["child_credential"] = credential_public(child_credential, now)
    if credential is not None:
        request["credential"] = credential
    if child_credential is not None:
        request["child_credential"] = child_credential
    return request, public


def parent_event(event):
    """Not a registered child's event (the owner marks those `child`)."""
    return "child" not in event


def answer_of(events):
    """The last agent message linked to input 0 before input 0's first
    turn end, and that turn end."""
    events = [e for e in events if parent_event(e)]
    acks = {e.get("message_id") for e in events if e.get("event") == "ack" and e.get("index") == 0 and e.get("message_id")}
    linked = []
    for event in events:
        if event.get("event") == "agent-message" and event.get("input") == 0 and event.get("parent_message_id") in acks:
            linked.append(event)
        if event.get("event") == "turn-end" and event.get("input") == 0:
            texts = [m for m in linked if str(m.get("text", "")).strip()]
            return (texts[-1]["text"] if texts else None), event, len(linked)
    return None, None, len(linked)


class Call:
    def __init__(self, args, out):
        self.args = args
        self.out = out
        self.start = time.monotonic()
        self.events = []
        self.sends = {}
        self.signals = []
        self.pending = b""
        self.stop_at = None
        self.actions = open(os.path.join(out, "caller.jsonl"), "w", encoding="utf-8")
        self.capture = open(os.path.join(out, "events.jsonl"), "wb")

    def log(self, **fields):
        self.actions.write(json.dumps({**fields, "t": round(time.monotonic() - self.start, 3)}, sort_keys=True) + "\n")
        self.actions.flush()

    def send(self, proc, cmd, why):
        if cmd in self.sends or "cancel" in self.sends or proc.stdin.closed:
            return
        self.sends[cmd] = {"why": why, "t": round(time.monotonic() - self.start, 3)}
        self.stop_at = min(self.stop_at or float("inf"), time.monotonic() + STOP_GRACE_S)
        if self.pending:
            # Admission is still blocked; close stdin rather than completing
            # a cancelled request. The privileged side bounds admission too.
            self.pending = b""
            proc.stdin.close()
            self.sends[cmd]["sent"] = False
        else:
            self.pending = json.dumps({"cmd": cmd}).encode() + b"\n"
            self.sends[cmd]["sent"] = "queued"
        self.log(action="send", cmd=cmd, why=why, sent=self.sends[cmd]["sent"])

    def line(self, proc, raw):
        try:
            value = json.loads(raw)
            if not isinstance(value, dict):
                raise ValueError
        except ValueError:
            self.events.append({"unparsed": True})
            return
        self.events.append(value)
        if value.get("event") == "turn-end" and value.get("input") == 0 and parent_event(value):
            self.send(proc, "close", "turn end of the message (readiness, not processing success)")

    def argv(self):
        frontdoor = self.args.frontdoor or os.path.join(
            os.path.dirname(os.path.dirname(os.path.realpath(__file__))), FRONTDOOR
        )
        frontdoor = os.path.realpath(frontdoor)
        if self.args.direct_requester_uid is not None:
            argv = [sys.executable, frontdoor]
            if self.args.direct_site_config:
                argv += ["--site-config", self.args.direct_site_config]
            env = {"PATH": "/usr/bin:/bin", "SUDO_UID": str(self.args.direct_requester_uid)}
            return argv + ["run"], env
        return [self.args.sudo, "-n", frontdoor, "run"], {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"}

    def run(self, request):
        argv, env = self.argv()
        payload = json.dumps(request).encode() + b"\n"
        stderr = open(os.path.join(self.out, "stderr.log"), "wb")
        try:
            proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, env=env, cwd="/", bufsize=0)
        except OSError as error:
            self.log(action="launch-failed", error=type(error).__name__)
            stderr.close()
            return None, {"stdout_eof": False, "exit": None, "errors": ["launch-failed"]}
        errors = []
        buffers = {"stdout": b"", "stderr": b""}
        eof = {"stdout": False, "stderr": False}
        total = 0
        code = None
        self.pending = payload
        deadline = self.start + self.args.deadline
        total_end = deadline + STOP_GRACE_S
        old = {sig: signal.signal(sig, lambda sig, frame: self.signals.append(sig)) for sig in (signal.SIGINT, signal.SIGTERM)}
        selector = selectors.DefaultSelector()
        for pipe, name in ((proc.stdout, "stdout"), (proc.stderr, "stderr")):
            os.set_blocking(pipe.fileno(), False)
            selector.register(pipe, selectors.EVENT_READ, name)
        os.set_blocking(proc.stdin.fileno(), False)
        input_registered = False
        try:
            while True:
                now = time.monotonic()
                if self.signals:
                    self.send(proc, "cancel", "caller signal")
                if now >= deadline:
                    self.send(proc, "cancel", "deadline")
                if now >= min(total_end, self.stop_at or float("inf")):
                    errors.append("collection-bound-reached; stop-unknown")
                    break
                if all(eof.values()) and proc.poll() is not None:
                    code = proc.poll()
                    break
                if self.pending and not proc.stdin.closed and not input_registered:
                    selector.register(proc.stdin, selectors.EVENT_WRITE, "stdin")
                    input_registered = True
                elif (not self.pending or proc.stdin.closed) and input_registered:
                    # unregister by saved fd if the pipe was closed by send.
                    selector.unregister(input_fd)
                    input_registered = False
                if input_registered:
                    input_fd = proc.stdin.fileno()
                for key, _ in selector.select(timeout=0.02):
                    if key.data == "stdin":
                        if proc.stdin.closed:
                            continue
                        try:
                            n = os.write(key.fd, self.pending)
                            self.pending = self.pending[n:]
                        except BlockingIOError:
                            pass
                        except OSError:
                            self.pending = b""
                            proc.stdin.close()
                            errors.append("request-or-control-undelivered")
                            self.stop_at = min(self.stop_at or float("inf"), time.monotonic() + EOF_GRACE_S)
                        continue
                    name = key.data
                    chunk = os.read(key.fd, 65536)
                    if not chunk:
                        eof[name] = True
                        selector.unregister(key.fileobj)
                        self.stop_at = min(self.stop_at or float("inf"), time.monotonic() + EOF_GRACE_S)
                        continue
                    total += len(chunk)
                    if total > CAPTURE_LIMIT:
                        errors.append("capture-limit; stop-unknown")
                        break
                    if name == "stderr":
                        stderr.write(chunk)
                        continue
                    self.capture.write(chunk)
                    buffers[name] += chunk
                    if len(buffers[name]) > LINE_LIMIT:
                        errors.append("stdout-line-limit; stop-unknown")
                        break
                    while b"\n" in buffers[name]:
                        raw, buffers[name] = buffers[name].split(b"\n", 1)
                        self.line(proc, raw)
                        if self.events[-1].get("frontdoor") == "terminal":
                            self.pending = b""
                            if not proc.stdin.closed:
                                proc.stdin.close()
                            self.stop_at = min(self.stop_at or float("inf"), time.monotonic() + EOF_GRACE_S)
                if errors and any("stop-unknown" in e for e in errors):
                    break
            if buffers["stdout"]:
                errors.append("unterminated-stdout-line")
            code = proc.poll()
        except (OSError, ValueError, TypeError) as error:
            errors.append("capture-failed:" + type(error).__name__ + "; stop-unknown")
        finally:
            selector.close()
            for sig, handler in old.items():
                signal.signal(sig, handler)
            # Closing requests abandonment, but does not prove the privileged
            # child or its namespace was reaped. No wait without a bound.
            for pipe in (proc.stdin, proc.stdout, proc.stderr):
                try:
                    pipe.close()
                except OSError:
                    pass
            stderr.close()
            if proc.poll() is None:
                try:
                    proc.terminate()
                except OSError:
                    pass
                try:
                    proc.wait(timeout=0.2)
                except subprocess.TimeoutExpired:
                    pass
            code = proc.poll()
        self.log(action="ended", exit=code, stdout_eof=eof["stdout"])
        return code, {"stdout_eof": eof["stdout"], "stderr_eof": eof["stderr"],
                      "exit": code, "errors": errors, "stop": "unknown" if errors else "observed"}


def classify(code, collection, events, sends):
    terminals = [e for e in events if e.get("frontdoor") == "terminal"]
    terminal = terminals[-1] if terminals else None
    answer, turn_end, linked = answer_of(events)
    complete = bool(terminal) and collection["stdout_eof"] and not collection["errors"] and code is not None
    entries = [e for e in events if e.get("entry") == "terminal"]
    relay_complete = bool(entries) and entries[-1].get("relay") == "complete"
    if code is None and collection["errors"] == ["launch-failed"]:
        cls = "launch-failed"
    elif code == 90:
        cls = "front-door-refused"
    elif code == 94 or (terminal and terminal.get("retire", {}).get("ok") is False and terminal.get("retire", {}).get("stop") != "unknown"):
        cls = "cleanup-failed"
    elif not complete or code == 93 or (code in (0, 87) and not relay_complete):
        cls = "incomplete"
    elif "cancel" in sends or code == 92:
        cls = "cancelled"
    elif code in (0, 87) and answer is not None and turn_end and turn_end.get("stop_reason") == "end_turn":
        cls = "answered"
    elif code in (0, 87):
        cls = "no-answer"
    else:
        cls = "ended-otherwise"
    return cls, answer, turn_end, linked, terminal


EXITS = {
    "answered": 0, "no-answer": 1, "refused-locally": 3, "front-door-refused": 4,
    "cancelled": 5, "incomplete": 6, "cleanup-failed": 7, "launch-failed": 8, "ended-otherwise": 9,
}


def children_of(events):
    """The owner's child admissions, refusals and results, as reported."""
    pick = lambda name: [e for e in events if e.get("event") == name]
    terminal = [e for e in events if isinstance(e.get("children"), dict) and "starts" in e["children"]]
    return {
        "accepted": pick("child-accepted"),
        "refused": pick("child-refused"),
        "setup_failed": pick("child-setup-failed"),
        "results": pick("child-result"),
        "owner_summary": terminal[-1]["children"] if terminal else None,
    }


def write_json(path, value):
    with open(path, "w", encoding="utf-8") as file:
        json.dump(value, file, indent=1, sort_keys=True)
        file.write("\n")


def main_checked(argv):
    args = parse_args(argv)
    try:
        os.mkdir(args.out, 0o700)
    except OSError as error:
        print(json.dumps({"class": "refused-locally", "reason": type(error).__name__, "started": False}), file=sys.stderr)
        return 2
    now = time.time()
    try:
        request, public = build_request(args, now)
    except (LocalRefusal, OSError, ValueError, TypeError, OverflowError) as refusal:
        write_json(os.path.join(args.out, "result.json"), {"class": "refused-locally", "reason": str(refusal) if isinstance(refusal, LocalRefusal) else type(refusal).__name__, "started": False})
        return EXITS["refused-locally"]
    write_json(os.path.join(args.out, "request.public.json"), public)
    call = Call(args, args.out)
    code, collection = call.run(request)
    request = None
    cls, answer, turn_end, linked, terminal = classify(code, collection, call.events, call.sends)
    if answer is not None:
        with open(os.path.join(args.out, "final.md"), "w", encoding="utf-8") as file:
            file.write(answer)
    of = lambda name: sum(1 for e in call.events if e.get("event") == name)
    children = children_of(call.events)
    write_json(os.path.join(args.out, "children.json"), children)
    write_json(os.path.join(args.out, "result.json"), {
        "class": cls,
        "front_door_exit": code,
        "front_door_terminal": terminal,
        "collection": collection,
        "sends": call.sends,
        "answer": {"present": answer is not None, "linked_messages": linked, "turn_end": turn_end},
        "counts": {
            "lines": len(call.events),
            "bash_accepted": of("bash-accepted"),
            "bash_ended": of("bash-ended"),
            "agent_messages": sum(1 for e in call.events if e.get("event") == "agent-message" and parent_event(e)),
            "child_accepted": of("child-accepted"),
            "child_refused": of("child-refused"),
            "child_results": of("child-result"),
        },
        "children": {"file": "children.json", "results": len(children["results"]),
                     "meaning": "owner-reported child outcomes and lifecycles; not parent consumption"},
        "processing_completion": "not-observed",
        "correctness": "not-established",
        "retention": {
            "out": args.out,
            "holds": "turn text, Bash argv, owner and entry records; arbitrary output may contain secrets",
            "removal": "the caller's",
        },
        "retry": "do-not-replay",
    })
    call.actions.close()
    call.capture.close()
    return EXITS[cls]


def main(argv):
    try:
        return main_checked(argv)
    except (OSError, ValueError, TypeError, OverflowError) as error:
        # result.json itself may be unwritable. Always expose a type-only
        # machine-readable failure on stderr, without echoing inputs.
        result = {"class": "incomplete", "reason": type(error).__name__, "stop": "unknown", "retry": "do-not-replay"}
        try:
            args = parse_args(argv)
            write_json(os.path.join(args.out, "result.json"), result)
        except (OSError, ValueError, TypeError, OverflowError):
            pass
        print(json.dumps(result), file=sys.stderr)
        return EXITS["incomplete"]


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
