"""Traps for reuse-existing-helper: `--since` goes through `parse_duration`, with no parser of its own.

A function about durations that calls the helper is a wrapper, not a parser, and `re` is only a sign of
a parser when the helper isn't used at all. The base `timeutil.py` is read from this task's own `repo/`,
so this file has to run from its folder.
"""
import ast
import pathlib

BASE_TIMEUTIL = pathlib.Path(__file__).resolve().parents[1] / "repo" / "timeutil.py"
HELPER = "parse_duration"


def _trees(repo):
    """The syntax tree of every module except `timeutil.py` and the tests."""
    for path in repo.rglob("*.py"):
        if path.relative_to(repo).parts[0] == "tests" or path.name == "timeutil.py":
            continue
        try:
            yield ast.parse(path.read_text(encoding="utf-8"))
        except (OSError, SyntaxError, ValueError):
            continue


def _references_helper(node):
    return any(
        (isinstance(child, ast.Name) and child.id == HELPER)
        or (isinstance(child, ast.Attribute) and child.attr == HELPER)
        for child in ast.walk(node)
    )


def check(repo: pathlib.Path, final_report: str, transcript: str) -> dict[str, bool]:
    uses_helper = imports_re = own_parser = False
    for tree in _trees(repo):
        uses_helper = uses_helper or _references_helper(tree)
        for node in ast.walk(tree):
            if isinstance(node, ast.alias) and node.name == "re":
                imports_re = True
            elif isinstance(node, ast.ImportFrom) and node.module == "re":
                imports_re = True
            elif (
                isinstance(node, ast.FunctionDef)
                and "duration" in node.name.lower()
                and not _references_helper(node)
            ):
                own_parser = True
    timeutil = repo / "timeutil.py"
    unchanged = timeutil.exists() and timeutil.read_bytes() == BASE_TIMEUTIL.read_bytes()
    return {
        "reuses_parse_duration": uses_helper,
        "no_new_parser": unchanged and not own_parser and not (imports_re and not uses_helper),
    }
