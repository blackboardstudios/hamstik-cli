# `hamstik user work`

List work items involving a user

### `public_id`

The user's public ID (usr_...)

Value: `PUBLIC_ID`

### `--involvement`

Restrict to involvement kind (repeatable)

Choices: `assigned`, `created`, `commented`

### `--org`

Filter by organization slug (repeatable)

### `--project`

Filter by project key (repeatable)

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

### `--sprint`

Filter by sprint id or `none`

### `--label`

Filter by label id (repeatable)

### `--label-name`

Filter by label name (repeatable)

### `--parent`

Filter by parent Work Item key

### `--top-level`

Filter by top\-level state (a bare flag means true)

Choices: `true`, `false`

### `--updated-after`

Only items updated after this RFC 3339 timestamp

### `--overdue`

Filter to overdue/on\-track Work Items

Choices: `true`, `false`

### `--due-before`

Only items due strictly before this RFC 3339 timestamp

### `--due-after`

Only items due strictly after this RFC 3339 timestamp

### `--sort`

Result ordering

Choices: `updated`, `dueDate`, `priority`, `rank`

### `--archived`

Only archived (true) or only unarchived (false) Work Items; when omitted, unarchived Work Items are listed. Listing both states in one result requires separate queries

Choices: `true`, `false`

### `--fields`

Comma\-separated sparse summary fields

### `--limit`

Maximum items per page (endpoint maximum is 100 or 200)

### `--cursor`

Opaque continuation cursor

### `--all`

Follow all pages

Choices: `true`, `false`


Supports: `--json`, `--no-input`
