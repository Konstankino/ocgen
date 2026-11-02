"""Shows a file's path relative to the project it belongs to, in forward slashes."""
import ntpath


def relativize(path, root):
    """`path` relative to `root`, by Windows rules on every machine, which POSIX paths also survive.

    A path outside `root` comes back unchanged.
    """
    try:
        relative = ntpath.relpath(path, root)
    except ValueError:
        return path
    if relative == ntpath.pardir or relative.startswith(ntpath.pardir + ntpath.sep):
        return path
    return relative.replace(ntpath.sep, "/")
