"""Persistent aggregate reservation ledger shared by all explicitly paid gates.

Use `with LiveBudget(path) as ledger` to reserve(label, limits, metadata), then
reconcile(reservation, usage, elapsed_seconds). `limits` uses
runtime.budgets camelCase names; usage uses inspect.data.budget.usage names.
An unreconciled reservation remains fully charged, including after process death.
No credential or prompt is stored. SQLite IMMEDIATE transactions serialize callers.
"""
import json
import math
from pathlib import Path
import sqlite3
import time
import uuid

DEFAULTS = {"providerRequests": 100, "totalTokens": 200000,
            "wallTimeSeconds": 1800, "costMicrousd": 25000000}
KEYS = {"providerRequests": "maxProviderRequests", "totalTokens": "maxTotalTokens",
        "wallTimeSeconds": "maxWallTimeSeconds", "costMicrousd": "maxCostMicrousd"}


class LiveBudget:
    def __init__(self, filename, limits=None):
        self.filename = Path(filename).resolve()
        self.filename.parent.mkdir(parents=True, exist_ok=True)
        self.connection = sqlite3.connect(self.filename, timeout=30, isolation_level=None)
        try:
            self.connection.execute("PRAGMA journal_mode=WAL")
            self.connection.execute("CREATE TABLE IF NOT EXISTS budget_config (id INTEGER PRIMARY KEY, limits_json TEXT NOT NULL)")
            self.connection.execute("CREATE TABLE IF NOT EXISTS reservations (id TEXT PRIMARY KEY, label TEXT NOT NULL, status TEXT NOT NULL, charged_json TEXT NOT NULL, reserved_json TEXT NOT NULL, metadata_json TEXT NOT NULL, created REAL NOT NULL)")
            self.connection.execute("INSERT OR IGNORE INTO budget_config VALUES (1, ?)", (json.dumps(limits or DEFAULTS),))
            configured = json.loads(self.connection.execute("SELECT limits_json FROM budget_config WHERE id=1").fetchone()[0])
            if limits is not None and configured != limits:
                raise ValueError("existing live-budget limits differ; cannot silently reset or increase a shared allowance")
            self.limits = configured
        except BaseException:
            self.close()
            raise

    def close(self):
        """Release the SQLite handle; committed reservations remain charged."""
        self.connection.close()

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        self.close()

    def _totals(self):
        totals = {key: 0 for key in KEYS}
        for (row,) in self.connection.execute("SELECT charged_json FROM reservations"):
            for key, value in json.loads(row).items():
                totals[key] += value
        return totals

    def reserve(self, label, limits, metadata=None):
        charged = {}
        for key, field in KEYS.items():
            value = limits.get(field)
            if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
                raise ValueError(f"live dispatch requires a positive explicit {field}")
            charged[key] = value
        self.connection.execute("BEGIN IMMEDIATE")
        try:
            totals = self._totals()
            for key, value in charged.items():
                if totals[key] + value > self.limits[key]:
                    raise ValueError(f"aggregate live budget exhausted for {key}: {totals[key]} + {value} > {self.limits[key]}")
            reservation = str(uuid.uuid4())
            encoded = json.dumps(charged)
            self.connection.execute("INSERT INTO reservations VALUES (?, ?, 'reserved', ?, ?, ?, ?)",
                                    (reservation, label, encoded, encoded, json.dumps(metadata or {}), time.time()))
            self.connection.execute("COMMIT")
            return reservation
        except BaseException:
            self.connection.execute("ROLLBACK")
            raise

    def reconcile(self, reservation, usage, elapsed_seconds):
        """Only reconcile when durable usage is complete, including partial failures.

        If any provider request is still uncertain, leave the reservation intact.
        Reasoning tokens are a subset of outputTokens and are not double counted.
        """
        required = ("providerRequests", "inputTokens", "outputTokens", "costMicrousd")
        if any(not isinstance(usage.get(key), int) or usage[key] < 0 for key in required):
            raise ValueError("missing or invalid durable usage; retain full live reservation")
        if usage.get("unpricedProviderRequests", 0):
            raise ValueError("unpriced provider usage; retain full live reservation")
        charged = {"providerRequests": usage["providerRequests"],
                   "totalTokens": usage["inputTokens"] + usage["outputTokens"],
                   "wallTimeSeconds": math.ceil(elapsed_seconds), "costMicrousd": usage["costMicrousd"]}
        self.connection.execute("BEGIN IMMEDIATE")
        try:
            row = self.connection.execute("SELECT status, reserved_json FROM reservations WHERE id=?", (reservation,)).fetchone()
            if row is None or row[0] != "reserved":
                raise ValueError("reservation is absent or already reconciled")
            reserved = json.loads(row[1])
            exceeded = [key for key in charged if charged[key] > reserved[key]]
            self.connection.execute("UPDATE reservations SET status=?, charged_json=? WHERE id=?",
                                    ("exceeded" if exceeded else "reconciled", json.dumps(charged), reservation))
            self.connection.execute("COMMIT")
            if exceeded:
                raise ValueError("runtime exceeded reserved live allowance: " + ", ".join(exceeded))
        except BaseException:
            if self.connection.in_transaction:
                self.connection.execute("ROLLBACK")
            raise

    def summary(self):
        return {"limits": self.limits, "charged": self._totals(),
                "unreconciled": self.connection.execute("SELECT count(*) FROM reservations WHERE status='reserved'").fetchone()[0],
                "costKind": "estimated from versioned public pricing, not an invoice"}
