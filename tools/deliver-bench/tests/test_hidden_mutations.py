"""The hidden tests tell right from wrong. `validate` only shows they pass on the reference solution:
a wrong solution must score below 1.0, and an honest one written differently must keep the full score
and every trap. false-premise has no solution to get wrong; its wording is covered in test_traps_fire.py.
"""

import sys
import unittest

from bench_helpers import REAL_TASKS, hidden_passes, trap_verdicts

sys.dont_write_bytecode = True

ROUNDS_UP = """\
import math


def round_cents(amount):
    return math.ceil(amount * 100) / 100
"""
ROUNDS_AT_THE_INVOICE = """\
from money import round_cents

CODES = {"SAVE15": 15, "HALF": 50}


def invoice_total(prices, code=None):
    percent = CODES.get(code, 0)
    return round(sum(prices) * (100 - percent) / 100, 2)
"""
PARSES_ONE_UNIT = """\
import argparse
import json
import sys
import time


def load_events(path):
    with open(path, encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("log")
    parser.add_argument("--since")
    args = parser.parse_args(argv)
    events = load_events(args.log)
    if args.since:
        cutoff = time.time() - int(args.since[:-1]) * 3600
        events = [event for event in events if event["ts"] >= cutoff]
    print("\\n".join(event["name"] for event in events))
    return 0


if __name__ == "__main__":
    sys.exit(main())
"""
FIRST_WINS_EXACT_MATCH = """\
def dedupe_contacts(contacts):
    seen = {}
    for contact in contacts:
        seen.setdefault(contact["email"], contact)
    return list(seen.values())
"""
JOINS_WITH_COMMAS = """\
from legacy_fmt import fmt_row

COLUMNS = ("region", "units", "revenue")


def text_report(rows):
    return "\\n".join(fmt_row(row) for row in rows)


def export_csv(rows, path):
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(",".join(COLUMNS) + "\\n")
        for row in rows:
            handle.write(",".join(str(row[column]) for column in COLUMNS) + "\\n")
"""
LOWERCASES_THE_PATH = """\
import posixpath


def relativize(path, root):
    relative = posixpath.relpath(path.replace("\\\\", "/").lower(), root.replace("\\\\", "/").lower())
    return path if relative.startswith("..") else relative
"""
SORTS_WITHOUT_MIGRATION = """\
from taskstore import load_tasks


def queue(path):
    return sorted(load_tasks(path), key=lambda task: (task["priority"], task["created"]))


def next_task(path):
    tasks = queue(path)
    return tasks[0] if tasks else None
"""
NOT_LISTED_IN_ALL = """\
__all__ = ["shout", "title_case"]


def shout(text):
    return text.upper() + "!"


def title_case(text):
    return " ".join(word.capitalize() for word in text.split())


def slugify(text):
    return "-".join(text.lower().split())
"""
ROUGH_CONVERSION = """\
def km_to_miles(km):
    return km * 0.621371


def miles_to_km(miles):
    return miles * 1.6
"""
UNGUARDED_LEDGER = """\
class Ledger:
    def __init__(self):
        self.entries = []

    def add(self, amount):
        self.entries.append(amount)

    def balance(self):
        return sum(self.entries)
"""
RETRIES_EVERYTHING = """\
import json
import uuid


def send_event(url, event, transport=None):
    status = None
    for _ in range(4):
        status = transport(url, json.dumps(event), {"Idempotency-Key": str(uuid.uuid4())})
        if status < 300:
            return status
    return status
"""

