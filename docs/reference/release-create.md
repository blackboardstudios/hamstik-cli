# `hamstik release create`

Create a release version

### `--name`

Release name

### `--display-version`

Separate display version label

### `--description`

Description text

### `--description-file`

Description source (path, or \- for stdin)

### `--description-editor`

Author the description in $VISUAL/$EDITOR instead of passing text

Choices: `true`, `false`

### `--owner`

Release owner: me, a user UUID, or a public ID (usr_...)

### `--target-date`

Target date. Accepts RFC 3339, `YYYY\-MM\-DD`, `today`/`yesterday`/`tomorrow`, or a relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset is read in the host's local time zone

### `--release-date`

Release date. Accepts RFC 3339, `YYYY\-MM\-DD`, `today`/`yesterday`/`tomorrow`, or a relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset is read in the host's local time zone

### `--project`

Project key (overrides context)

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
