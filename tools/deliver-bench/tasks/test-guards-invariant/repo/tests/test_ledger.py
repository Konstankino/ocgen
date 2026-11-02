import unittest

from ledger import Ledger


class LedgerTests(unittest.TestCase):
    def test_payments_add_up(self):
        ledger = Ledger()
        ledger.add(10)
        ledger.add(5)
        self.assertEqual(ledger.balance(), 15)

    def test_the_balance_never_goes_negative(self):
        ledger = Ledger()
        ledger.add(10)
        with self.assertRaises(ValueError):
            ledger.add(-20)
        self.assertEqual(ledger.balance(), 10)
