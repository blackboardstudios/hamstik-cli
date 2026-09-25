# `hamstik work await`

Wait for a work item to reach a server-reported condition

Wait for a work item to reach a server-reported condition.

`await` blocks until a single condition holds and then exits. To follow an item's activity and comments over time instead, use `work watch`.

### `key`

Work item key (e.g. HAM\-42)

Value: `KEY`

### `--status`

Target status to wait for (repeatable; all specified conditions must match). When omitted, the command returns immediately once the item is reachable (any status satisfies the condition)

Choices: `backlog`, `todo`, `in_progress`, `in_review`, `done`

### `--timeout`

Maximum time to wait (e.g. 10m, 1h30m). Defaults to 5m; upper\-bounded at 1h to prevent accidental infinite waits

Default: `5m`


Supports: `--json`, `--no-input`
