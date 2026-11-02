"""Starts the web server."""
from config import load_config


def bind_address(path):
    """The (host, port) the server listens on."""
    return load_config(path, "host"), load_config(path, "port")
