"""Small text helpers."""

__all__ = ["shout", "title_case"]


def shout(text):
    return text.upper() + "!"


def title_case(text):
    return " ".join(word.capitalize() for word in text.split())
