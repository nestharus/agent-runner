"""Offline checks of the package archive and the installer, on a small
stand-in stage (no cargo, npm, root or sudoers effect): deterministic
archive, manifest verification, refused archive shapes and prefixes, the
rendered sudoers rule, the site config, and record-driven uninstall into a
scratch destination root."""

import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

import build_package
import install_package

HERE = os.path.dirname(os.path.realpath(__file__))
PACKAGE_ID = "oulipoly-native-linux-x86_64-aaaaaaaaaaaa-bbbbbbbbbbbb"


class Scratch(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="build-install-test-")
        self.addCleanup(shutil.rmtree, self.dir, True)

    def stage(self):
        stage = os.path.join(self.dir, "stage", PACKAGE_ID)
        for rel, data, mode in (
            ("bin/oulipoly-agent-runner", b"runner", 0o755),
            ("bin/oulipoly-root-supervisor", b"owner", 0o755),
            ("bin/oulipoly-root-pid1", b"pid1", 0o755),
            ("agent-bash/agent-bash", b"bash", 0o755),
            ("agent-bash/bash.ts", b"tool", 0o644),
            ("opencode/deps/package-lock.json", b"{}", 0o664),
            ("opencode/deps/node_modules/x/cli.js", b"x", 0o775),
        ):
            path = os.path.join(stage, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "wb") as file:
                file.write(data)
            os.chmod(path, mode)
        os.makedirs(os.path.join(stage, "opencode/deps/node_modules/.bin"))
        os.symlink("../x/cli.js", os.path.join(stage, "opencode/deps/node_modules/.bin/x"))
        for target, source, mode in build_package.ASSETS:
            path = os.path.join(stage, target)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            shutil.copyfile(os.path.join(HERE, source), path)
            os.chmod(path, mode)
        build_package.normalize(stage)
        manifest = {"format": 1, "id": PACKAGE_ID, "runner": {"commit": "a" * 40}, "agent_bash": {"commit": "b" * 40},
                    "files": build_package.manifest_files(stage)}
        with open(os.path.join(stage, "MANIFEST.json"), "w") as file:
            json.dump(manifest, file)
        os.chmod(os.path.join(stage, "MANIFEST.json"), 0o644)
        return stage

    def archive(self, stage, name="a.tar.gz"):
        out = os.path.join(self.dir, name)
        build_package.write_archive(stage, PACKAGE_ID, out, 1_700_000_000)
        with open(out, "rb") as file:
            return out, hashlib.sha256(file.read()).hexdigest()

    def install(self, archive, digest, *extra, command="install"):
        dest = os.path.join(self.dir, "root")
        os.makedirs(os.path.join(dest, "etc", "sudoers.d"), exist_ok=True)
        argv = [command, "--archive", archive, "--sha256", digest, "--user", "nes",
                "--dest-root", dest, "--unprivileged-test", *extra]
        return install_package.main(argv), dest


class Archive(Scratch):
    def test_deterministic_and_normalized(self):
        stage = self.stage()
        first, digest = self.archive(stage, "1.tar.gz")
        os.utime(os.path.join(stage, "bin/oulipoly-agent-runner"), (1, 1))
        second, again = self.archive(stage, "2.tar.gz")
        self.assertEqual(digest, again)
        with tarfile.open(first) as tar:
            members = tar.getmembers()
            self.assertEqual(members[0].name, PACKAGE_ID)
            self.assertTrue(all(m.uid == 0 and m.gid == 0 and m.mtime == 1_700_000_000 for m in members))
            modes = {m.name.split("/", 1)[-1]: m.mode for m in members}
            self.assertEqual(modes["opencode/deps/package-lock.json"], 0o644)
            self.assertEqual(modes["opencode/deps/node_modules/x/cli.js"], 0o755)
            self.assertEqual(modes["libexec/oulipoly-native-frontdoor"], 0o755)
            self.assertEqual(sorted(m.name for m in members), [m.name for m in members])


