import unittest

from units import celsius_to_fahrenheit, km_to_miles


class Units(unittest.TestCase):
    def test_km_to_miles(self):
        self.assertAlmostEqual(km_to_miles(10), 6.21, places=2)

    def test_celsius_to_fahrenheit(self):
        self.assertEqual(celsius_to_fahrenheit(100), 212)
