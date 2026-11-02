"""Refunds for a plan cancelled part way through its term."""
from money import round_cents


def prorated_refund(paid, days_used, days_total):
    """What's owed back for the days not used."""
    return round_cents(paid * (days_total - days_used) / days_total)
