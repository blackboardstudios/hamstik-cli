# `hamstik work bulk from-csv`

Convert a CSV file into the `work bulk` operations JSON array

Convert a CSV file into the `work bulk` operations JSON array.

The first row is a header; every later row becomes one operation, so the emitted JSON array can be piped straight into `work bulk create|update --operations-file -`. Fields use RFC 4180 quoting (comma-separated, `"` quoting, `""` for an embedded quote); free-text values are not trimmed, while enum and identifier cells are interpreted (and emitted) trimmed. `--project` supplies `projectKey` for every operation.

`--op update` columns: `workItemKey` (required), `revision`, `title`, `description`, `type`, `priority`, `assignee`, `sprint`, `parent`, `storyPoints`, `dueDate` (the same date/time expressions as `--due-date`). At least one change column is required. An empty cell means the field is omitted.

`--op create` column: `title` (required). The frozen Public API v1 bulk create envelope carries only `projectKey` and `title`, so no other column is accepted.

Unknown columns, bad enum spellings, and malformed rows fail locally with the CSV row number and column name before any network call.

### `file`

CSV file (path, or \- for stdin). The first row must be a header

Value: `FILE`

### `--op`

Operation kind the rows convert to

Choices: `create`, `update`

### `--project`

Project key applied to every converted operation (defaults to the global `\-\-project`, then normal project selection such as HAMSTIK_PROJECT or `hamstik project use`)

### ``-o`, --output`

Write the operations JSON to a file instead of stdout


Supports: `--json`, `--no-input`
