"""A customer's account ledger."""


class Ledger:
    def __init__(self):
        self.entries = []

    def add(self, amount):
        """Record a payment, or a refund as a negative amount. The balance can't go below zero."""
        if amount == 0:
            raise ValueError("amount must not be zero")
        if self.balance() + amount < 0:
            raise ValueError("a refund can't exceed the balance")
        self.entries.append(amount)

    def balance(self):
        return sum(self.entries)
