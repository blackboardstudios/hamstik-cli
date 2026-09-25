# `hamstik release transition`

Transition a release to a target state

### `id`

Release version id (UUID)

Value: `ID`

### `target`

Target state

Value: `TARGET`

Choices: `planned`, `in_progress`, `released`, `archived`

### `--confirm-incomplete-scope`

Confirm releasing a scope that still contains incomplete Work Items

Choices: `true`, `false`

### `--reason`

Why the release moved

### `--force`

Bypass revision conflict protection (If\-Match: *)

Choices: `true`, `false`

### `--project`

Project key (overrides context)

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
