"""Credential-free regression checks for the paid suite's aggregate guard."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import tempfile
import unittest

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

    def test_independent_callers_cannot_oversubscribe(self):
        def reserve(label):
            try:
                return LiveBudget(self.path).reserve(label, self.request)
            except ValueError:
                return None
        with ThreadPoolExecutor(max_workers=2) as executor:
            results = list(executor.map(reserve, ["left", "right"]))
        self.assertEqual(sum(value is not None for value in results), 1)
        self.assertEqual(self.ledger.summary()["charged"]["providerRequests"], 2)

    def test_interrupted_reservation_survives_reopen(self):
        self.ledger.reserve("interrupted", self.request)
        resumed = LiveBudget(self.path)
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
        with self.assertRaisesRegex(ValueError, "cannot silently reset"):
            LiveBudget(self.path, {**self.limits, "providerRequests": 100})

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
