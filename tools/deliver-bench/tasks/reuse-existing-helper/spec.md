# What the user knows

- `--since` takes durations the way the team writes them everywhere else: `2h`, `1h30m`, `90s`, `2D`. People type both cases.
- It keeps the events newer than now minus the duration. An event exactly on the cutoff can go either way.
- A value that isn't a duration is an error and the command exits non-zero.
- Without `--since` the report stays as it is.
