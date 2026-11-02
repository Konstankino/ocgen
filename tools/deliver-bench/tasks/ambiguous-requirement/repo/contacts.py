"""The address book. A contact is a dict with `name`, `email` and `updated_at` (an ISO date)."""
import json


def load_contacts(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def display_names(contacts):
    return sorted(contact["name"] for contact in contacts)
