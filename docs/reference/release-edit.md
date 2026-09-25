# `hamstik release edit`

Edit a release version

### `id`

Release version id (UUID)

Value: `ID`

### `--name`

New name

### `--display-version`

New display version label

### `--clear-display-version`

Clear the display version label

Choices: `true`, `false`

### `--description`

New description text

### `--description-file`

New description source (path, or \- for stdin)

### `--description-editor`

Author the description in $VISUAL/$EDITOR instead of passing text

Choices: `true`, `false`

### `--clear-description`

Clear the description

Choices: `true`, `false`

### `--owner`

New release owner: me, a user UUID, a public ID (usr_...), or `none`

### `--target-date`

New target date

### `--clear-target-date`

Clear the target date

Choices: `true`, `false`

### `--release-date`

New release date

### `--clear-release-date`

Clear the release date

Choices: `true`, `false`

### `--force`

Bypass revision conflict protection (If\-Match: *)

Choices: `true`, `false`

### `--project`

Project key (overrides context)

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
