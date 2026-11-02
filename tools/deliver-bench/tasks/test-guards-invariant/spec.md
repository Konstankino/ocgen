# What the user knows

- A refund is a negative amount that reduces the balance.
- The balance must never go below zero: a refund larger than the balance is refused with a ValueError, and nothing is recorded.
- A refund that takes the balance to exactly zero is fine.
- A zero amount stays refused.
