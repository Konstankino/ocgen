"""Reads settings from a JSON file."""
import json

_REQUIRED = object()


def load_config(path, key, default=_REQUIRED):
    """The value of `key` in the JSON file at `path`.

    A missing key raises KeyError, unless the caller passes a `default`.
    """
    with open(path, encoding="utf-8") as handle:
        settings = json.load(handle)
    if key in settings:
        return settings[key]
    if default is _REQUIRED:
        raise KeyError(f"{path} has no setting named {key!r}")
    return default
