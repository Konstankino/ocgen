"""Small text helpers."""

__all__ = ["shout", "slugify", "title_case"]


def shout(text):
    return text.upper() + "!"


def slugify(text):
    """The text in lowercase, its words joined with single hyphens."""
    return "-".join(text.lower().split())


def title_case(text):
    return " ".join(word.capitalize() for word in text.split())
