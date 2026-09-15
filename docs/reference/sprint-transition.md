# `hamstik sprint transition`

Transition a sprint to a target state

### `id`

Sprint id (UUID)

Value: `ID`

### `target`

Target state

Value: `TARGET`

Choices: `active`, `done`

### `--move-to-backlog`

Move unfinished work items back to the backlog when completing

Choices: `true`, `false`

### `--move-to-sprint`

Move unfinished work items to this future sprint

### `--force`

Bypass revision conflict protection (If\-Match: *)

Choices: `true`, `false`

### `--project`

Project key (overrides context)

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
