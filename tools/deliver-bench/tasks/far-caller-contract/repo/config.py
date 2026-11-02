"""Reads settings from a JSON file."""
import json


def load_config(path, key):
    """The value of `key` in the JSON file at `path`."""
    with open(path, encoding="utf-8") as handle:
        return json.load(handle).get(key)
