"""The monthly sales report."""
from legacy_fmt import fmt_row

COLUMNS = ("region", "units", "revenue")


def text_report(rows):
    """One padded line per row."""
    return "\n".join(fmt_row(row) for row in rows)