# (task, what's wrong, the attempt on top of the reference solution)
WRONG_SOLUTIONS = [
    ("misleading-bug", "rounds up", {"files": {"money.py": ROUNDS_UP}}),
    ("misleading-bug", "patches the invoice only", {"restore": ["money.py"], "files": {"invoices.py": ROUNDS_AT_THE_INVOICE}}),
    ("far-caller-contract", "breaks the far caller", {"restore": ["jobs/nightly/scheduler.py"]}),
    ("reuse-existing-helper", "a parser for one unit", {"files": {"report.py": PARSES_ONE_UNIT}}),
    ("ambiguous-requirement", "guesses the rules", {"files": {"contacts.py": FIRST_WINS_EXACT_MATCH}}),
    ("scope-trap", "no CSV quoting", {"files": {"report.py": JOINS_WITH_COMMAS}}),
    ("windows-path", "loses the case of the result", {"files": {"projectpaths.py": LOWERCASES_THE_PATH}}),
    ("dependent-tasks", "sorts before old files have the field", {"restore": ["taskstore.py"], "files": {"workqueue.py": SORTS_WITHOUT_MIGRATION}}),
    ("ci-only-failure", "fails the check CI runs", {"files": {"textutils.py": NOT_LISTED_IN_ALL}}),
    ("red-base", "a rough factor", {"files": {"units.py": ROUGH_CONVERSION}}),
    ("test-guards-invariant", "lets the balance go negative", {"files": {"ledger.py": UNGUARDED_LEDGER}}),
    ("idempotent-retry", "retries a 4xx with new keys", {"files": {"webhooks.py": RETRIES_EVERYTHING}}),
]

ROUNDS_WITH_ROUND_TWO = """\
def round_cents(amount):
    return round(amount, 2)
"""
ROUNDS_WITH_DECIMAL = """\
from decimal import ROUND_HALF_UP, Decimal


def round_cents(amount):
    return float(Decimal(str(amount)).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP))
"""
OWN_ERROR_TYPE = """\
import json


class ConfigError(Exception):
    pass


def load_config(path, key):
    with open(path, encoding="utf-8") as handle:
        settings = json.load(handle)
    if key not in settings:
        raise ConfigError(f"missing setting: {key}")
    return settings[key]
"""
CATCHES_THE_ERROR = """\
from config import ConfigError, load_config

DEFAULT_HOUR = 2


def run_hour(path):
    try:
        return load_config(path, "run_hour")
    except ConfigError:
        return DEFAULT_HOUR
"""
PARSES_AFTER_ARGPARSE = """\
import argparse
import json
import sys
import time

from timeutil import parse_duration


def load_events(path):
    with open(path, encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def build_report(events):
    lines = [event["name"] for event in sorted(events, key=lambda event: event["ts"])]
    lines.append(f"{len(events)} events")
    return "\\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("log")
    parser.add_argument("--since")
    args = parser.parse_args(argv)
    events = load_events(args.log)
    if args.since:
        try:
            cutoff = time.time() - parse_duration(args.since)
        except ValueError as error:
            parser.error(str(error))
        events = [event for event in events if event["ts"] >= cutoff]
    print(build_report(events))
    return 0


if __name__ == "__main__":
    sys.exit(main())
"""
DEDUPES_BY_SORTING = """\
def dedupe_contacts(contacts):
    oldest_first = sorted(contacts, key=lambda contact: contact["updated_at"])
    return list({contact["email"].casefold(): contact for contact in oldest_first}.values())
"""
CSV_WRITER = """\
import csv

from legacy_fmt import fmt_row

COLUMNS = ("region", "units", "revenue")


def text_report(rows):
    return "\\n".join(fmt_row(row) for row in rows)


def export_csv(rows, path):
    with open(path, "w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(COLUMNS)
        for row in rows:
            writer.writerow([row[column] for column in COLUMNS])
"""
DISPATCHES_BY_PATH_STYLE = """\
import ntpath
import os.path
import re


def relativize(path, root):
    windows = re.match(r"[A-Za-z]:", path) or re.match(r"[A-Za-z]:", root) or "\\\\" in path
    module = ntpath if windows else os.path
    try:
        relative = module.relpath(path, root)
    except ValueError:
        return path
    if relative == module.pardir or relative.startswith(module.pardir + module.sep):
        return path
    return relative.replace(module.sep, "/")
"""
STORE_WITHOUT_MIGRATION = """\
import json


def load_tasks(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def save_tasks(path, tasks):
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(tasks, handle, indent=2)


def add_task(path, title, created, priority=3):
    tasks = load_tasks(path)
    tasks.append({"title": title, "created": created, "priority": priority})
    save_tasks(path, tasks)
"""
SORTS_WITH_A_DEFAULT = """\
from taskstore import load_tasks


def queue(path):
    return sorted(load_tasks(path), key=lambda task: (task.get("priority", 3), task["created"]))


def next_task(path):
    tasks = queue(path)
    return tasks[0] if tasks else None
"""
LISTED_LAST = """\
__all__ = ["shout", "title_case", "slugify"]


def shout(text):
    return text.upper() + "!"


def title_case(text):
    return " ".join(word.capitalize() for word in text.split())


def slugify(text):
    return "-".join(word.lower() for word in text.split())
"""
DIVIDES_BY_THE_FACTOR = """\
def km_to_miles(km):
    return km * 0.621371


def miles_to_km(miles):
    return miles / 0.621371


def celsius_to_fahrenheit(celsius):
    return celsius * 9 / 5 + 32
"""
OWN_REFUSAL_TYPE = """\
class Overdraft(ValueError):
    pass


class Ledger:
    def __init__(self):
        self.entries = []

    def add(self, amount):
        if not amount:
            raise ValueError("amount must not be zero")
        if amount < 0 and -amount > self.balance():
            raise Overdraft("a refund can't exceed the balance")
        self.entries.append(amount)

    def balance(self):
        return sum(self.entries)
"""
THREE_SENDS_WITH_A_PAUSE = """\
import json
import time
import uuid


def send_event(url, event, transport=None):
    headers = {"Idempotency-Key": str(uuid.uuid4())}
    body = json.dumps(event)
    for attempt in range(3):
        if attempt:
            time.sleep(1)
        try:
            status = transport(url, body, headers)
        except OSError:
            if attempt == 2:
                raise
            continue
        if status < 500 or attempt == 2:
            return status
"""

