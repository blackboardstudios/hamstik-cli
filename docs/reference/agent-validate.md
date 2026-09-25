# `hamstik agent validate`

Validate the local agent harness offline: installed skill metadata against the running CLI version, credential-shaped content in the config directory (reported without displaying values), and bundled OpenAPI snapshot freshness when online. Also available as `agent doctor`

Aliases: `doctor`

### `path`

Explicit path to a `SKILL.md` to validate instead of the default installed\-location lookup

Value: `PATH`

### `--offline`

Skip the live OpenAPI snapshot\-freshness comparison and make no network requests

Choices: `true`, `false`


Supports: `--json`, `--no-input`
