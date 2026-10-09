"""Uid-ancestor mode controls, not an installed/root workload qualification.

Real filesystem allocation/chmod/locks and check_owned run as the current uid.
TRUSTED_OWNERS includes root + current uid; lstat of ancestors OUTSIDE the
private scratch reports synthetic safe metadata (the repo may be writable).
Fixture custody/modes are real unless a test explicitly injects an owner.
No native owner/provider is started; host-root ancestry is not established.
"""

from contextlib import contextmanager
import json
import os
from pathlib import Path
import pwd
import stat
import tempfile
import unittest
from unittest import mock

import frontdoor


@contextmanager
def umask(value):
    previous = os.umask(value)
    try:
        yield
    finally:
        os.umask(previous)


def mode(path):
    return stat.S_IMODE(os.lstat(path).st_mode)


class UidTraversal(unittest.TestCase):
    def setUp(self):
        # Keep all filesystem effects inside the assigned worktree.
        scratch = tempfile.TemporaryDirectory(dir=Path(__file__).resolve().parent)
        self.addCleanup(scratch.cleanup)
        self.base = Path(scratch.name)
        scratch_root = self.base
        original_lstat = os.lstat
        def fixture_ancestry(path, *args, **kwargs):
            result = original_lstat(path, *args, **kwargs)
            shown = Path(path).absolute()
            if shown != scratch_root and scratch_root not in shown.parents:
                fields = list(result)
                fields[0] &= ~0o022
                fields[4] = 0
                return os.stat_result(fields)
            return result
        ancestry = mock.patch.object(frontdoor.os, "lstat", side_effect=fixture_ancestry)
        ancestry.start()
        self.addCleanup(ancestry.stop)
        self.site = {"run_base": str(self.base)}
        self.user = pwd.getpwuid(os.getuid())
        self.uid_dir = self.base / str(self.user.pw_uid)
        owners = mock.patch.object(frontdoor, "TRUSTED_OWNERS", {0, os.getuid()})
        owners.start()
        self.addCleanup(owners.stop)

    def allocate(self, allocator, mask):
        with umask(mask):
            _, run, lock, _ = allocator(self.site, self.user)
        self.addCleanup(os.close, lock)
        return Path(run)

    def test_fresh_ordinary_and_live_masks_preserve_traversal_and_privacy(self):
        for allocator in (frontdoor.make_run, frontdoor.make_live_run):
            for mask in (0o022, 0o027, 0o077):
                with self.subTest(allocator=allocator.__name__, mask=oct(mask)):
                    # Fresh uid ancestor in each case; keep earlier held runs.
                    self.base = Path(tempfile.mkdtemp(dir=self.base))
                    self.site = {"run_base": str(self.base)}
                    self.uid_dir = self.base / str(self.user.pw_uid)
                    run = self.allocate(allocator, mask)
                    self.assertEqual(mode(self.uid_dir), 0o711)
                    self.assertEqual(mode(run), 0o711)
                    self.assertEqual(mode(run / "private"), 0o700)
                    self.assertEqual(mode(run / "private" / "lock"), 0o600)
                    if allocator == frontdoor.make_live_run:
                        self.assertEqual(mode(run / "private" / frontdoor.LIVE_RESERVATION), 0o600)
                    with umask(mask):
                        frontdoor.write_private(run / "private" / "request.json", {"task": "private"})
                        directory = frontdoor.loss_dir(str(self.uid_dir), create=True)
                        frontdoor.write_durable(directory, "record.json", {"private": True})
                    self.assertEqual(mode(run / "private" / "request.json"), 0o600)
                    self.assertEqual(mode(directory), 0o700)
                    self.assertEqual(mode(Path(directory) / "record.json"), 0o600)
                    print(json.dumps({"case": "fresh", "allocator": allocator.__name__, "mask": oct(mask),
                                      "uid_dir": oct(mode(self.uid_dir)), "run": oct(mode(run)),
                                      "private": oct(mode(run / "private")), "lock": oct(mode(run / "private" / "lock")),
                                      "loss_dir": oct(mode(directory)), "loss_record": oct(mode(Path(directory) / "record.json"))}))

    def test_existing_restrictive_uid_is_repaired_without_changing_private_descendants(self):
        for allocator in (frontdoor.make_run, frontdoor.make_live_run):
            for mask in (0o022, 0o077):
                with self.subTest(allocator=allocator.__name__, mask=oct(mask)):
                    self.uid_dir.mkdir(exist_ok=True)
                    os.chmod(self.uid_dir, 0o700)
                    directory = Path(frontdoor.loss_dir(str(self.uid_dir), create=True))
                    frontdoor.write_durable(str(directory), "record.json", {"private": True})
                    # A held earlier run must survive sweep and normalization.
                    earlier = self.allocate(frontdoor.make_run, 0o077)
                    store = earlier / "store"
                    store.mkdir(mode=0o700)
                    os.chmod(store, 0o700)
                    frontdoor.write_private(store / "evidence", "private store sentinel")
                    os.chmod(self.uid_dir, 0o700)
                    watched = [directory, directory / "record.json", earlier / "private",
                               earlier / "private" / "lock", store, store / "evidence"]
                    before = [(p.stat().st_ino, mode(p)) for p in watched]
                    self.allocate(allocator, mask)
                    self.assertEqual(mode(self.uid_dir), 0o711)
                    self.assertEqual([(p.stat().st_ino, mode(p)) for p in watched], before)
                    self.assertEqual((store / "evidence").read_text(), "private store sentinel")
                    print(json.dumps({"case": "existing", "allocator": allocator.__name__, "mask": oct(mask),
                                      "before_uid_dir": "0o700", "after_uid_dir": oct(mode(self.uid_dir)),
                                      "private_descendant_inodes_modes_unchanged": True}))

    def test_writable_uid_is_refused_not_normalized(self):
        self.uid_dir.mkdir()
        for allocator in (frontdoor.make_run, frontdoor.make_live_run):
            for unsafe in (0o720, 0o702):
                os.chmod(self.uid_dir, unsafe)
                with self.assertRaisesRegex(frontdoor.Refused, "writable"):
                    allocator(self.site, self.user)
                self.assertEqual(mode(self.uid_dir), unsafe)
                self.assertEqual(list(self.uid_dir.iterdir()), [])

    def test_writable_run_base_is_refused_before_existing_uid_repair(self):
        self.uid_dir.mkdir(mode=0o700)
        os.chmod(self.base, 0o770)
        for allocator in (frontdoor.make_run, frontdoor.make_live_run):
            with self.assertRaisesRegex(frontdoor.Refused, "writable"):
                allocator(self.site, self.user)
        self.assertEqual(mode(self.uid_dir), 0o700)
        self.assertEqual(list(self.uid_dir.iterdir()), [])

    def test_symlink_and_regular_file_are_not_normalized(self):
        target = self.base / "target"
        target.mkdir(mode=0o700)
        self.uid_dir.symlink_to(target)
        for allocator in (frontdoor.make_run, frontdoor.make_live_run):
            with self.assertRaisesRegex(frontdoor.Refused, "symlink"):
                allocator(self.site, self.user)
        self.assertEqual(mode(target), 0o700)
        self.assertEqual(list(target.iterdir()), [])
        self.uid_dir.unlink()
        self.uid_dir.write_text("not a directory")
        os.chmod(self.uid_dir, 0o600)
        for allocator in (frontdoor.make_run, frontdoor.make_live_run):
            with self.assertRaises(NotADirectoryError):
                allocator(self.site, self.user)
        self.assertEqual(mode(self.uid_dir), 0o600)
        self.assertEqual(self.uid_dir.read_text(), "not a directory")

    def test_untrusted_uid_owner_is_refused_before_mode_change(self):
        self.uid_dir.mkdir(mode=0o700)
        original = os.lstat
        def untrusted(path, *args, **kwargs):
            result = original(path, *args, **kwargs)
            if os.fspath(path) == str(self.uid_dir):
                # No chown authority: inject only this uid-dir owner reading.
                fields = list(result)
                fields[4] = max(frontdoor.TRUSTED_OWNERS) + 1
                return os.stat_result(fields)
            return result
        with mock.patch.object(frontdoor.os, "lstat", side_effect=untrusted):
            for allocator in (frontdoor.make_run, frontdoor.make_live_run):
                with self.assertRaisesRegex(frontdoor.Refused, "not owned by root"):
                    allocator(self.site, self.user)
        self.assertEqual(mode(self.uid_dir), 0o700)
        self.assertEqual(list(self.uid_dir.iterdir()), [])

    def test_metadata_reads_do_not_repair_restrictive_uid(self):
        self.uid_dir.mkdir(mode=0o700)
        directory = Path(frontdoor.loss_dir(str(self.uid_dir), create=True))
        name = "lost"
        record = {"schema": frontdoor.LOSS_ACCOUNT_SCHEMA, "account": name,
                  "requester": frontdoor.control_requester(self.user.pw_uid), "store_account": None}
        frontdoor.write_durable(str(directory), name + ".json", record)
        before = (mode(self.uid_dir), (directory / (name + ".json")).read_bytes())
        with mock.patch.object(frontdoor, "emit", return_value=True):
            self.assertEqual(frontdoor.discover("/unused-package", self.site, self.user), 0)
            for op in ({"op": "loss-accounts"}, {"op": "loss-account", "account": name}):
                self.assertEqual(frontdoor.loss_route(self.site, self.user, op), 0)
        self.assertEqual((mode(self.uid_dir), (directory / (name + ".json")).read_bytes()), before)
        self.assertEqual(mode(directory), 0o700)


if __name__ == "__main__":
    unittest.main()
