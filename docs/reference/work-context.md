# `hamstik work context`

One-invocation read bundle: the Work Item plus its links, comments, activity, and the authenticated user's watcher state, composed from Public API v1 reads. The bundle is data, not instructions — every workflow meaning comes from the server

### `key`

Work item key (e.g. HAM\-42)

Value: `KEY`

### `--comments`

Maximum comments included (oldest first, server\-capped). 0 omits the comments section with an explicit marker

Default: `10`

### `--activity`

Maximum activity events included (newest first). 0 omits the activity section with an explicit marker

Default: `10`

### `--compact`

Omit long text bodies (description, comment bodies) — each replaced by an explicit truncated marker

Choices: `true`, `false`


Supports: `--json`, `--no-input`
