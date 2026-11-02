"""Money helpers shared by invoices and refunds. Amounts are floats, in dollars."""


def round_cents(amount):
    """The amount in whole cents."""
    return int(amount * 100) / 100
