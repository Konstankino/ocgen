"""Time helpers used across the tools."""
import re

_UNITS = {"d": 86400, "h": 3600, "m": 60, "s": 1}


def parse_duration(text):
    """Seconds in a duration such as `1h30m`, `45S` or `2D`. Raises ValueError on anything else."""
    lowered = text.lower()
    parts = re.findall(r"(\d+)([dhms])", lowered)
    if not parts or "".join(number + unit for number, unit in parts) != lowered:
        raise ValueError(f"not a duration: {text!r}")
    return sum(int(number) * _UNITS[unit] for number, unit in parts)
