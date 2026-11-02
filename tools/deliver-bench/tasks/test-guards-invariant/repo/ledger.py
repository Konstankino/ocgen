"""A customer's account ledger."""


class Ledger:
    def __init__(self):
        self.entries = []

    def add(self, amount):
        """Record a payment."""
        if amount <= 0:
            raise ValueError("amount must be positive")
        self.entries.append(amount)

    def balance(self):
        return sum(self.entries)
