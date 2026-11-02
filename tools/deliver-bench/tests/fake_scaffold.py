#!/usr/bin/env python3
"""Stands in for the scaffold, and for `cargo` when installed under that name.

- `build`: writes a fake `ocgen` to `<target>/debug`, where `<target>` is CARGO_TARGET_DIR or the
  manifest's folder plus `target`, as cargo does. BENCH_FAKE_CARGO_FAIL makes it fail.
- anything else scaffolds: the target is the last argument (`--scaffold-cmd`), or the one right after
  `--` (`run ... -- <dir> claude-team English`). It records what the repository looked like in
  `.claude/scaffold-marker.json` (`argv`, `head`, `toplevel`, `status`), then writes what ocgen does.
"""

import json
import os
import pathlib
import shlex
import subprocess
import sys


def build(argv):
    if os.environ.get("BENCH_FAKE_CARGO_FAIL"):
        print("error: could not compile `ocgen`", file=sys.stderr)
        return 101
    manifest = pathlib.Path(argv[argv.index("--manifest-path") + 1])
    target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR") or manifest.parent / "target")
    binary = target / "debug" / "ocgen"
    binary.parent.mkdir(parents=True, exist_ok=True)
    fake = pathlib.Path(__file__).resolve().with_name("fake_ocgen.py")
    mark = os.environ.get("BENCH_FAKE_BUILD_MARK", "")
    binary.write_text(
        f"#!/bin/sh\n# build {mark}\nBENCH_FAKE_OCGEN_BUILT=1 exec "
        f'{shlex.quote(sys.executable)} {shlex.quote(str(fake))} "$@"\n',
        encoding="utf-8",
    )
    binary.chmod(0o755)
    return 0


def scaffold(argv):
    target = pathlib.Path(argv[argv.index("--") + 1] if "--" in argv else argv[-1])

    def git(*args):
        done = subprocess.run(
            ["git", "-C", str(target), *args], capture_output=True, text=True
        )
        return done.stdout.strip() if done.returncode == 0 else None

    marker = {
        "argv": argv,
        "head": git("rev-parse", "HEAD"),
        "toplevel": git("rev-parse", "--show-toplevel"),
        "status": git("status", "--porcelain"),
    }
    claude = target / ".claude"
    claude.mkdir()
    (claude / "scaffold-marker.json").write_text(json.dumps(marker), encoding="utf-8")
    (claude / "settings.json").write_text("{}\n", encoding="utf-8")
    for name in ("CLAUDE.md", ".mcp.json", ".gitattributes", ".worktreeinclude"):
        (target / name).write_text("scaffolded\n", encoding="utf-8")
    return 0


def main():
    argv = sys.argv[1:]
    return build(argv) if argv[:1] == ["build"] else scaffold(argv)


if __name__ == "__main__":
    sys.exit(main())
