#!/usr/bin/env python3
"""The project's style check, run by CI: every public function of a module is listed in its `__all__`."""
import ast
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def problems(path):
    tree = ast.parse(path.read_text(encoding="utf-8"))
    listed = set()
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(getattr(t, "id", "") == "__all__" for t in node.targets):
            listed = set(ast.literal_eval(node.value))
    return [
        f"{path.name}: {node.name} is public but not in __all__"
        for node in tree.body
        if isinstance(node, ast.FunctionDef)
        and not node.name.startswith("_")
        and node.name not in listed
    ]


def main():
    found = [line for path in sorted(ROOT.glob("*.py")) for line in problems(path)]
    for line in found:
        print(line)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
