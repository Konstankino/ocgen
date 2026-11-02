"""Source checks on the tool itself: it never grants an approval, never bypasses permissions.

The behaviour is pinned in test_run.py; these catch a code path no test happens to drive. Build the
command printed for the human as a string: a list holding "approve" reads as a command to run.
"""

import ast
import os
import sys
import unittest

from bench_helpers import BENCH, SCORE, TOOL_DIR

PROCESS_CALLS = {"run", "Popen", "call", "check_call", "check_output", "system", "popen"}
UNSAFE_WORDS = ("bypassPermissions", "dangerously-skip-permissions")


def parse(path):
    return ast.parse(path.read_text(encoding="utf-8"), filename=str(path))


def strings_under(node):
    return [
        child.value
        for child in ast.walk(node)
        if isinstance(child, ast.Constant) and isinstance(child.value, str)
    ]


def docstring_nodes(tree):
    """The docstrings: prose may name a flag the code never passes."""
    found = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            first = node.body[0] if node.body else None
            if isinstance(first, ast.Expr) and isinstance(first.value, ast.Constant):
                found.add(id(first.value))
    return found


def called_name(node):
    func = node.func
    if isinstance(func, ast.Attribute):
        return func.attr
    return func.id if isinstance(func, ast.Name) else ""


class NoGrant(unittest.TestCase):
    def test_every_approve_command_is_the_status_form(self):
        status_forms = 0
        for path in (BENCH, SCORE):
            for node in ast.walk(parse(path)):
                if isinstance(node, (ast.List, ast.Tuple)):
                    words = [
                        item.value
                        for item in node.elts
                        if isinstance(item, ast.Constant) and isinstance(item.value, str)
                    ]
                    if "approve" in words:
                        self.assertIn("--status", words, f"{path.name}:{node.lineno}")
                        status_forms += 1
                elif isinstance(node, ast.Call) and called_name(node) in PROCESS_CALLS:
                    text = " ".join(strings_under(node))
                    if "approve" in text:
                        self.assertIn("--status", text, f"{path.name}:{node.lineno}")
                        status_forms += 1
        self.assertGreater(status_forms, 0, "no status check found: this test checked nothing")

    def test_no_permission_bypass_in_the_code(self):
        for path in (BENCH, SCORE):
            tree = parse(path)
            prose = docstring_nodes(tree)
            for node in ast.walk(tree):
                if not (isinstance(node, ast.Constant) and isinstance(node.value, str)):
                    continue
                if id(node) in prose:
                    continue
                for word in UNSAFE_WORDS:
                    self.assertNotIn(word, node.value, f"{path.name}:{node.lineno}")


class SourceRules(unittest.TestCase):
    def test_no_script_is_named_for_a_gated_action(self):
        for folder, subfolders, files in os.walk(TOOL_DIR):
            subfolders[:] = [name for name in subfolders if name not in ("runs", "__pycache__")]
            for name in files:
                self.assertFalse(
                    name.startswith(("deploy", "publish")), os.path.join(folder, name)
                )

    @unittest.skipUnless(hasattr(sys, "stdlib_module_names"), "needs Python 3.10")
    def test_only_the_standard_library_is_imported(self):
        allowed = set(sys.stdlib_module_names) | {"bench", "score"}
        for path in (BENCH, SCORE):
            for node in ast.walk(parse(path)):
                names = []
                if isinstance(node, ast.Import):
                    names = [alias.name for alias in node.names]
                elif isinstance(node, ast.ImportFrom) and node.level == 0:
                    names = [node.module]
                for name in names:
                    self.assertIn(name.split(".")[0], allowed, f"{path.name}:{node.lineno}")

    def test_each_file_has_its_usage_in_the_header(self):
        for path in (BENCH, SCORE):
            header = ast.get_docstring(parse(path)) or ""
            self.assertIn("usage", header.lower(), path.name)

    def test_the_scoring_module_runs_nothing(self):
        for node in ast.walk(parse(SCORE)):
            if isinstance(node, ast.Import):
                self.assertNotIn("subprocess", [alias.name for alias in node.names])
            elif isinstance(node, ast.ImportFrom):
                self.assertNotEqual(node.module, "subprocess")


if __name__ == "__main__":
    unittest.main()
