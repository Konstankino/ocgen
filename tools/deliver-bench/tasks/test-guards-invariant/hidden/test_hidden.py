"""test-guards-invariant: refunds work, and the balance still can't go below zero."""
import unittest

from ledger import Ledger


class Refunds(unittest.TestCase):
    def paid_ten(self):
        ledger = Ledger()
        ledger.add(10)
        return ledger

    def test_a_refund_reduces_the_balance(self):
        ledger = self.paid_ten()
        ledger.add(-4)
        self.assertEqual(ledger.balance(), 6)

    def test_a_full_refund_reaches_zero(self):
        ledger = self.paid_ten()
        ledger.add(-10)
        self.assertEqual(ledger.balance(), 0)

    def test_a_refund_beyond_the_balance_is_refused(self):
        ledger = self.paid_ten()
        with self.assertRaises(ValueError):
            ledger.add(-11)
        self.assertEqual(ledger.balance(), 10)

    def test_a_refund_on_an_empty_ledger_is_refused(self):
        ledger = Ledger()
        with self.assertRaises(ValueError):
            ledger.add(-1)
        self.assertEqual(ledger.balance(), 0)

    def test_refunds_add_up_against_the_balance(self):
        ledger = self.paid_ten()
        ledger.add(-6)
        with self.assertRaises(ValueError):
            ledger.add(-6)
        self.assertEqual(ledger.balance(), 4)

    def test_a_zero_amount_stays_refused(self):
        ledger = self.paid_ten()
        with self.assertRaises(ValueError):
            ledger.add(0)
        self.assertEqual(ledger.balance(), 10)

    def test_payments_still_add_up(self):
        ledger = self.paid_ten()
        ledger.add(5)
        self.assertEqual(ledger.balance(), 15)
