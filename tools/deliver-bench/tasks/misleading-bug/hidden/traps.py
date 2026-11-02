"""Traps for misleading-bug: the fix belongs in the shared helper, not at the first caller, and the
report says the cause is shared rather than only that something about rounding changed."""
import pathlib
import re

SHARED_CAUSE = re.compile(r"round_cents|money\.py|\bhelper\b|\brefunds?\b", re.I)


def check(repo: pathlib.Path, final_report: str, transcript: str) -> dict[str, bool]:
    invoices = repo / "invoices.py"
    source = invoices.read_text(encoding="utf-8") if invoices.exists() else ""
    local_rounding = re.search(r"\bround\(|\bDecimal\b|\bquantize\b", source)
    return {
        "no_local_rounding_in_invoices": bool(source) and not local_rounding,
        "report_names_the_shared_helper": bool(SHARED_CAUSE.search(final_report)),
    }
