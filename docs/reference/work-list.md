# `hamstik work list`

List work items

### `--search`

Free\-text search

### `--status`

Filter by status (repeatable)

Choices: `backlog`, `todo`, `in_progress`, `in_review`, `done`

### `--scope`

Status scope

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

Only items updated at/after this RFC 3339 timestamp

### `--overdue`

Only overdue items (true) or only on\-track items (false)

Choices: `true`, `false`

### `--due-before`

Only items due strictly before this RFC 3339 timestamp

### `--due-after`

Only items due strictly after this RFC 3339 timestamp

### `--sort`

Result ordering: updated, dueDate, priority, or rank

Choices: `updated`, `dueDate`, `priority`, `rank`

### `--archived`

Only archived (true) or only unarchived (false) items; when omitted, unarchived items are listed. Listing both states in one result requires separate queries

Choices: `true`, `false`

### `--fields`

Comma\-separated summary fields; an empty value selects all fields

### `--mine`

Shorthand for \-\-assignee me

Choices: `true`, `false`

### `--limit`

Maximum items per page (endpoint maximum is 100 or 200)

### `--cursor`

Opaque continuation cursor

### `--all`

Follow all pages

Choices: `true`, `false`


Supports: `--json`, `--no-input`
