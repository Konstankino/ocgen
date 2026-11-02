"""Sends events to a customer's webhook endpoint."""
import json
import urllib.error
import urllib.request
import uuid

MAX_RETRIES = 3


def post(url, body, headers):
    """One HTTP POST. Returns the status; a connection problem raises OSError."""
    request = urllib.request.Request(url, data=body.encode("utf-8"), headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code


def send_event(url, event, transport=post):
    """Send one event and return the last HTTP status.

    A connection error or a 5xx is retried up to MAX_RETRIES times with the same idempotency key, so
    the receiver can drop repeats. A 4xx is the receiver's answer and is never retried.
    """
    headers = {"Content-Type": "application/json", "Idempotency-Key": str(uuid.uuid4())}
    body = json.dumps(event)
    for retries_left in range(MAX_RETRIES, -1, -1):
        try:
            status = transport(url, body, headers)
        except OSError:
            if not retries_left:
                raise
            continue
        if status < 500 or not retries_left:
            return status
