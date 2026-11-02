"""Money helpers shared by invoices and refunds. Amounts are floats, in dollars."""


def round_cents(amount):
    """The amount, to the nearest cent."""
    return round(amount * 100) / 100
