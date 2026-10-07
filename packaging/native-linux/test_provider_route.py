"""Offline checks of the front door's registered-provider route: site shape,
admission custody of the provider executable, the request the entry gets
and the admitted report. Unprivileged: nothing here is root.

`EntryContract` feeds the request this front door produces to the real
Runner entry (`OULIPOLY_RUNNER`, with the root supervisor binaries and the
deterministic ACP peer built beside it) and a stand-in provider: the same
`provider` object reaches the owner and its harness. Only the declared
work isolation is changed, to `unprivileged-userns`, because a host-root
declaration needs a root entry; the host-root path is ROOT's to run."""

import json
import os
import pwd
import select
import subprocess
import tempfile
import time
import unittest
from unittest import mock

import frontdoor

NOW = 1_800_000_000
PROVIDER = {
    "harness": "provider",
    "executable": "/opt/oulipoly-providers/codex/bin/agent-runner-codex",
    "settings": {
        "settings_id": "codex-sol",
        "mode": "arg",
        "model": {"name": "gpt-6.1-sol", "provider_args": [], "inputs": {"prompt": None, "named": {}}},
        "launch": {},
    },
    "env": {"CODEX_HOME_HINT": "x"},
    "credential": "none",
}
SITE = {
    "v": 1,
    "allowed_users": ["nes"],
    "run_base": "/var/lib/oulipoly-native/runs",
    "max_deadline_s": 3600,
    "cancel_grace_s": 30,
    "credential_margin_s": 300,
    "allow_keep": False,
    "default_path": "/usr/local/bin:/usr/bin:/bin",
    "routes": {
        "codex-sol": PROVIDER,
        "fixture": {"model": "fixture/scripted", "credential": "none", "provider": {"fixture": {}}},
    },
    "child_routes": {
        "luna-max": {"model": "openai/gpt-6-luna", "credential": "required", "provider": {"openai": {}}},
    },
}


def site(**routes):
    value = json.loads(json.dumps(SITE))
    value["routes"].update(routes)
    return value


def request(**overrides):
    value = {
        "v": 1,
        "route": "codex-sol",
        "message": "look",
        "cwd": "/home/nes/project",
        "bash": {"allow": ["git status"]},
        "deadline_s": 1800,
    }
    value.update(overrides)
    return value


def loaded(value):
    with tempfile.TemporaryDirectory() as directory:
        path = os.path.join(directory, "frontdoor.json")
        with open(path, "w") as file:
            json.dump(value, file)
        with mock.patch.object(frontdoor, "check_owned"):
            return frontdoor.load_site(path)


class SiteShape(unittest.TestCase):
    def test_provider_route_is_a_site_route(self):
        self.assertEqual(loaded(SITE)["routes"]["codex-sol"]["harness"], "provider")

    def test_refuses_a_malformed_provider_route(self):
        for bad in (
            dict(PROVIDER, executable="relative/provider"),
            dict(PROVIDER, executable="/opt/../provider"),
            dict(PROVIDER, credential="required"),
            dict(PROVIDER, settings={"settings_id": "s"}),
            dict(PROVIDER, settings=dict(PROVIDER["settings"], extra=1)),
            dict(PROVIDER, env={"OULIPOLY_HOST_RESIDENT_SESSION_V1": "1"}),
            dict(PROVIDER, env={"BAD NAME": "x"}),
            dict(PROVIDER, config_root="relative"),
            dict(PROVIDER, model="x"),
            {key: value for key, value in PROVIDER.items() if key != "settings"},
        ):
            with self.assertRaisesRegex(frontdoor.Refused, "route bad", msg=bad):
                loaded(site(bad=bad))


