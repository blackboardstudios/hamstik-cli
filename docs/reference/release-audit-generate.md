# `hamstik release audit generate`

Generate a frozen dossier or UTC change register

### `--kind`

Package kind: dossier (one release) or register (date range)

Choices: `dossier`, `register`

### `--release`

Release version id (required for dossier packages)

### `--from`

Range start (required for register packages)

### `--through`

Range end, inclusive (required for register packages)

### `--project`

Project key (overrides context)

### `--idempotency-key`

Explicit idempotency key


Supports: `--json`, `--no-input`
