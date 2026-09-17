# `hamstik agent skill install`

Install the canonical bundled Agent Skill into a location agents discover. Defaults to the current project's portable `.agents/skills/hamstik/` location; `--global` installs into the user-level portable location. A locally modified installed skill is never silently overwritten; `--force` is required for replacement

### `--global`

Install into the user\-level portable Agent Skills location instead of the default current\-project location

Choices: `true`, `false`

### `--force`

Replace an existing locally modified installed skill

Choices: `true`, `false`


Supports: `--json`, `--no-input`
