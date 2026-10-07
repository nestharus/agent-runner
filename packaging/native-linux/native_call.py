#!/usr/bin/python3 -I
"""Run one Linux native ACP v2 task through its installed front door.

    oulipoly-native-call --route NAME --prompt-file FILE --cwd DIR --out DIR
        (--trusted-task | --allow-file FILE) [--deadline SECONDS]
        [--env NAME=VALUE ...] [--retention discard|keep]
        [--child-route NAME ... [--child-max-starts N] [--child-max-concurrent N]]
        [--live-handle NEWFILE]
    oulipoly-native-call --root HANDLE --out DIR
        (--prompt-file FILE | --close | --cancel | --inspect | --hold | --release)
        [--wait SECONDS]

--live-handle opens a live root: its supervisor outlives this call; --root
addresses it later (same requester), see share/README.md "Live roots".

Parent and child routes are registered external providers. Adapter settings
and environment own authentication; this caller prepares no credentials.
Nonblocking admission/control and bounded terminal/EOF/exit collection;
unknown stop is incomplete, without pretending a privileged child ended.
Output is captured without a redactor and can contain arbitrary secrets.
Answered/0 means linked text plus complete transport, not task correctness.
The answer and the automatic close are the parent's only: events marked
as a registered child's (`child`) never supply either. Child results and
lifecycles are kept separately (children.json); they are what the owner
reported, not proof any child or its Bash drained beyond what they say.
Exit classes and runtime bounds: share/README.md.
"""

import argparse
import errno
import json
import os
import selectors
import signal
import socket
import stat
import subprocess
import sys
import time

EOF_GRACE_S = 30
STOP_GRACE_S = 65
LINE_LIMIT = 8 * 1024 * 1024
CAPTURE_LIMIT = 64 * 1024 * 1024
# The front door's default site bound; a site may admit less (it refuses).
MAX_DEADLINE_S = 7200
FRONTDOOR = "libexec/oulipoly-native-frontdoor"


