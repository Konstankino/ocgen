"""red-base: the small goal works, whatever the unrelated test does."""
import unittest

import units


class MilesToKm(unittest.TestCase):
    def test_one_mile(self):
        self.assertAlmostEqual(units.miles_to_km(1), 1.609, places=2)

    def test_zero(self):
        self.assertEqual(units.miles_to_km(0), 0)

    def test_it_reverses_km_to_miles(self):
        self.assertAlmostEqual(units.km_to_miles(units.miles_to_km(26.2)), 26.2, places=2)

    def test_the_existing_conversion_still_works(self):
        self.assertAlmostEqual(units.km_to_miles(10), 6.21, places=2)
