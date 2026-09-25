# `hamstik milestone create`

Create an Organization Milestone

### `--name`

Milestone name

### `--description`

Description text

### `--description-file`

Description source (path, or \- for stdin)

### `--description-editor`

Author the description in $VISUAL/$EDITOR instead of passing text

Choices: `true`, `false`

### `--owner`

Milestone owner: me, a user UUID, or a public ID (usr_...)

### `--target-date`

Target date. Accepts RFC 3339, `YYYY\-MM\-DD`, `today`/`yesterday`/`tomorrow`, or a relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset is read in the host's local time zone

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
