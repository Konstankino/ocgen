"""misleading-bug: cents come out wrong everywhere the shared helper is used. Exact, and both ways: a
fix that rounds everything up gets these wrong too."""
import unittest

from invoices import invoice_total
from refunds import prorated_refund


class Invoices(unittest.TestCase):
    def test_the_reported_discount_rounds_up_to_the_nearest_cent(self):
        self.assertEqual(float(invoice_total([9.95], "SAVE15")), 8.46)

    def test_another_discount_rounds_up_to_the_nearest_cent(self):
        self.assertEqual(float(invoice_total([5.55], "SAVE15")), 4.72)

    def test_a_discount_rounds_down_to_the_nearest_cent(self):
        self.assertEqual(float(invoice_total([9.99], "SAVE15")), 8.49)

    def test_an_invoice_without_a_code_keeps_its_cents(self):
        self.assertEqual(float(invoice_total([19.99])), 19.99)

    def test_an_exact_amount_stays_exact(self):
        self.assertEqual(float(invoice_total([20.0], "HALF")), 10.0)


class Refunds(unittest.TestCase):
    def test_a_refund_rounds_up_to_the_nearest_cent(self):
        self.assertEqual(float(prorated_refund(10.0, 1, 3)), 6.67)

    def test_another_refund_rounds_up_to_the_nearest_cent(self):
        self.assertEqual(float(prorated_refund(25.0, 1, 7)), 21.43)

    def test_a_refund_rounds_down_to_the_nearest_cent(self):
        self.assertEqual(float(prorated_refund(10.0, 2, 3)), 3.33)
