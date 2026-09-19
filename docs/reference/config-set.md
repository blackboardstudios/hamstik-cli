# `hamstik config set`

Set a configuration value

### `key`

Configuration key: `profile`, `organization`, `project`, `editor`, `pager`, `output`, `git_branch_template`, or `audit_log`

Value: `KEY`

### `value`

New value for the key: `true`/`false` for `audit_log`, an existing profile name for `profile`, a defined output mode (`human`, `json`, `jsonl`, `tsv`, `quiet`) for `output`. Credentials are never accepted: they live in the OS credential store or `HAMSTIK_TOKEN`

Value: `VALUE`


Supports: `--json`, `--no-input`
