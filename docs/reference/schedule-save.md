# `hamstik schedule save`

Save (or replace) a schedule definition

Save (or replace) a schedule definition.

The command to run is given after `--`, for example: `hamstik schedule save weekly -- work export --query '...' --format csv --output weekly.csv`.

### `name`

Schedule name (letters, digits, `\-`, `_`; 1–64 chars)

Value: `NAME`

### `--force`

Replace an existing definition of the same name

Choices: `true`, `false`

### `--description`

Optional human description of the snapshot

### `command`

The `hamstik` arguments to run, after `\-\-`

Value: `COMMAND`


Supports: `--json`, `--no-input`
