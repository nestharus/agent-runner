"""Disposable shared-front-door fixtures; no installed alias, State, or broker."""

import json
import os
import shutil
import socket
import subprocess
import tempfile
import threading
import unittest
import uuid
from pathlib import Path

from shared_front_door import adopt_alias, check, prepare, publish, read_active
from stage_versioned_island import ASSETS, stage


class BrokerFixture:
    def __init__(self, path: Path, route: dict[str, str], calls: int = 1):
        self.path = path
        self.route = route
        self.calls = calls
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.error = None

    def __enter__(self):
        self.ready = threading.Event()
        self.thread.start()
        if not self.ready.wait(5):
            raise RuntimeError("broker fixture did not bind")
        return self

    def __exit__(self, *_):
        self.thread.join(5)
        if self.thread.is_alive():
            raise RuntimeError("broker fixture did not finish")
        if self.error:
            raise self.error

    def serve(self):
        try:
            self.path.unlink(missing_ok=True)
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                listener.bind(str(self.path))
                listener.listen()
                self.ready.set()
                for _ in range(self.calls):
                    conn, _ = listener.accept()
                    with conn:
                        conn.sendall(b"1" * 16)
                        request = conn.recv(17, socket.MSG_WAITALL)
                        if request != b"I" + b"1" * 16:
                            raise AssertionError("wrong broker readback request")
                        conn.sendall(("fresh-v30-route " + " ".join(self.route.values()) + "\n").encode())
        except Exception as error:
            self.error = error
            self.ready.set()


