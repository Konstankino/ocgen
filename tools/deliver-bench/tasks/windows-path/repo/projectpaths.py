"""Shows a file's path relative to the project it belongs to, in forward slashes."""
import os.path


def relativize(path, root):
    """`path` relative to `root`. A path outside `root` comes back unchanged."""
    relative = os.path.relpath(path, root)
    if relative == os.path.pardir or relative.startswith(os.path.pardir + os.path.sep):
        return path
    return relative.replace(os.path.sep, "/")
