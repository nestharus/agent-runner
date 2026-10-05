#!/usr/bin/python3 -I
"""oulipoly-native-call: run one task on a host-root native ACP v2 root
through the installed front door, as the calling user (Linux x86_64).

    oulipoly-native-call --route NAME --prompt-file FILE --cwd DIR --out DIR
        (--trusted-task | --allow-file FILE) [--deadline SECONDS]
        [--env NAME=VALUE ...] [--retention discard|keep]
        [--credential-opencode-auth FILE --credential-provider ID
         | --credential-codex-profile DIR]

It sends one request through `sudo -n <front door> run` (the front door
next to this script in the same package unless `--frontdoor` names one),
keeps the front door's stdin open as its control channel, and captures its
whole stdout. On the first turn end of the message it sends `close`; at
the deadline, or on SIGINT/SIGTERM, it sends `cancel` (the front door kills
the root after its grace). It never retries, replays or falls back.

Credential (only when the route takes one): read here, as the caller, from
exactly the source named, never searched for:

* `--credential-opencode-auth FILE --credential-provider ID`: an OpenCode
  `auth.json`; its `ID` entry must be `oauth`.
* `--credential-codex-profile DIR`: a Codex CLI profile's `DIR/auth.json`;
  its access token's own expiry is used and the entry is for `openai`.

Either way only the access token, its expiry and an account id are sent
(`refresh` empty): no refresh grant leaves the source, and nothing is
written back. It is refused here, before anything starts, if it expires
before the deadline plus `--credential-margin`. The value goes only into
the front door's stdin; never into argv, the environment, a log or `--out`.

`--out` (must not exist; made `0700`) receives, all non-secret:

* `events.jsonl`: the front door's whole stdout, byte for byte (the entry's
  and owner's JSON lines, which carry turn text and Bash argv);
* `stderr.log`: the front door's and entry's stderr;
* `caller.jsonl`: what this caller did and when;
* `request.public.json`: the request with the credential replaced by its
  provider and expiry;
* `final.md`: the last agent message linked to the task's message before
  its turn end, when there is one;
* `result.json`: the outcome class, exits and counts.

Retention of `--out` is the caller's; it holds turn text and Bash argv and
no credential. Exit status (also `result.json` `class`):

* `0` answered: an answer text, its turn end, a close followed through
  (entry `87`) or a clean end (`0`), credentials removed, complete capture;
* `1` no-answer: complete and closed, but no linked answer;
* `2` usage;
* `3` refused-locally: nothing was started (input or credential);
* `4` front-door-refused (`90`), nothing started there;
* `5` cancelled: deadline or signal;
* `6` incomplete: no front door terminal, or capture or exit not observed;
* `7` cleanup-failed: the front door could not remove a credential file;
* `8` launch-failed: the front door could not be started;
* `9` ended-otherwise: any other front door or entry status (see
  `result.json`).
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
    if args.deadline < 1:
        parser.error("--deadline must be positive")
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
            raise LocalRefusal(f"--env {item!r} is not NAME=VALUE")
        env[name] = value
    credential = None
    if args.credential_opencode_auth:
        credential = from_opencode_auth(args.credential_opencode_auth, args.credential_provider)
    elif args.credential_codex_profile:
        credential = from_codex_profile(args.credential_codex_profile)
    if credential is not None:
        check_fresh(credential, args.deadline, args.credential_margin, now)
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
    public = dict(request, credential=credential_public(credential, now))
    if credential is not None:
        request["credential"] = credential
    return request, public


def answer_of(events):
    """The last agent message linked to input 0 before input 0's first
    turn end, and that turn end."""
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
        self.actions = open(os.path.join(out, "caller.jsonl"), "w", encoding="utf-8")
        self.capture = open(os.path.join(out, "events.jsonl"), "wb")

    def log(self, **fields):
        self.actions.write(json.dumps({**fields, "t": round(time.monotonic() - self.start, 3)}, sort_keys=True) + "\n")
        self.actions.flush()

    def send(self, proc, cmd, why):
        if cmd in self.sends or "cancel" in self.sends or proc.stdin.closed:
            return
        self.sends[cmd] = {"why": why, "t": round(time.monotonic() - self.start, 3)}
        try:
            proc.stdin.write(json.dumps({"cmd": cmd}).encode() + b"\n")
            proc.stdin.flush()
            self.sends[cmd]["sent"] = True
        except (OSError, ValueError) as error:
            self.sends[cmd]["sent"] = False
            self.sends[cmd]["error"] = type(error).__name__
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
        if value.get("event") == "turn-end" and value.get("input") == 0:
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
        stderr = open(os.path.join(self.out, "stderr.log"), "wb")
        try:
            proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr, env=env, cwd="/")
        except OSError as error:
            self.log(action="launch-failed", error=type(error).__name__)
            return None, {"stdout_eof": False, "exit": None, "errors": ["launch-failed"]}
        self.log(action="started", argv=argv, deadline_s=self.args.deadline)
        errors = []
        try:
            proc.stdin.write(json.dumps(request).encode() + b"\n")
            proc.stdin.flush()
        except OSError as error:
            errors.append(f"request-write-{type(error).__name__}")
        request = None
        deadline = self.start + self.args.deadline
        old = {sig: signal.signal(sig, lambda sig, frame: self.signals.append(sig)) for sig in (signal.SIGINT, signal.SIGTERM)}
        selector = selectors.DefaultSelector()
        selector.register(proc.stdout, selectors.EVENT_READ)
        buffer = b""
        eof = False
        terminal_at = None
        try:
            while not eof:
                now = time.monotonic()
                if self.signals:
                    self.send(proc, "cancel", f"caller signal {self.signals[0]}")
                if now >= deadline:
                    self.send(proc, "cancel", f"deadline {self.args.deadline}s")
                if terminal_at is not None and now >= terminal_at + EOF_GRACE_S:
                    errors.append("stdout-eof-not-observed-after-terminal")
                    break
                for key, _ in selector.select(timeout=0.2):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        eof = True
                        break
                    self.capture.write(chunk)
                    self.capture.flush()
                    buffer += chunk
                    while b"\n" in buffer:
                        raw, buffer = buffer.split(b"\n", 1)
                        self.line(proc, raw)
                        if self.events[-1].get("frontdoor") == "terminal" and terminal_at is None:
                            terminal_at = time.monotonic()
                            proc.stdin.close()
            if buffer:
                errors.append("unterminated-stdout-line")
                self.line(proc, buffer)
            try:
                code = proc.wait(timeout=EOF_GRACE_S)
            except subprocess.TimeoutExpired:
                code = None
                errors.append("front-door-exit-not-observed")
        finally:
            selector.close()
            for sig, handler in old.items():
                signal.signal(sig, handler)
            if not proc.stdin.closed:
                try:
                    proc.stdin.close()
                except OSError:
                    pass
            proc.stdout.close()
            stderr.close()
        self.log(action="ended", exit=code, stdout_eof=eof)
        return code, {"stdout_eof": eof, "exit": code, "errors": errors}


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
    elif code == 94:
        cls = "cleanup-failed"
    elif not complete or (code in (0, 87) and not relay_complete):
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


def write_json(path, value):
    with open(path, "w", encoding="utf-8") as file:
        json.dump(value, file, indent=1, sort_keys=True)
        file.write("\n")


def main(argv):
    args = parse_args(argv)
    try:
        os.mkdir(args.out, 0o700)
    except OSError as error:
        print(f"oulipoly-native-call: --out {args.out}: {error}", file=sys.stderr)
        return 2
    now = time.time()
    try:
        request, public = build_request(args, now)
    except LocalRefusal as refusal:
        write_json(os.path.join(args.out, "result.json"), {"class": "refused-locally", "reason": str(refusal), "started": False})
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
            "agent_messages": of("agent-message"),
        },
        "processing_completion": "not-observed",
        "correctness": "not-established",
        "retention": {
            "out": args.out,
            "holds": "turn text, Bash argv, owner and entry records; no credential",
            "removal": "the caller's",
        },
        "retry": "do-not-replay",
    })
    call.actions.close()
    call.capture.close()
    return EXITS[cls]


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