class Admission(unittest.TestCase):
    def test_provider_route_takes_no_credential_and_its_policy(self):
        checked = frontdoor.check_request(request(), SITE, NOW)
        self.assertEqual(checked["policy"], {"bash_allow": ["git status"]})
        self.assertIsNone(checked["credential"])
        with self.assertRaisesRegex(frontdoor.Refused, "takes no credential"):
            frontdoor.check_request(request(credential={"openai": {}}), SITE, NOW)

    def test_provider_parent_children_take_a_separate_grant(self):
        route = dict(PROVIDER, children=["luna-max"])
        value = loaded(site(**{"codex-sol": route}))
        with self.assertRaisesRegex(frontdoor.Refused, "need child_credential"):
            frontdoor.check_request(request(children={"routes": ["luna-max"]}), value, NOW)
        grant = {"openai": {"type": "oauth", "refresh": "", "access": "a", "expires": (NOW + 7200) * 1000}}
        checked = frontdoor.check_request(request(children={"routes": ["luna-max"]}, child_credential=grant), value, NOW)
        self.assertEqual(checked["children"]["grant"][1]["source"], "separate-child-grant")

    def test_executable_custody_is_root_and_an_executable_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, "provider")
            with open(path, "w") as file:
                file.write("#!/bin/sh\n")
            os.chmod(path, 0o755)
            route = dict(PROVIDER, executable=path)
            # Real custody: a file this user owns is not root's.
            with self.assertRaisesRegex(frontdoor.Refused, "not owned by root"):
                frontdoor.check_provider_executable(route)
            with mock.patch.object(frontdoor, "check_owned"):
                frontdoor.check_provider_executable(route)
                os.chmod(path, 0o644)
                with self.assertRaisesRegex(frontdoor.Refused, "not an executable file"):
                    frontdoor.check_provider_executable(route)
                with self.assertRaisesRegex(frontdoor.Refused, "FileNotFoundError"):
                    frontdoor.check_provider_executable(dict(PROVIDER, executable=path + "-absent"))
            # Embedded routes are not provider routes.
            frontdoor.check_provider_executable(SITE["routes"]["fixture"])

    def test_admission_checks_custody_before_any_effect(self):
        calls = []
        with mock.patch.object(frontdoor, "check_provider_executable", side_effect=lambda route: calls.append(route) or (_ for _ in ()).throw(frontdoor.Refused("provider: custody"))), \
             mock.patch.object(frontdoor, "check_package"), \
             mock.patch.object(frontdoor, "load_site", return_value=json.loads(json.dumps(SITE))), \
             mock.patch.object(frontdoor, "requester", return_value=pwd.getpwuid(os.getuid())), \
             mock.patch.object(frontdoor, "read_first_line", return_value=(json.dumps(request()).encode(), b"")), \
             mock.patch.object(frontdoor, "check_cwd_as") as cwd:
            with self.assertRaisesRegex(frontdoor.Refused, "provider: custody"):
                frontdoor.admit(["frontdoor", "run"], {}, 0, NOW)
        self.assertEqual(calls[0]["harness"], "provider")
        cwd.assert_not_called()


CHILD = {k: v for k, v in PROVIDER.items() if k != "env"}
CHILD = dict(CHILD, executable="/opt/oulipoly-providers/codex/bin/agent-runner-codex",
             settings=dict(PROVIDER["settings"], settings_id="codex-luna"), env={"CODEX_ACCOUNT_HINT": "site-choice"})


def child_site():
    value = site(**{"codex-sol": dict(PROVIDER, children=["luna-codex", "luna-max"])})
    value["child_routes"]["luna-codex"] = CHILD
    return value


