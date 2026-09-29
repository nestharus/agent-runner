"""Disposable filesystem proof for the fixed-path first host installer."""

import io
import os
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from first_install_v30 import IMAGES, PREFIX, build
from install_first_host_v30 import check, install, preflight


class FirstHostInstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.images = {}
        for role in IMAGES:
            image = self.root / role
            image.write_bytes(("image-" + role).encode())
            self.images[role] = image
        self.service = Path(__file__).with_name("oulipoly-kernel-broker.service")
        self.package = self.root / "first.tar.gz"
        build(self.images, self.service, self.package, "0.1.0")

    def test_fixture_install_modes_bytes_and_readback(self):
        fixture = self.root / "fixture"
        fixture.mkdir()
        manifest = install(self.package, fixture)
        self.assertEqual(check(self.package, fixture), manifest)
        image_dir = fixture / "usr/local/libexec/oulipoly"
        for name in IMAGES.values():
            item = image_dir / name
            self.assertEqual(stat.S_IMODE(item.stat().st_mode), 0o555)
            self.assertEqual(item.read_bytes(), self.images[next(
                role for role, member in IMAGES.items() if member == name)].read_bytes())
        self.assertEqual(stat.S_IMODE((image_dir / "install-v1.json").stat().st_mode), 0o444)
        unit = fixture / "etc/systemd/system/oulipoly-kernel-broker.service"
        self.assertEqual(stat.S_IMODE(unit.stat().st_mode), 0o444)
        self.assertEqual(unit.read_bytes(), self.service.read_bytes())
        self.assertFalse((fixture / "var/lib/oulipoly-kernel-broker").exists())
        self.assertFalse((fixture / "usr/local/bin/agents").exists())
        with self.assertRaises(FileExistsError):
            install(self.package, fixture)
        self.images["runner"].write_bytes(b"different runner")
        wrong = self.root / "wrong.tar.gz"
        build(self.images, self.service, wrong, "0.1.0")
        with self.assertRaises(ValueError):
            check(wrong, fixture)
        item = image_dir / IMAGES["bash"]
        item.chmod(0o755)
        with self.assertRaisesRegex(ValueError, "installed file changed"):
            check(self.package, fixture)
        item.chmod(0o555)
        unit.chmod(0o644)
        with self.assertRaisesRegex(ValueError, "installed service changed"):
            check(self.package, fixture)

    def test_wrong_and_tampered_package_refuse_before_publication(self):
        fixture = self.root / "fixture"
        fixture.mkdir()
        wrong = self.root / "wrong.tar.gz"
        build(self.images, self.root / "runner", wrong, "0.1.0")
        with self.assertRaisesRegex(ValueError, "exact installed Broker unit"):
            install(wrong, fixture)
        tampered = self.root / "tampered.tar.gz"
        with tarfile.open(self.package) as source, tarfile.open(tampered, "w:gz") as output:
            for entry in source.getmembers():
                body = source.extractfile(entry).read()
                if entry.name == PREFIX + IMAGES["runner"]:
                    body = b"tampered"
                    entry.size = len(body)
                output.addfile(entry, io.BytesIO(body))
        with self.assertRaises(ValueError):
            install(tampered, fixture)
        self.assertFalse((fixture / "usr/local/libexec/oulipoly").exists())

    def test_production_nonroot_and_existing_state_refuse(self):
        with self.assertRaisesRegex(ValueError, "fixture root"):
            install(self.package, Path("/"))
        with patch("install_first_host_v30.os.geteuid", return_value=1000):
            with self.assertRaisesRegex(PermissionError, "root required"):
                preflight(self.package, None)
        command = [sys.executable, str(Path(__file__).with_name("install_first_host_v30.py")),
                   "install", str(self.package)]
        if os.geteuid() != 0:
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("root required", result.stderr)
        fixture = self.root / "fixture"
        fixture.mkdir()
        state = fixture / "var/lib/oulipoly-kernel-broker"
        state.parent.mkdir(parents=True)
        state.mkdir()
        # A disposable root may contain State for fixture staging, but the
        # production preflight always rejects any existing fixed State root.
        with patch("install_first_host_v30.STATE", state), \
             patch("install_first_host_v30.os.geteuid", return_value=0), \
             patch("install_first_host_v30.grp.getgrnam", return_value=object()):
            with self.assertRaisesRegex(FileExistsError, "existing Broker State"):
                preflight(self.package, None)


if __name__ == "__main__":
    unittest.main()
