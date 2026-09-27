"""Filesystem fixtures for the inert AGE-319 staging artifact."""

import os
import tempfile
import unittest
from pathlib import Path

from stage_versioned_island import ASSETS, stage, verify


class StageVersionedIslandTests(unittest.TestCase):
    def setUp(self):
        target = Path(__file__).resolve().parents[2] / "target" / "age319-staging-tests"
        target.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=target)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.destination = self.root / "generation"
        self.sources = {}
        for name in ASSETS:
            path = self.root / f"source-{name}"
            path.write_bytes(f"different {name} bytes\n".encode())
            self.sources[name] = path
        self.sources["legacy_runner_config"].write_text(
            f'data_dir = "{self.root / "old-state"}"\n'
            f'config_home = "{self.root / "old-config"}"\n'
        )
        self.sources["legacy_bash_config"].write_text(
            f'state_root = "{self.root / "old-handles"}"\n'
            f'agent_runner_bin = "{self.sources["legacy_runner"]}"\n'
        )
        self.sources["fresh_runner_config"].write_text(
            f'data_dir = "{self.root / "new-state"}"\n'
            f'config_home = "{self.root / "new-config"}"\n'
        )
        self.sources["fresh_bash_config"].write_text(
            f'state_root = "{self.root / "new-handles"}"\n'
            f'agent_runner_bin = "{self.destination / ASSETS["fresh_runner"][0]}"\n'
        )

    def test_exact_side_by_side_readback_and_lost_reply(self):
        v1_manifest = self.root / "install-v1.json"
        v1_manifest.write_bytes(b"unmodified old manifest\n")
        record = stage(self.destination, self.sources)
        self.assertEqual(record, verify(self.destination))
        self.assertEqual(record["admission"], "closed")
        self.assertEqual(record["activation"], "inert-no-selector")
        for name, (relative, _) in ASSETS.items():
            self.assertEqual((self.destination / relative).stat().st_mode & 0o111, 0, name)
        self.assertNotEqual(record["assets"]["legacy_runner"]["sha256"], record["assets"]["fresh_runner"]["sha256"])
        self.assertFalse((self.root / "active-v2.json").exists())
        self.assertEqual(v1_manifest.read_bytes(), b"unmodified old manifest\n")
        self.assertEqual(record["assets"]["legacy_bash"]["source_path"], str(self.sources["legacy_bash"]))
        self.sources["legacy_runner"].write_text("source changed after durable copy")
        self.assertEqual(record, verify(self.destination))
        with self.assertRaises(FileExistsError):
            stage(self.destination, self.sources)
        self.assertEqual(record, verify(self.destination))

    def test_tampered_asset_and_config_refuse_readback(self):
        stage(self.destination, self.sources)
        image = self.destination / ASSETS["fresh_bash"][0]
        original = image.read_bytes()
        image.chmod(0o600)
        image.write_bytes(b"X" + original[1:])
        image.chmod(0o400)
        with self.assertRaises(ValueError):
            verify(self.destination)

    def test_mixed_roots_and_missing_image_never_publish(self):
        self.sources["fresh_runner_config"].write_text(
            f'data_dir = "{self.root / "old-state" / "nested"}"\n'
            f'config_home = "{self.root / "new-config"}"\n'
        )
        with self.assertRaises(ValueError):
            stage(self.destination, self.sources)
        self.assertFalse(self.destination.exists())
        self.sources["fresh_runner_config"].write_text(
            f'data_dir = "{self.root / "new-state"}"\n'
            f'config_home = "{self.root / "new-config"}"\n'
        )
        self.sources["fresh_broker"].unlink()
        with self.assertRaises(FileNotFoundError):
            stage(self.destination, self.sources)
        self.assertFalse(self.destination.exists())
        self.assertEqual(list(self.root.glob(".stage-*")), [])

    def test_source_symlink_refuses_without_publication(self):
        image = self.sources["fresh_launcher"]
        image.unlink()
        os.symlink(self.sources["fresh_broker"], image)
        with self.assertRaises(ValueError):
            stage(self.destination, self.sources)
        self.assertFalse(self.destination.exists())

    def test_fresh_bash_cannot_point_to_legacy_runner(self):
        self.sources["fresh_bash_config"].write_text(
            f'state_root = "{self.root / "new-handles"}"\n'
            f'agent_runner_bin = "{self.sources["legacy_runner"]}"\n'
        )
        with self.assertRaisesRegex(ValueError, "fresh Bash helper path"):
            stage(self.destination, self.sources)
        self.assertFalse(self.destination.exists())

    def test_symlinked_fresh_root_cannot_alias_legacy_state(self):
        old_state = self.root / "old-state"
        old_state.mkdir()
        alias = self.root / "new-state"
        alias.symlink_to(old_state, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "fresh root aliases"):
            stage(self.destination, self.sources)
        self.assertFalse(self.destination.exists())


if __name__ == "__main__":
    unittest.main()