class RegisteredChildRoutes(unittest.TestCase):
    def test_site_child_route_may_be_a_registered_provider(self):
        loaded(child_site())
        bad = child_site()
        bad["child_routes"]["luna-codex"] = dict(CHILD, children=["luna-max"])
        with self.assertRaisesRegex(frontdoor.Refused, "child route luna-codex"):
            loaded(bad)
        bad["child_routes"]["luna-codex"] = dict(CHILD, credential="required")
        with self.assertRaisesRegex(frontdoor.Refused, "child route luna-codex"):
            loaded(bad)

    def test_registered_child_takes_no_grant_and_reaches_the_entry_as_registered(self):
        value = loaded(child_site())
        checked = frontdoor.check_request(request(children={"routes": ["luna-codex"]}), value, NOW)
        self.assertIsNone(checked["children"]["grant"])
        with self.assertRaisesRegex(frontdoor.Refused, "take no credential"):
            frontdoor.check_request(request(children={"routes": ["luna-codex"]}, child_credential={}), value, NOW)
        user = pwd.getpwuid(os.getuid())
        entry = frontdoor.entry_request("/opt/pkg", "/runs/1/r", user, checked, {}, False)
        self.assertEqual(entry["provider"]["root_child_bin"], "/opt/pkg/bin/oulipoly-root-child")
        self.assertEqual(entry["children"], {
            "routes": {"luna-codex": {"registered": {
                "executable": CHILD["executable"],
                "settings": CHILD["settings"],
                "env": CHILD["env"],
                "agent_bash_bin": "/opt/pkg/agent-bash/agent-bash",
            }}},
            "max_starts": 4, "max_concurrent": 2,
        })
        # With an OpenCode child beside it, the OpenCode inputs come too.
        grant = {"openai": {"type": "oauth", "refresh": "", "access": "a", "expires": (NOW + 7200) * 1000}}
        mixed = frontdoor.check_request(request(children={"routes": ["luna-codex", "luna-max"]}, child_credential=grant), value, NOW)
        entry = frontdoor.entry_request("/opt/pkg", "/runs/1/r", user, mixed, {}, False)
        self.assertIn("opencode", entry["children"])
        self.assertEqual(entry["children"]["routes"]["luna-max"]["model"], "openai/gpt-6-luna")
        self.assertEqual(entry["provider"]["root_child_bin"], "/opt/pkg/bin/oulipoly-root-child")
        self.assertNotIn("root_child_bin", entry["children"]["routes"]["luna-codex"]["registered"])

    def test_admission_checks_child_executable_custody_before_any_effect(self):
        calls = []

        def custody(route):
            calls.append(route)
            if route is not None and route.get("settings", {}).get("settings_id") == "codex-luna":
                raise frontdoor.Refused("provider: child custody")

        asked = request(children={"routes": ["luna-codex"]})
        with mock.patch.object(frontdoor, "check_provider_executable", side_effect=custody), \
             mock.patch.object(frontdoor, "check_package"), \
             mock.patch.object(frontdoor, "load_site", return_value=loaded(child_site())), \
             mock.patch.object(frontdoor, "requester", return_value=pwd.getpwuid(os.getuid())), \
             mock.patch.object(frontdoor, "read_first_line", return_value=(json.dumps(asked).encode(), b"")), \
             mock.patch.object(frontdoor, "check_cwd_as") as cwd:
            with self.assertRaisesRegex(frontdoor.Refused, "child custody"):
                frontdoor.admit(["frontdoor", "run"], {}, 0, NOW)
        self.assertEqual([route["settings"]["settings_id"] for route in calls], ["codex-sol", "codex-luna"])
        cwd.assert_not_called()


class EntryRequest(unittest.TestCase):
    def test_provider_route_emits_the_provider_and_no_embedded_harness(self):
        user = pwd.getpwuid(os.getuid())
        checked = frontdoor.check_request(request(), SITE, NOW)
        value = frontdoor.entry_request("/opt/pkg", "/runs/1/r", user, checked, {"PATH": "/usr/bin"}, False)
        self.assertNotIn("opencode", value)
        self.assertNotIn("claude", value)
        self.assertEqual(value["provider"], {
            "executable": PROVIDER["executable"],
            "settings": PROVIDER["settings"],
            "env": PROVIDER["env"],
            "agent_bash_bin": "/opt/pkg/agent-bash/agent-bash",
            "bash_allow": ["git status"],
        })
        self.assertEqual(value["workload"], {"isolation": "host-root", "user": user.pw_name})
        trusted = frontdoor.check_request(request(bash={"authority": "trusted-task"}), site(**{"codex-sol": dict(PROVIDER, config_root="/etc/codex")}), NOW)
        value = frontdoor.entry_request("/opt/pkg", "/runs/1/r", user, trusted, {}, False)
        self.assertEqual(value["provider"]["bash_authority"], "trusted-task")
        self.assertEqual(value["provider"]["config_root"], "/etc/codex")

    def test_admitted_report_names_the_provider_not_a_model(self):
        report = frontdoor.harness_report(PROVIDER)
        self.assertEqual(report["harness"], "provider")
        self.assertIsNone(report["model"])
        self.assertEqual(report["provider"]["executable"], PROVIDER["executable"])
        self.assertEqual(len(report["provider"]["settings_sha256"]), 64)
        self.assertNotIn("gpt-6.1-sol", json.dumps(report))
        self.assertEqual(frontdoor.harness_report(SITE["routes"]["fixture"]), {"harness": "opencode", "model": "fixture/scripted"})


