"""Disposable checks for the inert fresh-only first-install artifact."""

import io
import json
import tarfile
import tempfile
import unittest
from pathlib import Path

from first_install_v30 import IMAGES, PREFIX, build, check, stage, verify


class FirstInstallV30Tests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.images = {}
        for role in IMAGES:
            path = self.root / role
            path.write_bytes(("featureless-" + role).encode())
            self.images[role] = path
        self.service = self.root / "service"
        self.service.write_bytes(b"[Service]\nExecStart=/pinned/broker\n")
        self.package = self.root / "fresh.tar.gz"

    def test_exact_inert_pair_and_durable_readback(self):
        manifest = build(self.images, self.service, self.package, "0.1.0")
        self.assertEqual(verify(self.package), manifest)
        second = self.root / "second.tar.gz"
        self.assertEqual(build(self.images, self.service, second, "0.1.0"), manifest)
        self.assertEqual(second.read_bytes(), self.package.read_bytes())
        self.assertEqual(manifest["activation"], "unavailable")
        with tarfile.open(self.package) as archive:
            names = {item.name for item in archive.getmembers()}
            self.assertEqual(names, {PREFIX + name for name in
                ("manifest.json", "install-v1.json", "oulipoly-kernel-broker.service", *IMAGES.values())})
            self.assertTrue(all(item.mode == 0o400 and item.isfile() for item in archive.getmembers()))
            pair = json.load(archive.extractfile(PREFIX + "install-v1.json"))
            self.assertEqual(pair["schema"], 2)
            self.assertEqual(pair["generation"], manifest["generation"])
        staged = self.root / "staged"
        self.assertEqual(stage(self.package, staged), manifest)
        self.assertEqual(check(staged), manifest)
        self.assertTrue(all(path.stat().st_mode & 0o111 == 0 for path in staged.iterdir()))
        with self.assertRaises(FileExistsError):
            stage(self.package, staged)
        self.images["bash"].write_bytes(b"different source after package")
        self.assertEqual(check(staged), manifest)

    def test_tampered_package_and_staging_refuse(self):
        build(self.images, self.service, self.package, "0.1.0")
        staged = self.root / "staged"
        stage(self.package, staged)
        image = staged / IMAGES["bash"]
        image.chmod(0o600)
        image.write_bytes(b"changed")
        image.chmod(0o400)
        with self.assertRaisesRegex(ValueError, "changed"):
            check(staged)
        tampered = self.root / "tampered.tar.gz"
        with tarfile.open(self.package) as source, tarfile.open(tampered, "w:gz") as output:
            for item in source.getmembers():
                body = source.extractfile(item).read()
                if item.name == PREFIX + IMAGES["broker"]:
                    body = b"changed broker"
                    item.size = len(body)
                output.addfile(item, io.BytesIO(body))
        with self.assertRaisesRegex(ValueError, "changed"):
            verify(tampered)

    def test_symlink_and_extra_member_refuse(self):
        build(self.images, self.service, self.package, "0.1.0")
        staged = self.root / "staged"
        stage(self.package, staged)
        (staged / "active.json").write_bytes(b"unallowed selector")
        with self.assertRaisesRegex(ValueError, "member set"):
            check(staged)
        (staged / "active.json").unlink()
        image = staged / IMAGES["runner"]
        image.unlink()
        image.symlink_to(self.images["runner"])
        with self.assertRaisesRegex(ValueError, "unsafe staged file"):
            check(staged)


if __name__ == "__main__":
    unittest.main()
