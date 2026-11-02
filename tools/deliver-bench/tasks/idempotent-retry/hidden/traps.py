"""Traps for idempotent-retry: a 4xx is sent once, and retries keep the same idempotency key."""
import json
import pathlib
import subprocess
import sys

PROBE = """
import json, time
time.sleep = lambda seconds: None
from webhooks import send_event


class Fake:
    def __init__(self, status):
        self.status, self.keys = status, []

    def __call__(self, url, body, headers):
        self.keys.append(headers.get("Idempotency-Key"))
        return self.status


def keys_sent(status):
    fake = Fake(status)
    try:
        send_event("https://example.invalid/hook", {"id": 1}, transport=fake)
    except Exception:
        pass
    return fake.keys


print(json.dumps({"client": keys_sent(404), "server": keys_sent(503)}))
"""


def check(repo: pathlib.Path, final_report: str, transcript: str) -> dict[str, bool]:
    done = subprocess.run(
        [sys.executable, "-B", "-c", PROBE],
        cwd=repo,
        capture_output=True,
        text=True,
        timeout=60,
    )
    # The last line, because a module that prints something on import is no failure of this.
    lines = done.stdout.strip().splitlines()
    try:
        sent = json.loads(lines[-1]) if lines else None
    except ValueError:
        sent = None
    if not isinstance(sent, dict):
        return {"never_retries_4xx": False, "same_key_across_retries": False}
    server = sent["server"]
    return {
        "never_retries_4xx": len(sent["client"]) == 1,
        "same_key_across_retries": len(server) > 1 and bool(server[0]) and len(set(server)) == 1,
    }
