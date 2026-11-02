"""Sends events to a customer's webhook endpoint."""
import json
import urllib.error
import urllib.request
import uuid


def post(url, body, headers):
    """One HTTP POST. Returns the status; a connection problem raises OSError."""
    request = urllib.request.Request(url, data=body.encode("utf-8"), headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code


def send_event(url, event, transport=post):
    """Send one event. Returns the HTTP status."""
    headers = {"Content-Type": "application/json", "Idempotency-Key": str(uuid.uuid4())}
    return transport(url, json.dumps(event), headers)
