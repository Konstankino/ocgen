import json
import unittest

from webhooks import send_event


class SendEvent(unittest.TestCase):
    def test_it_posts_the_event_and_returns_the_status(self):
        seen = []

        def transport(url, body, headers):
            seen.append((url, json.loads(body), headers))
            return 200

        self.assertEqual(send_event("https://example.invalid/hook", {"id": 1}, transport=transport), 200)
        url, body, headers = seen[0]
        self.assertEqual(url, "https://example.invalid/hook")
        self.assertEqual(body, {"id": 1})
        self.assertEqual(headers["Content-Type"], "application/json")
