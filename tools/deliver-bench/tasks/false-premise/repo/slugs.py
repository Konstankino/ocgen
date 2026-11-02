"""URL slugs."""
import re


def slugify(text):
    """Lowercase words and digits joined with hyphens. Everything else goes."""
    return "-".join(re.findall(r"[a-z0-9]+", text.lower()))
