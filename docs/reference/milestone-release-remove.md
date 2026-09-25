# `hamstik milestone release remove`

Remove a release version from the milestone

### `id`

Milestone id (UUID)

Value: `ID`

### `release_id`

Release version id (UUID)

Value: `RELEASE_ID`

### `--confirm-remove`

Confirm removing the release and its milestone progress

Choices: `true`, `false`

### `--reason`

Why the release was removed

### `--force`

Bypass revision conflict protection (If\-Match: *)

Choices: `true`, `false`

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
