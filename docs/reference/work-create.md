# `hamstik work create`

Create a work item

### `--title`

Work item title

### `--description`

Description text

### `--description-file`

Description source (path, or \- for stdin)

### `--description-editor`

Author the description in $VISUAL/$EDITOR instead of passing text

Choices: `true`, `false`

### `--type`

Work item type

Choices: `task`, `bug`, `story`, `feature`, `epic`

### `--status`

Initial status

Choices: `backlog`, `todo`, `in_progress`, `in_review`, `done`

### `--priority`

Priority

Choices: `low`, `medium`, `high`, `urgent`

### `--assignee`

Assignee user id (or `me`)

### `--sprint`

Sprint id

### `--parent`

Parent work item key (or parent UUID)

### `--story-points`

Story point estimate

### `--due-date`

Due date. Accepts RFC 3339, `YYYY\-MM\-DD`, `today`/`yesterday`/`tomorrow`, or a relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset is read in the host's local time zone

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
