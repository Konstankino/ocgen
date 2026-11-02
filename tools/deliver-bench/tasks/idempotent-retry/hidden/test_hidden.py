"""idempotent-retry: what is retried, how often, and what a retry keeps the same."""
import json
import unittest
import urllib.error
from unittest import mock

import webhooks


class Receiver:
    """A fake transport: gives each answer in turn, then repeats the last. An exception is raised."""

    def __init__(self, *answers):
        self.answers = list(answers)
        self.headers = []
        self.bodies = []

    def __call__(self, url, body, headers):
        self.headers.append(dict(headers))
        self.bodies.append(body)
        answer = self.answers.pop(0) if len(self.answers) > 1 else self.answers[0]
        if isinstance(answer, Exception):
            raise answer
        return answer

    def keys(self):
        return [headers.get("Idempotency-Key") for headers in self.headers]


class Case(unittest.TestCase):
    def setUp(self):
        sleeping = mock.patch("time.sleep")
        sleeping.start()
        self.addCleanup(sleeping.stop)

    def send(self, receiver):
        return webhooks.send_event("https://example.invalid/hook", {"id": 7}, transport=receiver)

    def send_until_it_gives_up(self, receiver):
        try:
            self.send(receiver)
        except Exception:
            pass


class Retries(Case):
    def test_a_success_sends_once(self):
        receiver = Receiver(200)
        self.assertEqual(self.send(receiver), 200)
        self.assertEqual(len(receiver.headers), 1)

    def test_a_server_error_is_retried_until_it_works(self):
        receiver = Receiver(500, 503, 200)
        self.assertEqual(self.send(receiver), 200)
        self.assertEqual(len(receiver.headers), 3)

    def test_a_connection_error_is_retried(self):
        receiver = Receiver(urllib.error.URLError("refused"), 200)
        self.assertEqual(self.send(receiver), 200)
        self.assertEqual(len(receiver.headers), 2)

    def test_it_gives_up(self):
        # "Up to 3 times" reads as 3 sends or as 3 retries. That isn't what this task measures.
        receiver = Receiver(500)
        self.send_until_it_gives_up(receiver)
        self.assertIn(len(receiver.headers), (3, 4))

    def test_it_returns_the_last_status_when_the_tries_run_out(self):
        self.assertEqual(self.send(Receiver(500)), 500)

    def test_a_connection_error_on_the_last_try_raises(self):
        with self.assertRaises(OSError):
            self.send(Receiver(urllib.error.URLError("refused")))


class NeverRetried(Case):
    def test_a_bad_request(self):
        receiver = Receiver(400, 200)
        self.send_until_it_gives_up(receiver)
        self.assertEqual(len(receiver.headers), 1)

    def test_not_found(self):
        receiver = Receiver(404, 200)
        self.send_until_it_gives_up(receiver)
        self.assertEqual(len(receiver.headers), 1)

    def test_a_client_error_after_a_server_error(self):
        receiver = Receiver(500, 404, 200)
        self.send_until_it_gives_up(receiver)
        self.assertEqual(len(receiver.headers), 2)


class IdempotencyKey(Case):
    def test_every_try_keeps_the_same_key(self):
        receiver = Receiver(500, 503, 200)
        self.send(receiver)
        keys = receiver.keys()
        self.assertEqual(len(keys), 3)
        self.assertTrue(keys[0])
        self.assertEqual(len(set(keys)), 1)

    def test_a_new_event_gets_a_new_key(self):
        first, second = Receiver(200), Receiver(200)
        self.send(first)
        self.send(second)
        self.assertNotEqual(first.keys(), second.keys())

    def test_every_try_sends_the_event(self):
        receiver = Receiver(500, 200)
        self.send(receiver)
        self.assertEqual([json.loads(body) for body in receiver.bodies], [{"id": 7}, {"id": 7}])
