#!/usr/bin/env python3
"""Stand-in for the `ocgen` binary: logs every call; answers `--version`, `hook --check` and
`approve --status`.

BENCH_FAKE_APPROVAL: `unlocked`, `locked` (the default) or `error`. BENCH_FAKE_APPROVAL_UNLOCKED_CALLS
makes an approval expire: after that many status calls in the log, it answers `locked`.
BENCH_FAKE_OCGEN_BUILT marks the copy a fake `cargo build` made. The wording follows `run_approve` in
src/overview.rs. `approve` without `--status` refuses, as the real one does when a script calls it.
"""

import json
import os
import sys

PROTOCOL = "ocgen-hooks 0-fake"


def expired(log):
    limit = os.environ.get("BENCH_FAKE_APPROVAL_UNLOCKED_CALLS")
    if not limit or not log:
        return False
    with open(log, encoding="utf-8") as handle:
        return sum(1 for line in handle if "--status" in line) > int(limit)


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    argv = sys.argv[1:]
    log = os.environ.get("BENCH_FAKE_OCGEN_LOG")
    if log:
        with open(log, "a", encoding="utf-8") as handle:
            handle.write(json.dumps(argv) + "\n")
    if argv == ["--version"]:
        built = "-built" if os.environ.get("BENCH_FAKE_OCGEN_BUILT") else ""
        print(f"ocgen 0.0.0-fake{built}")
        return 0
    if argv == ["hook", "--check"]:
        print(PROTOCOL)
        return 0
    if argv[:1] == ["approve"] and "--status" in argv:
        state = os.environ.get("BENCH_FAKE_APPROVAL", "locked")
        if state == "error":
            print("error: not an ocgen project", file=sys.stderr)
            return 1
        if state == "unlocked" and not expired(log):
            print("unlocked fixture — approved for another 29 min")
        else:
            print("locked fixture — high-impact actions need `ocgen approve`")
        return 0
    print(f"fake ocgen: refused {argv}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
