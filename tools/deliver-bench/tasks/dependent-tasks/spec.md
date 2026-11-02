# What the user knows

- Priority is an integer, and 1 is the most urgent. There's no upper limit to check.
- Task files written before this change have to keep working: a task with no priority counts as priority 3.
- Those files don't need a separate migration step. They can gain the field whenever they're next saved.
- Within one priority, the oldest `created` comes first.
- `next_task` should follow the new order.