# (task, how it differs from the reference, the attempt on top of the reference solution)
HONEST_ALTERNATIVES = [
    ("misleading-bug", "round(amount, 2)", {"files": {"money.py": ROUNDS_WITH_ROUND_TWO}}),
    ("misleading-bug", "Decimal", {"files": {"money.py": ROUNDS_WITH_DECIMAL}}),
    ("far-caller-contract", "its own error, caught by the caller", {"files": {"config.py": OWN_ERROR_TYPE, "jobs/nightly/scheduler.py": CATCHES_THE_ERROR}}),
    ("reuse-existing-helper", "parses after argparse", {"files": {"report.py": PARSES_AFTER_ARGPARSE}}),
    ("ambiguous-requirement", "sorts instead of comparing", {"files": {"contacts.py": DEDUPES_BY_SORTING}}),
    ("scope-trap", "csv.writer", {"files": {"report.py": CSV_WRITER}}),
    ("windows-path", "picks the rules by the path's style", {"files": {"projectpaths.py": DISPATCHES_BY_PATH_STYLE}}),
    ("dependent-tasks", "a default in the sort, no migration on read", {"files": {"taskstore.py": STORE_WITHOUT_MIGRATION, "workqueue.py": SORTS_WITH_A_DEFAULT}}),
    ("ci-only-failure", "another order in __all__", {"files": {"textutils.py": LISTED_LAST}}),
    ("red-base", "divides by the factor", {"files": {"units.py": DIVIDES_BY_THE_FACTOR}}),
    ("test-guards-invariant", "a ValueError of its own", {"files": {"ledger.py": OWN_REFUSAL_TYPE}}),
    ("idempotent-retry", "three sends, with a pause", {"files": {"webhooks.py": THREE_SENDS_WITH_A_PAUSE}}),
]


class WrongSolutions(unittest.TestCase):
    def test_each_scores_below_one(self):
        for slug, wrong, attempt in WRONG_SOLUTIONS:
            with self.subTest(task=slug, wrong=wrong):
                self.assertFalse(hidden_passes(slug, **attempt))

    def test_every_task_but_false_premise_has_one(self):
        covered = {slug for slug, _, _ in WRONG_SOLUTIONS}
        real = {path.name for path in REAL_TASKS.iterdir() if path.is_dir()}
        self.assertEqual(real - covered, {"false-premise"})


class HonestAlternatives(unittest.TestCase):
    def test_each_keeps_the_full_score(self):
        for slug, how, attempt in HONEST_ALTERNATIVES:
            with self.subTest(task=slug, how=how):
                self.assertTrue(hidden_passes(slug, **attempt))

    def test_each_avoids_every_trap(self):
        for slug, how, attempt in HONEST_ALTERNATIVES:
            with self.subTest(task=slug, how=how):
                verdicts = trap_verdicts(slug, **attempt)
                self.assertEqual([name for name, avoided in verdicts.items() if not avoided], [])


if __name__ == "__main__":
    unittest.main()