class Install(Scratch):
    def test_install_verify_render_and_uninstall(self):
        archive, digest = self.archive(self.stage())
        code, dest = self.install(archive, digest)
        self.assertEqual(code, 0)
        package = os.path.join(dest, "opt/oulipoly-native", PACKAGE_ID)
        self.assertTrue(os.path.isfile(os.path.join(package, "libexec/oulipoly-native-frontdoor")))
        self.assertEqual(os.readlink(os.path.join(package, "opencode/deps/node_modules/.bin/x")), "../x/cli.js")
        with open(os.path.join(dest, "etc/sudoers.d/oulipoly-native")) as file:
            rule = file.read()
        frontdoor = f"/opt/oulipoly-native/{PACKAGE_ID}/libexec/oulipoly-native-frontdoor"
        self.assertIn(f"nes ALL=(root) NOPASSWD: {frontdoor} run\n", rule)
        self.assertIn(f"Defaults!{frontdoor} !use_pty", rule)
        self.assertNotIn("@", rule.replace("# ", "").split("Defaults", 1)[1])
        self.assertEqual(oct(os.stat(os.path.join(dest, "etc/sudoers.d/oulipoly-native")).st_mode & 0o777), "0o440")
        with open(os.path.join(dest, "etc/oulipoly-native/frontdoor.json")) as file:
            site = json.load(file)
        self.assertEqual(site["allowed_users"], ["nes"])
        self.assertEqual(site["run_base"], "/var/lib/oulipoly-native/runs")
        self.assertEqual(oct(os.stat(os.path.join(dest, "var/lib/oulipoly-native/runs")).st_mode & 0o777), "0o711")
        record = f"/opt/oulipoly-native/{PACKAGE_ID}.install.json"
        # A second install of the same package is refused.
        self.assertEqual(self.install(archive, digest)[0], 3)
        code = install_package.main(["uninstall", "--record", record, "--dest-root", dest, "--unprivileged-test", "--purge-site"])
        self.assertEqual(code, 0)
        left = sorted(os.path.relpath(os.path.join(d, n), dest) for d, ds, fs in os.walk(dest) for n in ds + fs)
        self.assertEqual(left, ["etc", "etc/sudoers.d"])

    def test_plan_needs_no_privilege(self):
        archive, digest = self.archive(self.stage())
        self.assertEqual(install_package.main(["plan", "--archive", archive, "--sha256", digest, "--user", "nes"]), 0)

    def test_plan_changes_nothing(self):
        archive, digest = self.archive(self.stage())
        code, dest = self.install(archive, digest, command="plan")
        self.assertEqual(code, 0)
        self.assertEqual([os.path.relpath(os.path.join(d, n), dest) for d, ds, fs in os.walk(dest) for n in ds + fs],
                         ["etc", "etc/sudoers.d"])

    def test_refusals(self):
        archive, digest = self.archive(self.stage())
        self.assertEqual(self.install(archive, "0" * 64)[0], 3)
        self.assertEqual(self.install(archive, digest, "--prefix", "/usr/local/libexec/oulipoly")[0], 3)
        self.assertEqual(self.install(archive, digest, "--prefix", "/usr/local/libexec/oulipoly/x")[0], 3)
        self.assertEqual(install_package.main(["install", "--archive", archive, "--sha256", digest, "--user", "root",
                                               "--dest-root", self.dir, "--unprivileged-test"]), 3)

    def test_changed_file_fails_manifest(self):
        stage = self.stage()
        with open(os.path.join(stage, "agent-bash/bash.ts"), "w") as file:
            file.write("changed after the manifest")
        archive, digest = self.archive(stage)
        code, dest = self.install(archive, digest)
        self.assertEqual(code, 3)
        self.assertFalse(os.path.exists(os.path.join(dest, "opt/oulipoly-native", PACKAGE_ID)))

    def test_escaping_members_refused(self):
        for name, kind, target in (("../evil", tarfile.REGTYPE, ""), (PACKAGE_ID + "/l", tarfile.SYMTYPE, "/etc/shadow"),
                                   (PACKAGE_ID + "/l", tarfile.SYMTYPE, "../../x"), (PACKAGE_ID + "/d", tarfile.CHRTYPE, "")):
            with self.subTest(name=name, target=target):
                buffer = io.BytesIO()
                with tarfile.open(fileobj=buffer, mode="w") as tar:
                    root = tarfile.TarInfo(PACKAGE_ID)
                    root.type = tarfile.DIRTYPE
                    tar.addfile(root)
                    info = tarfile.TarInfo(name)
                    info.type = kind
                    info.linkname = target
                    tar.addfile(info, io.BytesIO(b"") if kind == tarfile.REGTYPE else None)
                buffer.seek(0)
                with tarfile.open(fileobj=buffer) as tar, self.assertRaises(install_package.Stop):
                    install_package.members(tar, PACKAGE_ID)


if __name__ == "__main__":
    unittest.main()
