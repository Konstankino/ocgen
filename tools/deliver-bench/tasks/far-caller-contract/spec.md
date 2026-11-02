# What the user knows

- A missing key should raise an error whose message names the key. The exception type is the developer's choice.
- Everything that works today has to keep working.
- `run_hour` is optional, if anyone asks: without it the nightly job runs at its default hour.
- `host` and `port` are required.
- The callers may change if they need to.
