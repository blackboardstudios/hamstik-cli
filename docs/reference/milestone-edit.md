# `hamstik milestone edit`

Edit a milestone

### `id`

Milestone id (UUID)

Value: `ID`

### `--name`

New name

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

New milestone owner: me, a user UUID, a public ID (usr_...), or `none`

### `--target-date`

New target date

### `--clear-target-date`

Clear the target date

Choices: `true`, `false`

### `--reason`

Why the milestone changed (recorded in events)

### `--force`

Bypass revision conflict protection (If\-Match: *)

Choices: `true`, `false`

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
