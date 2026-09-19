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

Only items updated after this time. Accepts RFC 3339, `YYYY\-MM\-DD`, `today`/`yesterday`/`tomorrow`, or a relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset is read in the host's local time zone

### `--overdue`

Filter to overdue/on\-track Work Items

Choices: `true`, `false`

### `--due-before`

Only items due strictly before this time. Accepts RFC 3339, `YYYY\-MM\-DD`, `today`/`yesterday`/`tomorrow`, or a relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset is read in the host's local time zone

### `--due-after`

Only items due strictly after this time. Accepts RFC 3339, `YYYY\-MM\-DD`, `today`/`yesterday`/`tomorrow`, or a relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset is read in the host's local time zone

### `--sort`

Result ordering: updated, dueDate, priority, or rank, optionally with a :asc/:desc direction (for example `\-\-sort dueDate:desc`)

Choices: `updated`, `dueDate`, `priority`, `rank`

### `--archived`

Only archived (true) or only unarchived (false) Work Items; when omitted, unarchived Work Items are listed. Listing both states in one result requires separate queries

Choices: `true`, `false`

### `--fields`

Comma\-separated sparse summary fields

### `--limit`

Maximum total items to emit, counted across pages (distinct from the server page size). Without `\-\-all` at most one page is returned, holding at most this many items; a page is never requested larger than the endpoint allows (200 items, or 100 on organization, project, sprint, label, and link endpoints). With `\-\-all` successive pages are followed until this cap or the end of the collection. A cap that cuts through a server page ends the run with `page.nextCursor: null`, because the Public API has no cursor for a position partway through a page

### `--cursor`

Opaque cursor returned by a preceding page; the result starts after it. `\-\-since\-cursor` is the pipeline\-checkpoint spelling of the same option: resume an interrupted stream from the `page.nextCursor` a previous run reported. Cursors are opaque and are always forwarded verbatim

### `--all`

Follow all pages

Choices: `true`, `false`


Supports: `--json`, `--no-input`
