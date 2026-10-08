"""Opt-in required-account controls over the delivered Codex pairing driver.

The driver path and built binaries must be supplied explicitly. This invokes
only the driver's synthetic native, never a live CLI, profile or account.
"""
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import unittest

DRIVER = os.environ.get("OULIPOLY_PAIRING_DRIVER")
if DRIVER:
    path = Path(DRIVER)
    if not path.is_absolute():
        raise ValueError("pairing driver must be an absolute path")
    spec = importlib.util.spec_from_file_location("delivered_pairing", path)
    driver = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(driver)


@unittest.skipUnless(DRIVER, "requires explicit immutable pairing driver")
class WorkAccountPairing(unittest.TestCase):
    def setUp(self):
        self.case = driver.RunnerPairing("test_late_record_failure_keeps_publication_unestablished")
        self.case.setUp()

    def tearDown(self):
        self.case.tearDown()

    def test_endpoint_failure_is_readable_from_required_terminal_and_store(self):
        case = self.case
        run = case.start("recordfail synthetic")
        ack, end = case.turn(run, 0)
        diagnostic = run.event("endpoint-record-error")
        case.close(run)
        terminal = next(v for v in run.seen if v.get("event") == "terminal")
        required = terminal["required_account"]
        self.assertEqual(terminal["status"], "closed")
        self.assertEqual(terminal["owed"], 0)
        self.assertEqual(required["counts"]["endpoint-record-error"], 1)
        self.assertTrue(required["details_complete"])
        self.assertEqual(required["persistence"], "stored")
        record = required["records"][0]
        self.assertEqual(record["message_id"], ack["message_id"])
        self.assertEqual(record["message_id"], diagnostic["message_id"])
        self.assertEqual(record["root_id"], required["root_id"])
        self.assertEqual(record["incarnation"], 1)
        self.assertEqual(record["observed_generation"], 1)
        self.assertEqual(record["private_details"], "withheld")
        self.assertEqual(record["endpoint_durability"], "contrary-diagnostic")
        self.assertEqual(record["canonical_publication"], "not-established")
        self.assertEqual(record["source"], "Runner-owner-observation")
        self.assertTrue(record["endpoint_tag_complete"])
        # The input still has the same proven host ACK/tagged end and no debt.
        self.assertEqual(end["message_id"], ack["message_id"])
        self.assertTrue(end["own_turn_end"])
        self.assertEqual(end["endpoint_durability"], "not-established")
        with sqlite3.connect(f"file:{case.fixture.root}/store/intent.sqlite3?mode=ro", uri=True) as db:
            stored = json.loads(db.execute("SELECT required_account FROM root").fetchone()[0])
            self.assertEqual(stored["records"], required["records"])
            self.assertEqual(db.execute("SELECT count(*) FROM message WHERE turn_end_generation=1").fetchone()[0], 1)
        self.assertEqual(len(case.fixture.native_calls()), 1)
        started = next(v for v in run.seen if v.get("event") == "resident-session-started")
        self.assertEqual(started["canonical_binding"], "unbound")
        negotiated = next(v for v in run.seen if v.get("event") == "negotiated")
        self.assertFalse(negotiated["live_reattach"])


if __name__ == "__main__":
    unittest.main()
