"""Finite source-construction controls, not a fresh package or host-root proof.

Build commands are recorded; staging products are authored fixture bytes.
No dependency install, native CLI, privilege change or activation occurs.
"""

import json
import os
import tempfile
import unittest
from unittest import mock

import build_package
import frontdoor
import install_package
from test_provider_route import NOW, child_site, loaded, request


class Requester(unittest.TestCase):
    def test_builder_builds_stages_and_manifests_requester(self):
        with tempfile.TemporaryDirectory(prefix="u110-linux-requester-") as root:
            calls = []
            with mock.patch.object(build_package, "run", side_effect=lambda argv, *a, **kw: calls.append(argv)):
                build_package.build_binaries(root, "/runner", "/bash-source", "/log")
            self.assertIn("oulipoly-root-child", calls[0])
            self.assertIn("oulipoly-root-bash", calls[0])
            self.assertIn("--locked", calls[0])
            self.assertIn("--release", calls[0])
            for target, product in build_package.BINARIES:
                path = os.path.join(root, product)
                os.makedirs(os.path.dirname(path), exist_ok=True)
                with open(path, "wb") as f:
                    f.write(product.encode())
                os.chmod(path, 0o755)
            stage = os.path.join(root, "specimen")
            build_package.stage_binaries(root, stage)
            files = build_package.manifest_files(stage)
            reader = files["bin/oulipoly-root-bash"]
            self.assertEqual(reader["mode"], "0o755")
            self.assertEqual(reader["sha256"], build_package.sha256(os.path.join(root, "target/release/oulipoly-root-bash")))
            entry = files["bin/oulipoly-root-child"]
            self.assertEqual(entry["mode"], "0o755")
            self.assertEqual(entry["sha256"], build_package.sha256(os.path.join(root, "target/release/oulipoly-root-child")))
            with open(os.path.join(stage, "MANIFEST.json"), "w") as f:
                json.dump({"files": files}, f)
            install_package.verify(stage)
            # Custody identifies changed staged bytes even if executable.
            with open(os.path.join(stage, frontdoor.ROOT_CHILD), "ab") as f:
                f.write(b"changed")
            with self.assertRaisesRegex(install_package.Stop, "root-child: digest"):
                install_package.verify(stage)

    def test_staging_refuses_missing_and_nonexecutable_requester(self):
        with tempfile.TemporaryDirectory(prefix="u110-linux-requester-") as root:
            for target, product in build_package.BINARIES:
                path = os.path.join(root, product)
                os.makedirs(os.path.dirname(path), exist_ok=True)
                with open(path, "wb") as f:
                    f.write(b"fixture")
                os.chmod(path, 0o755)
            requester = os.path.join(root, "target/release/oulipoly-root-child")
            os.chmod(requester, 0o644)
            with self.assertRaisesRegex(SystemExit, "no executable.*root-child"):
                build_package.stage_binaries(root, os.path.join(root, "specimen"))
            os.unlink(requester)
            with self.assertRaisesRegex(SystemExit, "no executable.*root-child"):
                build_package.stage_binaries(root, os.path.join(root, "specimen"))

    def test_frontdoor_checks_requester_before_effects(self):
        with tempfile.TemporaryDirectory(prefix="u110-linux-requester-") as root:
            for rel in (frontdoor.RUNNER, frontdoor.SUPERVISOR, frontdoor.PID1,
                        frontdoor.ROOT_CHILD, frontdoor.BASH_BIN):
                path = os.path.join(root, rel)
                os.makedirs(os.path.dirname(path), exist_ok=True)
                with open(path, "w") as f:
                    f.write("fixture")
                os.chmod(path, 0o755)
            # Only ancestry is bypassed for this user-owned scratch; tree
            # ownership and writable-mode checks execute with the test UID.
            with mock.patch.object(frontdoor, "check_owned"), \
                 mock.patch.object(frontdoor, "TRUSTED_OWNERS", {os.getuid()}):
                frontdoor.check_package(root)
                path = os.path.join(root, frontdoor.ROOT_CHILD)
                for mode, reason in ((0o775, "writable"), (0o644, "no executable.*root-child")):
                    os.chmod(path, mode)
                    with self.assertRaisesRegex(frontdoor.Refused, reason):
                        frontdoor.check_package(root)
                os.unlink(path)
                with mock.patch.object(frontdoor, "package_root", return_value=root), \
                     mock.patch.object(frontdoor, "make_run") as allocate, \
                     mock.patch.object(frontdoor, "emit") as emit:
                    self.assertEqual(frontdoor.run_locked(["frontdoor", "run"], {}), 90)
                    allocate.assert_not_called()
                    self.assertEqual(emit.call_args.args[0]["effects"], "none")
                    self.assertIn("root-child", emit.call_args.args[0]["reason"])

    def test_requester_exactly_with_selected_routes(self):
        import pwd
        user = pwd.getpwuid(os.getuid())
        site = loaded(child_site())
        for routes in (None, ["luna-codex"]):
            asked = request() if routes is None else request(children={"routes": routes})
            checked = frontdoor.check_request(asked, site, NOW)
            entry = frontdoor.entry_request("/opt/caller-selected-package", "/run/r", user, checked, {})
            if routes is None:
                self.assertNotIn("root_child_bin", entry["provider"])
                self.assertNotIn("children", entry)
            else:
                self.assertEqual(entry["provider"]["root_child_bin"],
                                 "/opt/caller-selected-package/bin/oulipoly-root-child")
                self.assertEqual(list(entry["children"]["routes"]), routes)
                self.assertNotIn("root_child_bin", entry["children"]["routes"][routes[0]]["registered"])


if __name__ == "__main__":
    unittest.main()
