import unittest
from vendor_version import major
class VersionTests(unittest.TestCase):
    def test_single_digit(self): self.assertEqual(major("2.3.4"), 2)
    def test_multi_digit(self): self.assertEqual(major("12.3.4"), 12)
    def test_invalid(self):
        with self.assertRaises(ValueError): major("x.0.0")
