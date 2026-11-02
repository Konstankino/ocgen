"""Invoice totals."""
from money import round_cents

CODES = {"SAVE15": 15, "HALF": 50}


def invoice_total(prices, code=None):
    """The sum of `prices`, less the percentage the discount code gives."""
    percent = CODES.get(code, 0)
    return round_cents(sum(prices) * (100 - percent) / 100)
