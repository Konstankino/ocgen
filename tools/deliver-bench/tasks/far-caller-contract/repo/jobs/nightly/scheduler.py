"""When the nightly cleanup runs."""
from config import load_config

DEFAULT_HOUR = 2


def run_hour(path):
    """The configured hour, or the default when the file sets none."""
    hour = load_config(path, "run_hour")
    return DEFAULT_HOUR if hour is None else hour
