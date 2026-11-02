"""The monthly sales report."""
import csv

from legacy_fmt import fmt_row

COLUMNS = ("region", "units", "revenue")


def text_report(rows):
    """One padded line per row."""
    return "\n".join(fmt_row(row) for row in rows)


def export_csv(rows, path):
    """Write the rows to `path` as CSV, with a header line."""
    with open(path, "w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=COLUMNS)
        writer.writeheader()
        writer.writerows(rows)
