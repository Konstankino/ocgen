import unittest

from invoices import invoice_total
from refunds import prorated_refund


class Billing(unittest.TestCase):
    def test_invoice_adds_up_the_prices(self):
        self.assertEqual(invoice_total([10.0, 2.5]), 12.5)

    def test_invoice_applies_a_discount_code(self):
        self.assertEqual(invoice_total([20.0], "HALF"), 10.0)

    def test_refund_covers_the_unused_days(self):
        self.assertEqual(prorated_refund(10.0, 5, 10), 5.0)

    def test_refund_of_an_unused_plan_is_everything(self):
        self.assertEqual(prorated_refund(20.0, 0, 30), 20.0)
