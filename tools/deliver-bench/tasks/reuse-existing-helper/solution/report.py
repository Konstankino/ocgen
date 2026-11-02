"""Prints a report of the events in a JSON-lines log: `python3 report.py <log> [--since 1h30m]`.

Each line holds an event's `name` and its `ts`, in seconds since the epoch.
"""
import argparse
import json
import sys
import time

from timeutil import parse_duration


def load_events(path):
    with open(path, encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def build_report(events):
    """One line per event, oldest first, then the count."""
    lines = [event["name"] for event in sorted(events, key=lambda event: event["ts"])]
    lines.append(f"{len(events)} events")
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description="Report on an event log.")
    parser.add_argument("log")
    parser.add_argument("--since", type=parse_duration, help="only events this recent, like 1h30m")
    args = parser.parse_args(argv)
    events = load_events(args.log)
    if args.since is not None:
        cutoff = time.time() - args.since
        events = [event for event in events if event["ts"] >= cutoff]
    print(build_report(events))
    return 0


if __name__ == "__main__":
    sys.exit(main())
