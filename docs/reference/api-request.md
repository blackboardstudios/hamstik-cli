# `hamstik api request`

Call a documented Public API v1 route through the CLI

### `path`

The Public API v1 path (must start with `/api/v1/`)

Value: `PATH`

### `--method`

HTTP method (GET is the default; the route's documented methods apply)

Choices: `GET`, `POST`, `PATCH`, `PUT`, `DELETE`

Default: `GET`

### `--body-file`

Read the JSON request body from this file; `\-` reads stdin

### `--field`

Structured JSON field as `key=value`; repeatable. Value is parsed as JSON when it parses, else a JSON string

### `--query`

Query parameter as `key=value`; repeatable (allowlisted, ordered)

### `--header`

Header override as `Name: value`; restricted to a safe allowlist (`Accept`, `Content\-Type`, `If\-Match`). `Authorization` and any credential\-bearing header are rejected

### `--idempotency-key`

Idempotency key override; mutations get a fresh strong key otherwise (generated once and reused across internal retries)


Supports: `--json`, `--no-input`