RUNNER = os.environ.get("OULIPOLY_RUNNER")

STAND_IN = r'''#!/usr/bin/python3
import hashlib, json, os, sys
op = sys.argv[1]
request = None if op == "resident.serve" else json.loads(sys.stdin.read())
with open(CALLS, "a") as f:
    f.write(op + "\n")
def answer(result):
    print(json.dumps({"contract": "oulipoly.provider/v1", "request_id": request["request_id"], "ok": True, "result": result}))
caps = {"launch": True, "policy": True, "quota": False, "session": False, "terminal": False, "rotation": False,
        "discovery": False, "settings": False, "setup_brain": False, "setup": False, "migration": False}
if RESIDENT and request is not None and request["host"].get("env", {}).get("OULIPOLY_HOST_RESIDENT_SESSION_V1") == "1":
    caps["resident_session_v1"] = True
mediation_env = request and request["params"].get("launch", {}).get("env", {}).get("OULIPOLY_TOOL_MEDIATION_V1")
mediation = json.loads(mediation_env) if mediation_env else None
marker = dict(mediation or {})
marker.pop("requester", None)
marker.update(tool="fake_mediated_bash", native_tools=["fake_mediated_bash"])
markers = [{"name": "oulipoly.tool_mediation/v1", "value": marker}]
if request and request["host"].get("env", {}).get("OULIPOLY_HOST_TOOL_MEDIATION_V1") == "1":
    caps["tool_mediation_v1"] = True
if op == "describe":
    answer({"provider_id": "stand-in-external", "display_name": "Stand-in", "contract_versions": ["oulipoly.provider/v1"],
            "preferred_contract": "oulipoly.provider/v1", "capabilities": caps})
elif op == "policy.evaluate":
    answer({"accepted": True, "argv": ["native"], "env": {"OULIPOLY_TOOL_MEDIATION_V1": mediation_env}, "stdin": None, "prompt": None, "diagnostics": [], "markers": markers})
elif op == "resident.prepare":
    data = request["host"]["data_root"]
    config = json.dumps(request["params"]["launch"], sort_keys=True).encode()
    digest = hashlib.sha256(config).hexdigest()
    path = os.path.join(data, digest + ".json")
    with open(path, "wb") as f:
        f.write(config)
    answer({"protocol": "oulipoly.resident_session/v1", "invocation": {"args": ["resident.serve", "--config", path], "endpoint": "stdio"},
            "acp": {"protocol_version": 2, "schema": "schema-v2.0.0-alpha.7", "dedup_contract": 1}, "config_sha256": digest,
            "operations": ["initialize", "session/new", "session/prompt", "session/cancel", "session/close"]})
elif op == "resident.serve":
    path = sys.argv[sys.argv.index("--config") + 1]
    os.execv(PEER, [PEER, "--state", os.path.join(os.path.dirname(path), "peer.json")])
'''


def stop(entry):
    """Ends an entry still running after a failure through its own cancel, so
    the owner settles its root before tearDown removes the store; a kill is
    the last resort and can leave the root's harness running."""
    if entry.poll() is not None:
        return
    try:
        entry.stdin.write(b'{"cmd":"cancel"}\n')
        entry.stdin.flush()
        entry.wait(timeout=30)
    except (BrokenPipeError, subprocess.TimeoutExpired):
        entry.kill()
        entry.wait()


