# `hamstik squeakql save`

Save an expression as a named query for reuse

### `name`

Saved query name (letters, digits, `\-`, `_`; 1–64 chars)

Value: `NAME`

### `query`

SqueakQL expression (inline), or provide it with \-\-file/stdin

Value: `QUERY`

### `--file`

Read the expression from a file (`\-` for stdin) instead of inline

### `--force`

Replace an existing saved query of the same name

Choices: `true`, `false`


Supports: `--json`, `--no-input`
