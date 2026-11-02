# What the user knows

- Two contacts are the same person when their emails match, ignoring case.
- When contacts collide, the one with the most recent `updated_at` stays. The dates are ISO strings, so comparing them as text works. The others are dropped, not merged.
- Plus-addressing is a different address: `ann+work@example.com` isn't `ann@example.com`. Don't strip the `+` part.
- The function returns a list. Its order doesn't matter.
