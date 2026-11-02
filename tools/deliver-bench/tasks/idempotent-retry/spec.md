# What the user knows

- A failed send is a connection error or a 5xx status.
- Never retry a 4xx: the receiver has answered, and asking again won't change that.
- Every try of one event keeps the same `Idempotency-Key`, so the receiver can drop repeats. A new event gets a new key.
- Three retries means up to four sends in all.
- A pause between tries is fine but isn't needed.
- When the tries run out, return the last status. If the last try was a connection error, let it raise.
