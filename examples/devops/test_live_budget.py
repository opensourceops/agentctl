"""Credential-free regression checks for the paid suite's aggregate guard."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

from live_budget import LiveBudget


class SharedBudgetTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "budget.sqlite3"
        self.limits = {"providerRequests": 3, "totalTokens": 100,
                       "wallTimeSeconds": 60, "costMicrousd": 1000}
        self.request = {"maxProviderRequests": 2, "maxTotalTokens": 60,
                        "maxWallTimeSeconds": 30, "maxCostMicrousd": 600}
        self.ledger = LiveBudget(self.path, self.limits)
        self.addCleanup(self.ledger.close)

    def test_independent_callers_cannot_oversubscribe(self):
        def reserve(label):
            try:
                with LiveBudget(self.path) as ledger:
                    return ledger.reserve(label, self.request)
            except ValueError:
                return None
        with ThreadPoolExecutor(max_workers=2) as executor:
            results = list(executor.map(reserve, ["left", "right"]))
        self.assertEqual(sum(value is not None for value in results), 1)
        self.assertEqual(self.ledger.summary()["charged"]["providerRequests"], 2)

    def test_interrupted_reservation_survives_reopen(self):
        self.ledger.reserve("interrupted", self.request)
        self.ledger.close()
        with LiveBudget(self.path) as resumed:
            with self.assertRaisesRegex(ValueError, "exhausted"):
                resumed.reserve("new-run", self.request)
            self.assertEqual(resumed.summary()["unreconciled"], 1)

    def test_reconcile_does_not_double_count_reasoning(self):
        reservation = self.ledger.reserve("first", self.request)
        self.ledger.reconcile(reservation, {"providerRequests": 1, "inputTokens": 10,
            "outputTokens": 20, "reasoningTokens": 15, "costMicrousd": 50}, 2.1)
        charged = self.ledger.summary()["charged"]
        self.assertEqual(charged["totalTokens"], 30)
        self.assertEqual(charged["wallTimeSeconds"], 3)
        self.ledger.reserve("second", self.request)

    def test_unknown_pricing_retains_full_reservation(self):
        reservation = self.ledger.reserve("unknown-price", self.request)
        with self.assertRaisesRegex(ValueError, "unpriced"):
            self.ledger.reconcile(reservation, {"providerRequests": 1, "inputTokens": 10,
                "outputTokens": 5, "costMicrousd": 0, "unpricedProviderRequests": 1}, 1)
        self.assertEqual(self.ledger.summary()["charged"]["costMicrousd"], 600)

    def test_allowance_cannot_be_reset_by_later_caller(self):
        connection = sqlite3.connect(self.path, isolation_level=None)
        self.addCleanup(connection.close)
        with patch("live_budget.sqlite3.connect", return_value=connection):
            with self.assertRaisesRegex(ValueError, "cannot silently reset"):
                LiveBudget(self.path, {**self.limits, "providerRequests": 100})
        with self.assertRaises(sqlite3.ProgrammingError):
            connection.execute("SELECT 1")

    def test_context_closes_before_database_cleanup(self):
        for interrupted in [False, True]:
            with self.subTest(interrupted=interrupted), tempfile.TemporaryDirectory() as directory:
                database = Path(directory) / "managed.sqlite3"
                propagated = False
                try:
                    with LiveBudget(database, self.limits) as ledger:
                        ledger.reserve("managed", self.request)
                        if interrupted:
                            raise RuntimeError("fixture interruption")
                except RuntimeError as error:
                    propagated = True
                    self.assertTrue(interrupted)
                    self.assertEqual(str(error), "fixture interruption")
                self.assertEqual(propagated, interrupted)
                with self.assertRaises(sqlite3.ProgrammingError):
                    ledger.connection.execute("SELECT 1")
                with LiveBudget(database) as resumed:
                    self.assertEqual(resumed.summary()["unreconciled"], 1)
                    self.assertEqual(resumed.summary()["charged"]["providerRequests"], 2)
                database.unlink()
                self.assertFalse(database.exists())

    def test_exceeded_actual_usage_stays_charged(self):
        reservation = self.ledger.reserve("exceeded", self.request)
        with self.assertRaisesRegex(ValueError, "exceeded"):
            self.ledger.reconcile(reservation, {"providerRequests": 4, "inputTokens": 70,
                "outputTokens": 40, "costMicrousd": 1200}, 70)
        self.assertEqual(self.ledger.summary()["charged"]["providerRequests"], 4)
        with self.assertRaisesRegex(ValueError, "exhausted"):
            self.ledger.reserve("after-exceeded", self.request)


if __name__ == "__main__":
    unittest.main()
