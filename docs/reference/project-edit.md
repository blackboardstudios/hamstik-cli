# `hamstik project edit`

Edit a project (Organization administrators only)

### `key`

Project key

Value: `KEY`

### `--name`

New name

### `--description`

New description text

### `--description-file`

New description source (path, or \- for stdin)

### `--description-editor`

Author the description in $VISUAL/$EDITOR instead of passing text

Choices: `true`, `false`

### `--color`

New display color as a hex string

### `--clear-description`

Clear the description

Choices: `true`, `false`

### `--force`

Bypass revision conflict protection (If\-Match: *)

Choices: `true`, `false`

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
