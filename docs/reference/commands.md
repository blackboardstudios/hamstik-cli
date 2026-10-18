# `hamstik commands`

Print the machine-readable command manifest derived from the real command tree, or the copy-pasteable API cookbook with `--cookbook`

### `--manifest-format`

Output format for the manifest

Choices: `json`

Default: `json`

### `--cookbook`

Print the copy\-pasteable API cookbook derived from the bundled Agent Skill instead of the command manifest

Choices: `true`, `false`

### `--exit-codes`

Print the stable exit\-code reference (CLI\-73) instead of the command manifest: a human table by default, the versioned `exitCodesVersion: 1` document under `\-\-json`/`\-\-format json`

Choices: `true`, `false`


Supports: `--json`, `--no-input`