class LocalRefusal(Exception):
    """Refused before anything was started."""


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
    parser.add_argument("--child-route", action="append", default=[])
    parser.add_argument("--child-max-starts", type=int)
    parser.add_argument("--child-max-concurrent", type=int)
    parser.add_argument("--live-handle",
                        help="open a live root: no automatic close; its handle is written to this new file")
    parser.add_argument("--frontdoor")
    parser.add_argument("--sudo", default="/usr/bin/sudo")
    # Test seam only: run the front door directly (the caller is root in a
    # test user namespace) and name the requester it would get from sudo.
    parser.add_argument("--direct-requester-uid", type=int, help=argparse.SUPPRESS)
    parser.add_argument("--direct-site-config", help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if not 1 <= args.deadline <= MAX_DEADLINE_S:
        parser.error(f"--deadline must be 1..{MAX_DEADLINE_S}")
    if not args.child_route and any(value is not None for value in (args.child_max_starts, args.child_max_concurrent)):
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
    return request, dict(request)


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


def async_of(events):
    """Reconcile reports with the owner's terminal account. Missing trailing
    reports are not unsettled debt when the owner supplies its actual end
    account; contradictory or missing terminal evidence remains explicit."""
    parent = [e for e in events if parent_event(e)]
    reports = [e for e in parent if e.get("event") == "async-owed"]
    completions = [e for e in parent if e.get("event") == "bash-async-completion-admitted"]
    accepted = [e.get("work") for e in reports if e.get("change") == "owed"]
    ended = [e.get("work") for e in reports if e.get("change") == "turn-ended"]
    lost = [{"work": e.get("work"), "reason": e.get("reason")}
            for e in reports if e.get("change") == "undelivered"]
    observed = {"accepted": len(accepted), "turn_ended": len(ended),
                "undelivered": lost, "unsettled": len(accepted) - len(ended) - len(lost)}
    terminals = [e for e in parent if e.get("event") == "terminal"]
    owner = terminals[-1].get("async") if terminals else None
    account = dict(observed)
    errors, gaps = [], []
    active = bool(reports or completions)
    for completion in completions:
        work = completion.get("work")
        if type(work) is not int or work not in accepted:
            errors.append(f"async-completion-work-not-in-owed-reports: {work!r}")
    if owner is None:
        if active:
            errors.append("owner-terminal-async-account-missing")
    elif not isinstance(owner, dict) or not all(
            type(owner.get(k)) is int and owner[k] >= 0 for k in ("accepted", "turn_ended", "owed")) \
            or not isinstance(owner.get("undelivered"), list) \
            or not all(isinstance(v, dict) and type(v.get("work")) is int
                       and isinstance(v.get("reason"), str) and v["reason"]
                       for v in owner["undelivered"]):
        errors.append("owner-terminal-async-account-invalid")
    else:
        account = {"accepted": owner["accepted"], "turn_ended": owner["turn_ended"],
                   "undelivered": owner["undelivered"], "unsettled": owner["owed"]}
        for loss in owner["undelivered"]:
            if loss["work"] not in accepted:
                errors.append(f"owner-terminal-undelivered-work-not-in-owed-reports: {loss['work']!r}")
        if owner["accepted"] != owner["turn_ended"] + len(owner["undelivered"]) + owner["owed"]:
            errors.append("owner-terminal-async-totals-inconsistent")
        if len(accepted) != len(set(accepted)) or len(ended) != len(set(ended)) \
                or len(lost) != len({v["work"] for v in lost}) \
                or set(ended) & {v["work"] for v in lost} \
                or set(ended) & {v["work"] for v in owner["undelivered"]} \
                or not (set(ended) | {v["work"] for v in lost}) <= set(accepted) \
                or len(accepted) > owner["accepted"] or len(ended) > owner["turn_ended"] \
                or any(v not in [{"work": u["work"], "reason": u["reason"]}
                                 for u in owner["undelivered"]] for v in lost) \
                or len(owner["undelivered"]) != len({v["work"] for v in owner["undelivered"]}):
            errors.append("async-reports-contradict-owner-terminal")
        if observed != {**account, "undelivered": [{"work": u["work"], "reason": u["reason"]}
                                                 for u in owner["undelivered"]]}:
            gaps.append("async-reports-differ-from-owner-terminal; see both accounts")
    return {
        **account, "active": active or bool(account["accepted"]), "reports": observed, "owner_summary": owner,
        "errors": errors, "evidence_gaps": gaps,
        "meaning": "turn_ended: the completion input was acknowledged and a tagged turn end covered it; not proof the agent used or accepted the output",
    }


def turns_of(events):
    """Every linked turn of the parent: its input, kind, last linked text
    and turn end, in turn-end order."""
    events = [e for e in events if parent_event(e)]
    acks = {}
    completions = {e.get("input"): e.get("work") for e in events if e.get("event") == "bash-async-completion-admitted"}
    texts = {}
    turns = []
    for event in events:
        name = event.get("event")
        if name == "ack" and event.get("message_id"):
            acks[event.get("index")] = event["message_id"]
        elif name == "agent-message" and event.get("input") is not None \
                and event.get("parent_message_id") == acks.get(event.get("input")) \
                and str(event.get("text", "")).strip():
            texts[event["input"]] = event["text"]
        elif name == "turn-end" and event.get("input") is not None:
            index = event["input"]
            turns.append({
                "input": index,
                "kind": "initial" if index == 0 else "background-completion" if index in completions else "follow-up",
                "work": completions.get(index),
                "text": texts.get(index),
                "stop_reason": event.get("stop_reason"),
                "acknowledged": index in acks,
                "ended": True,
            })
    # Preserve linked diagnostic text even when its carrying turn never ends.
    covered = {t["input"] for t in turns}
    turns += [{"input": index, "kind": "initial" if index == 0 else "background-completion" if index in completions else "follow-up",
               "work": completions.get(index), "text": text, "stop_reason": None,
               "acknowledged": True, "ended": False}
              for index, text in texts.items() if index not in covered]
    return turns


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
        self.owed_async = 0
        self.actions = open(os.path.join(out, "caller.jsonl"), "w", encoding="utf-8")
        self.capture = open(os.path.join(out, "events.jsonl"), "wb")

    def log(self, **fields):
        self.actions.write(json.dumps({**fields, "t": round(time.monotonic() - self.start, 3)}, sort_keys=True) + "\n")
        self.actions.flush()

    def send(self, proc, cmd, why):
        if cmd in self.sends or "cancel" in self.sends or proc.stdin.closed:
            return
        self.sends[cmd] = {"why": why, "t": round(time.monotonic() - self.start, 3)}
        if cmd == "close" and self.owed_async > 0:
            # The owner keeps its harness for owed background completions;
            # the stop bound starts once none is owed (or at the deadline).
            self.sends[cmd]["stop_bound"] = "after-owed-background-completions"
        else:
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
        if value.get("event") == "async-owed" and parent_event(value) and type(value.get("owed_async")) is int:
            self.owed_async = value["owed_async"]
            if self.owed_async == 0 and "close" in self.sends and "stop_bound" in self.sends["close"]:
                self.stop_at = min(self.stop_at or float("inf"), time.monotonic() + STOP_GRACE_S)
        if value.get("event") == "turn-end" and value.get("input") == 0 and parent_event(value) \
                and not getattr(getattr(self, "args", None), "live_handle", None):
            self.send(proc, "close", "turn end of the message (readiness, not processing success); owed background completions still get their own turns")

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
    background = async_of(events)
    turns = turns_of(events)
    if background["active"]:
        texts = [t["text"] for t in turns if t["text"] is not None]
        answer = texts[-1] if texts else None
    complete = bool(terminal) and collection["stdout_eof"] and not collection["errors"] and code is not None
    entries = [e for e in events if e.get("entry") == "terminal"]
    relay_complete = bool(entries) and entries[-1].get("relay") == "complete"
    if code is None and collection["errors"] == ["launch-failed"]:
        cls = "launch-failed"
    elif code == 90:
        cls = "front-door-refused"
    elif code == 94 or (terminal and terminal.get("retire", {}).get("ok") is False and terminal.get("retire", {}).get("stop") != "unknown"):
        cls = "cleanup-failed"
    elif not complete or code == 93 or (code in (0, 83, 87) and not relay_complete):
        cls = "incomplete"
    elif "cancel" in sends or code == 92:
        cls = "cancelled"
    elif code in (0, 83, 87) and background["errors"]:
        cls = "incomplete"
    elif code in (0, 83, 87) and background["undelivered"]:
        cls = "async-undelivered"
    elif code in (0, 87) and background["unsettled"] != 0:
        cls = "incomplete"
    elif code in (0, 87) and background["accepted"] and (
            turn_end is None or len({t["work"] for t in turns if t["kind"] == "background-completion"
                 and t["acknowledged"] and t["ended"]}) != background["accepted"]
            or any(not t["acknowledged"] or not t["ended"]
                   for t in turns if t["kind"] == "background-completion")):
        cls = "incomplete"
    elif code in (0, 87) and answer is not None and turn_end and turn_end.get("stop_reason") == "end_turn" \
            and all(t["stop_reason"] == "end_turn"
                    for t in turns if background["accepted"] and t["kind"] == "background-completion"):
        cls = "answered"
    elif code in (0, 87):
        cls = "no-answer"
    else:
        cls = "ended-otherwise"
    return cls, answer, turn_end, linked, terminal


EXITS = {
    "answered": 0, "no-answer": 1, "refused-locally": 3, "front-door-refused": 4,
    "cancelled": 5, "incomplete": 6, "cleanup-failed": 7, "launch-failed": 8, "ended-otherwise": 9,
    "async-undelivered": 10,
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
    if args.live_handle:
        if os.path.lexists(args.live_handle):
            write_json(os.path.join(args.out, "result.json"), {"class": "refused-locally", "reason": "live handle file exists", "started": False})
            return EXITS["refused-locally"]
        request["live"] = True
        public["live"] = True
    write_json(os.path.join(args.out, "request.public.json"), public)
    call = Call(args, args.out)
    code, collection = call.run(request)
    request = None
    terminals = [e for e in call.events if e.get("frontdoor") == "terminal"]
    if args.live_handle and terminals and terminals[-1].get("stage") == "live-opened" and code == 0 \
            and not collection["errors"]:
        return opened_live(args, call, terminals[-1], collection)
    cls, answer, turn_end, linked, terminal = classify(code, collection, call.events, call.sends)
    background = async_of(call.events)
    turns = turns_of(call.events)
    final = answer
    if background["active"] and any(t["text"] is not None for t in turns):
        # Every linked turn's answer, in order: the first turn alone is not
        # a complete background task.
        final = "\n\n".join(
            f"## Turn {n} (input {t['input']}, {t['kind']}{'' if t['work'] is None else ', work ' + str(t['work'])})\n\n"
            + (t["text"] if t["text"] is not None else "(no linked answer text)")
            + ("" if t["ended"] else "\n\n(carrying turn end not observed; diagnostic text)")
            for n, t in enumerate(turns, 1))
    if final is not None:
        with open(os.path.join(args.out, "final.md"), "w", encoding="utf-8") as file:
            file.write(final)
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
        "async": background,
        "turns": [{k: v for k, v in t.items() if k != "text"} | {"answered": t["text"] is not None} for t in turns],
        "counts": {
            "lines": len(call.events),
            "bash_accepted": of("bash-accepted"),
            "bash_ended": of("bash-ended"),
            "bash_scope": "parent and child Bash combined",
            "agent_messages": sum(1 for e in call.events if e.get("event") == "agent-message" and parent_event(e)),
            "child_accepted": of("child-accepted"),
            "child_refused": of("child-refused"),
            "child_results": of("child-result"),
        },
        "children": {"file": "children.json", "results": len(children["results"]),
                     "meaning": "owner exports, possibly incomplete before Bash drain; local parent result can precede export; zero exports does not mean zero local results; not parent consumption"},
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


# Live roots: one owning supervisor that outlives the call that opened it.

LIVE_EXITS = {
    "answered": 0, "closed": 0, "no-answer": 1, "refused-locally": 3, "cancelled": 5, "incomplete": 6,
    "cleanup-failed": 7, "ended-otherwise": 9, "root-absent": 11, "root-dead": 12, "root-foreign": 13,
    "root-refused": 14, "follow-up-refused": 15, "root-ended": 16,
    "root-unavailable": 17, "async-undelivered": 10,
    "inspected": 0, "acknowledged": 0, "control-refused": 18, "control-unknown": 19,
}

CONTROL_PROTOCOL = "oulipoly.session_control/v2"


def read_private(path):
    """The caller's own regular file (its live handle), read without
    following a final symlink or blocking on a FIFO, bounded."""
    st = os.lstat(path)
    if not stat.S_ISREG(st.st_mode):
        raise LocalRefusal("live handle is not a regular file")
    if st.st_uid != os.getuid():
        raise LocalRefusal("live handle is not the caller's own file")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    try:
        opened = os.fstat(fd)
        if not stat.S_ISREG(opened.st_mode) or opened.st_uid != os.getuid():
            raise LocalRefusal("live handle changed or is not the caller's own regular file")
        chunks = []
        while True:
            chunk = os.read(fd, 65536)
            if not chunk:
                break
            chunks.append(chunk)
            if sum(map(len, chunks)) > 1024 * 1024:
                raise LocalRefusal("live handle is too large")
        return b"".join(chunks)
    finally:
        os.close(fd)


def read_handle(path):
    try:
        handle = json.loads(read_private(path))
    except LocalRefusal:
        raise
    except (OSError, ValueError) as error:
        raise LocalRefusal(f"live handle: {type(error).__name__}") from None
    return check_handle(handle)


def check_handle(handle):
    if not isinstance(handle, dict) or handle.get("v") != 1 \
            or not all(isinstance(handle.get(k), str) and handle[k] for k in ("run", "socket", "token")) \
            or not handle["socket"].startswith("/") or type(handle.get("uid")) is not int:
        raise LocalRefusal("live handle: shape")
    try:
        address = os.fsencode(handle["socket"])
        json.dumps(handle, ensure_ascii=False).encode("utf-8")
    except UnicodeError:
        raise LocalRefusal("live handle: invalid Unicode") from None
    if b"\0" in address or len(address) >= 108:
        raise LocalRefusal("live handle: unusable AF_UNIX address")
    if handle["uid"] != os.getuid():
        raise LocalRefusal("live handle names another requester")
    return handle


def write_handle(path, handle):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as file:
        json.dump(handle, file, sort_keys=True)
        file.write("\n")


class Attached:
    """One caller attachment to a live root: connect, prove the handle,
    then exchange JSON lines until the caller's own goal or bound."""

    def __init__(self, handle, out, actions=None, capture=None):
        self.handle = handle
        self.start = time.monotonic()
        self.events = []
        self.actions = actions or open(os.path.join(out, "caller.jsonl"), "a", encoding="utf-8")
        self.capture = capture or open(os.path.join(out, "events.jsonl"), "ab")
        self.sock = None
        self.buffer = b""
        self.eof = False
        self.signals = []
        self.errors = []

    def log(self, **fields):
        self.actions.write(json.dumps({**fields, "t": round(time.monotonic() - self.start, 3)}, sort_keys=True) + "\n")
        self.actions.flush()

    def connect(self):
        """None when attached, else the refusal class and its reason."""
        sock = None
        try:
            check_handle(self.handle)
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM | socket.SOCK_CLOEXEC)
            sock.settimeout(5)
            sock.connect(self.handle["socket"])
            self.sock = sock
            self.write({"v": 1, "attach": {"run": self.handle["run"], "token": self.handle["token"]}})
            first = self.next_event(time.monotonic() + 5)
        except LocalRefusal as error:
            return "refused-locally", str(error)
        except OSError as error:
            if sock is not None:
                sock.close()
            self.sock = None
            if isinstance(error, FileNotFoundError) or error.errno == errno.ENOTDIR:
                return "root-absent", "no live address: ended and retired, or never existed; terminal outcome unknown"
            if isinstance(error, ConnectionRefusedError):
                return "root-dead", "address present but no supervisor listening: owner died without retirement"
            if isinstance(error, PermissionError):
                return "root-foreign", "address not accessible to this requester"
            return "root-unavailable", type(error).__name__
        if first is None:
            return "root-refused", "no attach answer"
        if first.get("frontdoor") == "attach-refused":
            reason = first.get("reason")
            return ("root-foreign" if reason == "foreign-requester" else "root-refused"), reason
        if first.get("frontdoor") != "attached":
            return "root-refused", "unexpected attach answer"
        self.log(action="attached", attach=first.get("attach"), backlog_dropped=first.get("backlog_dropped"))
        return None

    def write(self, value):
        self.sock.settimeout(5)
        self.sock.sendall(json.dumps(value).encode() + b"\n")

    def next_event(self, until):
        while b"\n" not in self.buffer:
            if self.eof:
                return None
            remaining = until - time.monotonic()
            if remaining <= 0 or self.signals:
                return None
            self.sock.settimeout(min(remaining, 0.2))
            try:
                chunk = self.sock.recv(65536)
            except socket.timeout:
                continue
            except OSError as error:
                self.errors.append("transport-read:" + type(error).__name__)
                chunk = b""
            if not chunk:
                if self.buffer:
                    self.errors.append("interrupted-transport-record")
                self.eof = True
                continue
            self.capture.write(chunk)
            self.buffer += chunk
            if len(self.buffer) > LINE_LIMIT:
                self.errors.append("transport-line-limit")
                self.eof = True
                return None
        raw, self.buffer = self.buffer.split(b"\n", 1)
        try:
            value = json.loads(raw)
            if not isinstance(value, dict):
                raise ValueError
        except ValueError:
            self.errors.append("invalid-transport-record")
            value = {"unparsed": True}
        self.events.append(value)
        return value

    def detach(self):
        try:
            self.write({"cmd": "detach"})
        except OSError:
            pass
        self.close()

    def close(self):
        if self.sock is not None:
            self.sock.close()
            self.sock = None
        self.capture.flush()

    def turn(self, index, ref, until):
        """Waits for the caller's own input: admitted (by ref), ACK, its
        linked agent text and tagged turn end. Returns the account."""
        account = {"input": index, "ref": ref, "admitted": index is not None, "ack": None,
                   "linked_messages": 0, "turn_end": None, "refused": None, "root_ended": None}
        text = None
        while True:
            event = self.next_event(until)
            if event is None:
                account["stop"] = "eof" if self.eof else "signal" if self.signals else "wait-bound"
                break
            if event.get("frontdoor") == "terminal":
                account["root_ended"] = event
                break
            if not parent_event(event):
                continue
            name = event.get("event")
            if ref is not None and event.get("ref") == ref:
                if name == "follow-up-admitted" and type(event.get("input")) is int:
                    account["input"], account["admitted"] = event["input"], True
                elif name == "follow-up-refused":
                    account["refused"] = event.get("reason")
                    break
            if account["input"] is None:
                continue
            if name == "ack" and event.get("index") == account["input"] and event.get("message_id"):
                account["ack"] = {k: event.get(k) for k in ("message_id", "label", "durable")}
            elif name == "agent-message" and event.get("input") == account["input"] and account["ack"] \
                    and event.get("parent_message_id") == account["ack"]["message_id"]:
                account["linked_messages"] += 1
                if str(event.get("text", "")).strip():
                    text = event["text"]
            elif name == "turn-end" and event.get("input") == account["input"]:
                account["turn_end"] = event
                break
        account["answer_present"] = text is not None
        return account, text


def turn_class(account, text):
    if account["refused"] is not None:
        return "follow-up-refused"
    if account["turn_end"] is None:
        return "root-ended" if account["root_ended"] is not None else "incomplete"
    if text is not None and account["ack"] and account["turn_end"].get("stop_reason") == "end_turn":
        return "answered"
    return "no-answer"


def live_close_account(terminal, events):
    """Use the live supervisor's runtime-only witness across attachments.
    Caller-local records alone may omit earlier turns/reports. Never infer
    whole-root settlement from the closer's own turn or physical retirement."""
    witness = terminal.get("account_events") if terminal else None
    errors = list(terminal.get("account_errors", [])) if terminal else []
    if not isinstance(witness, list) or not all(isinstance(e, dict) for e in witness):
        witness = [e for e in events if parent_event(e)]
        errors.append("live-account-witness-missing-or-invalid")
    owners = [e for e in witness if e.get("event") == "terminal" and parent_event(e)]
    entries = [e for e in witness if e.get("entry") == "terminal"]
    owner = owners[-1] if owners else None
    if owner is None or owner.get("async") is None:
        errors.append("owner-terminal-async-account-missing")
    if len(owners) != 1:
        errors.append("owner-terminal-missing-or-multiple")
    if len(entries) != 1 or entries[-1].get("relay") != "complete":
        errors.append("entry-relay-completion-missing-or-contradictory")
    background = async_of(witness)
    errors.extend(background["errors"])
    if terminal and terminal.get("exit") in (0, 87):
        if owner and owner.get("status") != "closed":
            errors.append("owner-terminal-contradicts-close")
        if terminal.get("entry_status") not in (0, 87) or terminal.get("killed"):
            errors.append("entry-status-contradicts-close")
    turns = turns_of(witness)
    completed = {t["work"] for t in turns if t["kind"] == "background-completion"
                 and t["acknowledged"] and t["ended"] and t["stop_reason"] == "end_turn"}
    if not background["undelivered"] and (background["unsettled"] != 0
            or len(completed) != background["accepted"]):
        errors.append("async-carrying-turns-incomplete")
    return {"async": background, "owner_terminal": owner, "errors": errors,
            "meaning": "owner reports, ACK and tagged carrying turn ends; not semantic processing"}


def end_class(terminal, account, collection, cmd):
    if terminal is None:
        return "incomplete"
    code = terminal.get("exit")
    retired = terminal.get("retire", {})
    if code == 94 or (retired.get("ok") is False and retired.get("stop") != "unknown"):
        return "cleanup-failed"
    if code in (90, 91, 93) or code is None or terminal.get("collection_errors") \
            or not collection["eof"] or collection["errors"] or retired.get("stop") == "unknown":
        return "incomplete"
    if cmd == "cancel" or code == 92 or (terminal.get("cancel") or "").startswith(("abandoned", "deadline", "requester cancel")):
        return "cancelled"
    if code in (0, 83, 87):
        if retired.get("ok") is not True or account["errors"]:
            return "incomplete"
        if account["async"]["undelivered"]:
            return "async-undelivered"
        if terminal.get("live", {}).get("backlog_dropped_total", 0):
            return "incomplete"
        return "closed" if code in (0, 87) else "ended-otherwise"
    return "ended-otherwise"


def live_result(out, cls, fields):
    if cls not in LIVE_EXITS:
        fields = {**fields, "classification_error": "undeclared live outcome: " + str(cls)}
        cls = "incomplete"
    write_json(os.path.join(out, "result.json"), {
        "class": cls, **fields,
        "processing_completion": "not-observed", "correctness": "not-established", "retry": "do-not-replay",
    })
    return LIVE_EXITS[cls]


def caller_signals(attached):
    return {sig: signal.signal(sig, lambda sig, frame: attached.signals.append(sig)) for sig in (signal.SIGINT, signal.SIGTERM)}


def opened_live(args, call, terminal, collection):
    """The opening caller: the root is live; write its handle, attach and
    wait for input 0's own turn, then leave the root running."""
    handle = terminal.get("handle")
    base = {"front_door_terminal": {k: v for k, v in terminal.items() if k != "handle"}, "collection": collection}
    try:
        check_handle(handle)
    except LocalRefusal as error:
        return live_result(args.out, "refused-locally", {**base, "reason": str(error)})
    try:
        write_handle(args.live_handle, handle)
    except (OSError, TypeError, ValueError) as error:
        # Nobody else could address it: close it rather than leave it.
        attached = Attached(handle, args.out, call.actions, call.capture)
        refused = attached.connect()
        if refused is None:
            attached.write({"cmd": "close"})
        attached.close()
        return live_result(args.out, "incomplete", {**base, "reason": "live handle not written: " + type(error).__name__,
                                                   "close": "requested" if refused is None else refused[0]})
    attached = Attached(handle, args.out, call.actions, call.capture)
    old = caller_signals(attached)
    try:
        refused = attached.connect()
        if refused is not None:
            return live_result(args.out, refused[0], {**base, "reason": refused[1], "handle": args.live_handle})
        account, text = attached.turn(0, None, call.start + args.deadline)
        attached.detach()
    finally:
        for sig, handler in old.items():
            signal.signal(sig, handler)
    if text is not None:
        with open(os.path.join(args.out, "final.md"), "w", encoding="utf-8") as file:
            file.write(text)
    cls = "incomplete" if attached.errors else turn_class(account, text)
    return live_result(args.out, cls, {
        **base, "handle": args.live_handle, "run": handle.get("run"), "turn": account,
        "transport": {"errors": attached.errors, "attached": attached.events[0] if attached.events else None},
        "root": "ended" if account["root_ended"] else "live (detached; this exit is not the root's end)",
    })


def parse_live_args(argv):
    parser = argparse.ArgumentParser(prog="oulipoly-native-call --root",
                                     description="Address a live root opened with --live-handle.")
    parser.add_argument("--root", required=True, help="the live handle file")
    parser.add_argument("--out", required=True)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--prompt-file", help="deliver one more input and wait for its own turn")
    action.add_argument("--close", action="store_true", help="close the root: drain and end its tree")
    action.add_argument("--cancel", action="store_true", help="cancel the root")
    action.add_argument("--inspect", action="store_true",
                        help="report the root's control state and settlement reading (session_control/v2)")
    action.add_argument("--hold", action="store_true",
                        help="hold new input (admission only; running work continues)")
    action.add_argument("--release", action="store_true", help="release an input hold")
    parser.add_argument("--wait", type=int, default=1800, help="this call's own bound; the root is unaffected by it")
    args = parser.parse_args(argv)
    if not 1 <= args.wait <= MAX_DEADLINE_S:
        parser.error(f"--wait must be 1..{MAX_DEADLINE_S}")
    return args


def main_live(argv):
    args = parse_live_args(argv)
    try:
        os.mkdir(args.out, 0o700)
    except OSError as error:
        print(json.dumps({"class": "refused-locally", "reason": type(error).__name__, "started": False}), file=sys.stderr)
        return LIVE_EXITS["refused-locally"]
    try:
        handle = read_handle(args.root)
        message = None
        if args.prompt_file:
            with open(args.prompt_file, encoding="utf-8") as file:
                message = file.read()
            if not message.strip():
                raise LocalRefusal("prompt file is empty")
    except (LocalRefusal, OSError, UnicodeError) as refusal:
        return live_result(args.out, "refused-locally", {"reason": str(refusal) if isinstance(refusal, LocalRefusal) else type(refusal).__name__, "started": False})
    attached = Attached(handle, args.out)
    base = {"handle": args.root, "run": handle["run"]}
    old = caller_signals(attached)
    try:
        refused = attached.connect()
        if refused is not None:
            attached.close()
            return live_result(args.out, refused[0], {**base, "reason": refused[1]})
        until = attached.start + args.wait
        if message is not None:
            ref = "c" + os.urandom(8).hex()
            attached.write({"cmd": "send", "text": message, "ref": ref})
            attached.log(action="send", ref=ref)
            account, text = attached.turn(None, ref, until)
            attached.detach()
            if text is not None:
                with open(os.path.join(args.out, "final.md"), "w", encoding="utf-8") as file:
                    file.write(text)
            return live_result(args.out, "incomplete" if attached.errors else turn_class(account, text), {
                **base, "turn": account,
                "transport": {"errors": attached.errors, "attached": attached.events[0] if attached.events else None},
                "root": "ended" if account["root_ended"] else "live (detached; this exit is not the root's end)"})
        if args.inspect or args.hold or args.release:
            return live_control(args, attached, base, until)
        cmd = "close" if args.close else "cancel"
        attached.write({"cmd": cmd})
        attached.log(action="send", cmd=cmd)
        terminal = None
        while True:
            event = attached.next_event(until + STOP_GRACE_S)
            if event is None:
                break
            if event.get("frontdoor") == "terminal":
                terminal = event
        attached.close()
        collection = {"eof": attached.eof, "errors": attached.errors}
        account = live_close_account(terminal, attached.events)
        cls = end_class(terminal, account, collection, cmd)
        return live_result(args.out, cls, {
            **base, "front_door_terminal": terminal, "collection": collection,
            "owner_terminal": account["owner_terminal"], "async": account["async"],
            "account_errors": account["errors"],
            "transport": {"attached": attached.events[0] if attached.events else None,
                          "loss": terminal.get("live") if terminal else None},
            "physical_close": bool(terminal and terminal.get("entry_status") is not None),
            "root": "ended" if terminal else "unknown (no terminal record; stop not observed)"})
    except OSError as error:
        attached.close()
        return live_result(args.out, "incomplete", {**base, "reason": type(error).__name__})
    finally:
        for sig, handler in old.items():
            signal.signal(sig, handler)


def inspected(attached, until):
    """Asks for the root's current state; returns (control_state, settlement)
    or Nones when either does not arrive within `until`."""
    attached.write({"cmd": "inspect"})
    attached.log(action="send", cmd="inspect")
    state = settlement = None
    while settlement is None:
        event = attached.next_event(until)
        if event is None or event.get("frontdoor") == "terminal":
            break
        if event.get("kind") == "control_state":
            state = event
        elif event.get("event") == "settlement" and state is not None:
            settlement = event
    return state, settlement


def live_control(args, attached, base, until):
    """`--inspect`, or one `session_control/v2` hold/release request addressed
    to the authority the root's own inspection reports. The answer is the
    owner's claims for this request; this caller attests nothing itself."""
    state, settlement = inspected(attached, until)
    fields = {**base, "control_state": state, "settlement": settlement}
    if state is None or settlement is None:
        attached.detach()
        return live_result(args.out, "incomplete", {**fields, "reason": "no inspection within the wait bound"})
    if args.inspect:
        attached.detach()
        return live_result(args.out, "inspected", fields)
    key = "c" + os.urandom(8).hex()
    request = {
        "kind": "request", "protocol": CONTROL_PROTOCOL, "request_key": key,
        "requester": f"uid:{os.getuid()}", "addressed": state["reporter"],
        "operation": "input_hold" if args.hold else "input_release",
        "scope": {"root": state["reporter"]["root"]},
    }
    attached.write(request)
    attached.log(action="send", control=request["operation"], request_key=key)
    claims, refusal = [], None
    while True:
        event = attached.next_event(until)
        if event is None or event.get("frontdoor") == "terminal":
            break
        if event.get("frontdoor") == "control-refused" or event.get("event") == "session-control-unavailable":
            refusal = event
            break
        if event.get("request_key") == key and isinstance(event.get("kind"), str):
            claims.append(event)
            if event["kind"] == "outcome" or (event["kind"] == "refusal" and event.get("reason") == "key_conflict"):
                break
    attached.detach()
    outcome = next((c for c in reversed(claims) if c["kind"] == "outcome"), None)
    cls = "control-refused" if refusal is not None else \
        "acknowledged" if outcome and outcome.get("result") == "acknowledged" else \
        "control-refused" if outcome and outcome.get("result") == "refused" else \
        "control-unknown" if outcome else "incomplete"
    return live_result(args.out, cls, {**fields, "request": request, "claims": claims, "refusal": refusal,
                                       "meaning": "input hold is admission/input only; running work continues; not pause, drain or close"})


def main(argv):
    live = any(arg == "--root" or arg.startswith("--root=") for arg in argv)
    try:
        return main_live(argv) if live else main_checked(argv)
    except (OSError, ValueError, TypeError, OverflowError, KeyError, AttributeError, RecursionError) as error:
        # result.json itself may be unwritable. Always expose a type-only
        # machine-readable failure on stderr, without echoing inputs.
        result = {"class": "incomplete", "reason": type(error).__name__, "stop": "unknown", "retry": "do-not-replay"}
        try:
            args = parse_live_args(argv) if live else parse_args(argv)
            write_json(os.path.join(args.out, "result.json"), result)
        except (OSError, ValueError, TypeError, OverflowError, KeyError, AttributeError, RecursionError):
            pass
        print(json.dumps(result), file=sys.stderr)
        return EXITS["incomplete"]


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
