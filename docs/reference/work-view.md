# `hamstik work view`

View one or more work items

View one or more work items.

A single key keeps the regular single-item output. Two or more keys (or `--file`, `-` for stdin; up to 500 keys) fetch items in one invocation with bounded concurrency and emit exactly one JSON envelope in input order: `{"items": [{"key", "status": "ok", "item", plus requested "comments"/"activity"/"links" sections} | {"key", "status": "error", "error"}], "failures": N, "total": N}`. A missing or forbidden item never aborts the batch; the exit code is the most severe per-item exit code (0 when every item succeeded).

### `keys`

Work item keys (e.g. HAM\-42). Accepts multiple keys for one batch read (up to 500 per invocation)

Value: `KEY`

### `--file`

Read keys from a file (`\-` for stdin), one key per line. Blank lines and lines starting with `#` are ignored. Up to 500 keys; input is capped at 1 MiB

### `--comments`

Maximum comments included per item (0 omits the section). Batch reads only (two or more keys); use `work context` for one item with sections

Default: `0`

### `--activity`

Maximum activity events included per item (0 omits the section). Batch reads only (two or more keys); use `work context` for one item with sections

Default: `0`

### `--links`

Maximum links included per item (0 omits the section). Batch reads only (two or more keys); use `work context` for one item with sections

Default: `0`

### `--compact`

Omit long text bodies (description, comment bodies) — each replaced by an explicit truncated marker

Choices: `true`, `false`


Supports: `--json`, `--no-input`
