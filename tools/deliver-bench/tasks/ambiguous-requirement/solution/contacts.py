"""The address book. A contact is a dict with `name`, `email` and `updated_at` (an ISO date)."""
import json


def load_contacts(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def display_names(contacts):
    return sorted(contact["name"] for contact in contacts)


def dedupe_contacts(contacts):
    """One contact per email, ignoring case; the most recent `updated_at` stays."""
    latest = {}
    for contact in contacts:
        key = contact["email"].lower()
        if key not in latest or contact["updated_at"] > latest[key]["updated_at"]:
            latest[key] = contact
    return list(latest.values())
