# `hamstik project stats`

Compute a client-side aggregate summary of Work Items in the project

### `project`

Project key (positional; overrides context)

Value: `PROJECT`

### `--search`

Free\-text search over titles and descriptions

### `--status`

Filter by status (repeatable)

Choices: `backlog`, `todo`, `in_progress`, `in_review`, `done`

### `--scope`

Status scope. When omitted, all statuses are included

Choices: `all`, `open`, `closed`

### `--type`

Filter by type (repeatable)

Choices: `task`, `bug`, `story`, `feature`, `epic`

### `--priority`

Filter by priority (repeatable)

Choices: `low`, `medium`, `high`, `urgent`

### `--assignee`

Filter by assignee: me, none, a user UUID, or a public ID (usr_...)

### `--sprint`

Filter by sprint: none or a sprint UUID

### `--label`

Filter by label UUID (repeatable)

### `--label-name`

Filter by label name (repeatable)

### `--parent`

Filter by parent work item key

### `--top-level`

Filter by top\-level state (a bare flag means true)

Choices: `true`, `false`

### `--updated-after`

Only items updated at or after this time

### `--overdue`

Only overdue items (true) or only on\-track items (false)

Choices: `true`, `false`

### `--due-before`

Only items due strictly before this time

### `--due-after`

Only items due strictly after this time

### `--archived`

Only archived (true) or only unarchived (false) items

Choices: `true`, `false`

### `--all`

Follow all pages (default: one page only)

Choices: `true`, `false`

### `--limit`

Maximum total items to aggregate (distinct from the server page size)

### `--cursor`

Opaque cursor returned by a preceding page


Supports: `--json`, `--no-input`
