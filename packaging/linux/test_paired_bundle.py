import importlib.util
import tarfile
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("build_paired_bundle.py")
SPEC = importlib.util.spec_from_file_location("paired_bundle", SCRIPT)
bundle = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bundle)


class PairedBundleTest(unittest.TestCase):
    def test_manifest_service_and_two_entrant_links(self):
        work = Path(__file__).resolve().parents[2] / "src-tauri/target/age319-package-fixture"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as folder:
            root = Path(folder)
            runner, broker, launcher, bash, service, output = (root / name for name in ("runner", "broker", "launcher", "bash", "service", "pair.tar.gz"))
            runner.write_bytes(b"runner-v1")
            broker.write_bytes(b"broker-v1")
            launcher.write_bytes(b"launcher-v1")
            bash.write_bytes(b"bash-v1")
            service.write_text("[Service]\nKillMode=process\n")
            expected = bundle.build(runner, broker, launcher, service, output, "0.1.0")
            self.assertEqual(expected["schema"], 1)
            self.assertNotIn("bash_sha256", expected)
            self.assertEqual(bundle.verify(output), expected)
            with tarfile.open(output) as archive:
                names = {member.name: member for member in archive.getmembers()}
                self.assertNotIn("usr/local/libexec/oulipoly/agent-bash", names)
                self.assertEqual(names["usr/local/bin/agents"].linkname, names["usr/local/bin/oulipoly-plane"].linkname)
                self.assertEqual(archive.extractfile("etc/systemd/system/oulipoly-kernel-broker.service").read(), service.read_bytes())
            paired = bundle.build(runner, broker, launcher, service, output, "0.1.0", bash)
            self.assertEqual(paired["schema"], 2)
            self.assertEqual(bundle.verify(output), paired)
            self.assertNotEqual(paired["generation"], expected["generation"])
            launcher.write_bytes(b"changed-launcher")
            changed = bundle.build(runner, broker, launcher, service, output, "0.1.0", bash)
            self.assertNotEqual(changed["generation"], paired["generation"])
            bash.write_bytes(b"changed-bash")
            changed_bash = bundle.build(runner, broker, launcher, service, output, "0.1.0", bash)
            self.assertNotEqual(changed_bash["generation"], changed["generation"])
            broker.unlink()
            with self.assertRaises(FileNotFoundError):
                bundle.build(runner, broker, launcher, service, output, "0.1.0", bash)


if __name__ == "__main__":
    unittest.main()