class SharedFrontDoorTests(unittest.TestCase):
    def setUp(self):
        target = Path(__file__).resolve().parents[2] / "target" / "age319-front-door-tests"
        target.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=target)
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name)
        self.stage = self.work / "stage"
        self.root = self.work / "front-door-v2"
        self.socket = Path(__file__).resolve().parents[2] / "src-tauri/target" / f"b-{uuid.uuid4().hex[:8]}.sock"
        self.addCleanup(lambda: self.socket.unlink(missing_ok=True))
        self.route = dict(zip(("lane_id", "source_generation", "domain_id"),
                              (str(uuid.uuid4()) for _ in range(3))))
        self.home = self.work / "home" / "local-user"
        self.alias_dir = self.home / ".local" / "bin"
        self.alias_dir.mkdir(parents=True)
        self.no_gui = self._testMethodName.startswith("test_user_local_")
        self.aliases = {"runner": self.alias_dir / "agents",
                        "runner_alt": self.alias_dir / "oulipoly-agent-runner",
                        "gui": None if self.no_gui else self.alias_dir / "oulipoly-plane",
                        "bash": self.alias_dir / "agent-bash"}
        old_runner_dir = self.work / "old-runner"
        old_bash_dir = self.work / "old-bash"
        old_runner_dir.mkdir()
        old_bash_dir.mkdir()
        self.old_runner = old_runner_dir / "oulipoly-agent-runner"
        self.old_gui = old_runner_dir / "oulipoly-installed-launcher"
        self.old_bash = old_bash_dir / "agent-bash"
        for path, label in ((self.old_runner, "OLD_RUNNER"), (self.old_gui, "OLD_GUI"),
                            (self.old_bash, "OLD_BASH")):
            subprocess.run(["cc", "-x", "c", "-", "-o", str(path)], input=(
                '#include <stdio.h>\nint main(int argc,char**argv){printf("' + label + '\\n");return 0;}\n'
            ), text=True, check=True, capture_output=True)
        old_runner_config = old_runner_dir / "config.toml"
        old_bash_config = old_bash_dir / "agent-bash.toml"
        old_runner_config.write_text(f'data_dir = "{self.work / "old-state"}"\nconfig_home = "{self.work / "old-config"}"\n')
        old_bash_config.write_text(f'state_root = "{self.work / "old-handles"}"\nagent_runner_bin = "{self.old_runner}"\n')
        self.sources = {}
        for name in ASSETS:
            path = self.work / f"source-{name}"
            path.write_bytes(f"fixture {name}\n".encode())
            self.sources[name] = path
        self.sources["legacy_runner"] = self.old_runner
        self.sources["legacy_bash"] = self.old_bash
        self.sources["legacy_runner_config"] = old_runner_config
        self.sources["legacy_bash_config"] = old_bash_config
        self.sources["fresh_runner_config"].write_text(
            f'data_dir = "{self.work / "fresh-state"}"\nconfig_home = "{self.work / "fresh-config"}"\n')
        self.sources["fresh_bash_config"].write_text(
            f'state_root = "{self.work / "fresh-handles"}"\n'
            f'agent_runner_bin = "{self.stage / ASSETS["fresh_runner"][0]}"\n')
        self.staged = stage(self.stage, self.sources)
        launcher = Path(__file__).resolve().parents[2] / "src-tauri/target/debug/oulipoly-shared-front-door"
        if not launcher.exists():
            self.fail("build oulipoly-shared-front-door fixture binary first")
        for role, alias in self.aliases.items():
            if alias is None:
                continue
            old = {"runner": self.old_runner, "runner_alt": self.old_runner,
                   "gui": self.old_gui, "bash": self.old_bash}[role]
            alias.symlink_to(os.path.relpath(old, alias.parent) if role == "gui" else old)
        prepare(self.root, self.stage, launcher, self.aliases, self.sources["fresh_broker"],
                self.socket, self.route,
                adapter_bindings={"runner": self.aliases["runner"], "bash": self.aliases["bash"]})

    def invoke(self, role, *args):
        return subprocess.run([str(self.aliases[role]), *args], capture_output=True, text=True)

    def adopt_all(self):
        for role in ("runner", "runner_alt", "gui", "bash"):
            adopt_alias(self.root, role, {"runner": self.old_runner, "runner_alt": self.old_runner,
                                          "gui": self.old_gui, "bash": self.old_bash}[role])

    def test_transition_late_old_and_closed_new_with_old_handle(self):
        legacy, digest = publish(self.root, self.staged["generation"], "legacy", None, str(uuid.uuid4()))
        self.assertEqual(legacy["epoch"], 0)
        adopt_alias(self.root, "runner", self.old_runner)
        self.assertEqual(self.invoke("runner", "--version").stdout, "OLD_RUNNER\n")
        self.assertEqual(self.invoke("runner_alt", "--version").stdout, "OLD_RUNNER\n")  # not adopted
        self.assertEqual(self.invoke("bash", "run").stdout, "OLD_BASH\n")  # not yet adopted
        adopt_alias(self.root, "gui", self.old_gui)
        adopt_alias(self.root, "runner_alt", self.old_runner)
        adopt_alias(self.root, "bash", self.old_bash)
        self.assertEqual(self.invoke("gui").stdout, "OLD_GUI\n")
        self.assertEqual(self.invoke("bash", "run").stdout, "OLD_BASH\n")  # selected epoch zero
        old_handle = self.work / "old-handles" / "ab_old_1"
        old_handle.mkdir(parents=True)
        (old_handle / "meta.json").write_text('{"handle":"ab_old_1"}\n')
        publication = str(uuid.uuid4())
        with BrokerFixture(self.socket, self.route, calls=6):
            fresh, fresh_digest = publish(self.root, self.staged["generation"], "fresh-closed", digest, publication)
            self.assertEqual(fresh["epoch"], 1)
            self.assertEqual(publish(self.root, self.staged["generation"], "fresh-closed", digest, publication), (fresh, fresh_digest))
            self.assertIn("fresh effect closed", self.invoke("runner", "--version").stderr)
            self.assertIn("fresh effect closed", self.invoke("runner_alt", "--version").stderr)
            self.assertIn("fresh effect closed", self.invoke("gui").stderr)
            self.assertIn("fresh effect closed", self.invoke("bash", "run").stderr)
            self.assertEqual(self.invoke("bash", "status", "ab_old_1").stdout, "OLD_BASH\n")
            self.assertEqual(self.invoke("bash", "snapshot", "ab_old_1", "--bytes", "10").stdout, "OLD_BASH\n")
            self.assertIn("fresh effect closed", self.invoke("bash", "status", "ab_unknown").stderr)
        with self.assertRaisesRegex(ValueError, "rollback"):
            publish(self.root, self.staged["generation"], "legacy", fresh_digest, str(uuid.uuid4()))

    def test_missing_partial_selector_wrong_image_and_broker_restart(self):
        self.adopt_all_after_legacy()
        (self.root / "active.json").unlink()
        self.assertIn("No such file", self.invoke("runner").stderr)
        self.assertFalse((self.work / "fresh-state").exists())
        self.assertFalse((self.work / "old-handles").exists())
        (self.root / "active.json").write_text("{\n")
        self.assertNotEqual(self.invoke("bash", "run").returncode, 0)
        (self.root / "active.json").unlink()
        _, digest = publish(self.root, self.staged["generation"], "legacy", None, str(uuid.uuid4()))
        with BrokerFixture(self.socket, self.route):
            publish(self.root, self.staged["generation"], "fresh-closed", digest, str(uuid.uuid4()))
        changed = self.root / "images/fresh/runner/oulipoly-agent-runner"
        changed.chmod(0o600)
        changed.write_bytes(b"wrong image")
        changed.chmod(0o444)
        self.assertIn("image mismatch", self.invoke("runner").stderr)
        changed.chmod(0o600)
        shutil.copyfile(self.stage / ASSETS["fresh_runner"][0], changed)
        changed.chmod(0o444)
        broker_image = self.sources["fresh_broker"]
        original_broker = broker_image.read_bytes()
        broker_image.write_bytes(b"different broker image")
        self.assertIn("image mismatch", self.invoke("runner").stderr)
        broker_image.write_bytes(original_broker)
        with BrokerFixture(self.socket, dict(self.route, domain_id=str(uuid.uuid4()))):
            self.assertIn("broker generation incompatible", self.invoke("runner").stderr)
        active_path = self.root / "active.json"
        original = active_path.read_bytes()
        selector = json.loads(original)
        selector["manifest_sha256"] = "0" * 64
        active_path.chmod(0o600)
        active_path.write_text(json.dumps(selector))
        active_path.chmod(0o444)
        self.assertIn("generation manifest mismatch", self.invoke("runner").stderr)
        active_path.chmod(0o600)
        active_path.write_bytes(original)
        active_path.chmod(0o444)
        self.assertIn("Connection refused", self.invoke("runner").stderr)

    def adopt_all_after_legacy(self):
        publish(self.root, self.staged["generation"], "legacy", None, str(uuid.uuid4()))
        self.adopt_all()

    def test_concurrent_publication_one_winner_and_no_replay(self):
        self.adopt_all_after_legacy()
        _, digest = read_active(self.root)
        results = []
        def attempt():
            try:
                results.append(publish(self.root, self.staged["generation"], "fresh-closed", digest, str(uuid.uuid4())))
            except ValueError as error:
                results.append(str(error))
        with BrokerFixture(self.socket, self.route):
            first = threading.Thread(target=attempt)
            second = threading.Thread(target=attempt)
            first.start(); second.start(); first.join(); second.join()
        self.assertEqual(sum(isinstance(item, tuple) for item in results), 1)
        self.assertEqual(read_active(self.root)[0]["epoch"], 1)

    def test_user_local_optional_gui_relative_exec_and_alias_readback(self):
        self.assertIsNone(self.aliases["gui"])
        _, digest = publish(self.root, self.staged["generation"], "legacy", None, str(uuid.uuid4()))
        adopt_alias(self.root, "runner", self.old_runner)
        relative = subprocess.run([".local/bin/agents"], cwd=self.home, capture_output=True, text=True)
        self.assertEqual(relative.stdout, "OLD_RUNNER\n")
        via_path = subprocess.run(["agents"], env={"PATH": str(self.alias_dir)},
                                  capture_output=True, text=True)
        self.assertEqual(via_path.stdout, "OLD_RUNNER\n")
        self.assertEqual(self.invoke("runner_alt").stdout, "OLD_RUNNER\n")
        adopt_alias(self.root, "runner_alt", self.old_runner)
        adopt_alias(self.root, "bash", self.old_bash)
        self.assertEqual(self.invoke("bash", "run").stdout, "OLD_BASH\n")
        self.assertEqual(check(self.root, self.staged["generation"])["aliases"],
                         {"runner": "adopted", "runner_alt": "adopted",
                          "gui": "absent", "bash": "adopted"})
        self.aliases["bash"].unlink()
        with self.assertRaises(FileNotFoundError):
            check(self.root, self.staged["generation"])
        with self.assertRaises(FileNotFoundError):
            publish(self.root, self.staged["generation"], "fresh-closed", digest, str(uuid.uuid4()))
        self.aliases["bash"].symlink_to(self.old_bash)
        adopt_alias(self.root, "bash", self.old_bash)
        with BrokerFixture(self.socket, self.route, calls=4):
            publish(self.root, self.staged["generation"], "fresh-closed", digest, str(uuid.uuid4()))
            self.assertIn("fresh effect closed", self.invoke("runner").stderr)
            self.assertIn("fresh effect closed", self.invoke("runner_alt").stderr)
            self.assertIn("fresh effect closed", self.invoke("bash", "run").stderr)
        # Local-owner alteration is detected on readback and on invocation through
        # an altered link. A direct old image remains explicitly outside mediation.
        self.aliases["bash"].unlink()
        self.aliases["bash"].symlink_to(self.old_bash)
        with self.assertRaisesRegex(ValueError, "activated selector has legacy alias"):
            check(self.root, self.staged["generation"])
        self.assertEqual(subprocess.run([str(self.old_bash)], capture_output=True, text=True).stdout,
                         "OLD_BASH\n")

    def test_adapter_override_cannot_be_declared_as_managed_alias(self):
        launcher = Path(__file__).resolve().parents[2] / "src-tauri/target/debug/oulipoly-shared-front-door"
        with self.assertRaisesRegex(ValueError, "adapter entries must be"):
            prepare(self.work / "other-front-door", self.stage, launcher, self.aliases,
                    self.sources["fresh_broker"], self.socket, self.route,
                    adapter_bindings={"runner": self.old_runner, "bash": self.aliases["bash"]})


if __name__ == "__main__":
    unittest.main()
