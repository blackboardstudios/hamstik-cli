# `hamstik milestone transition`

Transition a milestone to a target state

### `id`

Milestone id (UUID)

Value: `ID`

### `target`

Target state

Value: `TARGET`

Choices: `planned`, `in_progress`, `completed`, `archived`

### `--reason`

Why the milestone moved

### `--force`

Bypass revision conflict protection (If\-Match: *)

Choices: `true`, `false`

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