@unittest.skipUnless(RUNNER, "set OULIPOLY_RUNNER to a built oulipoly-agent-runner with the root supervisor binaries and deterministic peer beside it")
class EntryContract(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.package = os.path.join(self.dir, "pkg")
        os.makedirs(os.path.join(self.package, "agent-bash"))
        bash = os.path.join(self.package, frontdoor.BASH_BIN)
        with open(bash, "w") as file:
            file.write("#!/bin/sh\nexit 0\n")
        os.chmod(bash, 0o755)
        self.run_dir = os.path.join(self.dir, "run")
        os.mkdir(self.run_dir)
        self.pending = b""

    def tearDown(self):
        subprocess.run(["rm", "-rf", "--", self.dir], check=True)

    def entry(self, resident):
        executable = os.path.join(self.dir, "provider")
        peer = os.path.join(os.path.dirname(RUNNER), "oulipoly-acp-deterministic-peer")
        with open(executable, "w") as file:
            file.write(STAND_IN.replace("op = sys.argv[1]", "CALLS = %r\nPEER = %r\nRESIDENT = %r\nop = sys.argv[1]" % (
                os.path.join(self.dir, "calls"), peer, resident)))
        os.chmod(executable, 0o755)
        route = dict(PROVIDER, executable=executable)
        checked = frontdoor.check_request(request(message="echo:through the front door", cwd=self.dir), site(**{"codex-sol": route}), NOW)
        user = pwd.getpwuid(os.getuid())
        value = frontdoor.entry_request(self.package, self.run_dir, user, checked, {"PATH": "/usr/bin:/bin"}, False)
        self.assertEqual(value["workload"]["isolation"], "host-root")
        # A host-root declaration needs a root entry (ROOT's run).
        value["workload"] = {"isolation": "unprivileged-userns"}
        path = os.path.join(self.dir, "request.json")
        with open(path, "w") as file:
            json.dump(value, file)
        return subprocess.Popen([RUNNER, "native-root", "--request", path], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, env={"PATH": "/usr/bin:/bin"}, cwd="/")

    def lines_until(self, entry, predicate, seen):
        # The descriptor is read directly: a buffered readline after select
        # can strand complete lines in the buffer while select waits.
        end = time.monotonic() + 60
        fd = entry.stdout.fileno()
        while time.monotonic() < end:
            while b"\n" in self.pending:
                line, self.pending = self.pending.split(b"\n", 1)
                value = json.loads(line)
                seen.append(value)
                if predicate(value):
                    return value
            if not select.select([fd], [], [], 1)[0]:
                continue
            chunk = os.read(fd, 65536)
            if not chunk:
                return None
            self.pending += chunk
        self.fail(f"watchdog: {seen}")

    def calls(self):
        with open(os.path.join(self.dir, "calls")) as file:
            return file.read().split()

    def test_front_door_request_reaches_the_owner_through_the_provider(self):
        entry = self.entry(resident=True)
        seen = []
        try:
            turn = self.lines_until(entry, lambda v: v.get("event") == "turn-end", seen)
            self.assertIsNotNone(turn, seen)
            self.assertEqual(turn["harness"], "stand-in-external")
            entry.stdin.write(b'{"cmd":"close"}\n')
            entry.stdin.flush()
            self.lines_until(entry, lambda v: v.get("entry") == "terminal", seen)
            self.assertEqual(entry.wait(timeout=60), 87, seen)
        finally:
            stop(entry)
        setup = next(v for v in seen if v.get("entry") == "setup-completed")
        self.assertEqual(setup["harness"], "registered-provider")
        self.assertEqual(setup["launch"]["tools"]["bash"], {"allow": ["git status"]})
        self.assertEqual(setup["launch"]["tools"]["requester"], os.path.join(self.package, frontdoor.BASH_BIN))
        reply = next(v for v in seen if v.get("event") == "agent-message")
        self.assertEqual(reply["text"], "through the front door")
        self.assertEqual(self.calls(), ["describe", "policy.evaluate", "resident.prepare", "resident.serve"])

    def test_provider_without_the_resident_capability_is_refused_after_it_ran(self):
        entry = self.entry(resident=False)
        seen = []
        try:
            terminal = self.lines_until(entry, lambda v: v.get("entry") == "terminal", seen)
            self.assertEqual(entry.wait(timeout=60), 65, seen)
        finally:
            stop(entry)
        self.assertEqual(terminal["stage"], "provider-refused")
        self.assertEqual(terminal["effects"], {"runner_setup": "none", "provider": "unknown: it ran"})
        self.assertEqual(self.calls(), ["describe"])
        self.assertFalse(os.path.exists(os.path.join(self.run_dir, "launch")))


if __name__ == "__main__":
    unittest.main()
