# `hamstik work diff`

Diff a work item against a recorded local baseline (or a server revision reference), showing per-field before/after changes (CLI-56)

### `key`

Work item key

Value: `KEY`

### `--save-baseline`

Record the item's current state as the new local baseline instead of diffing. Baselines are per\-user files under the config directory (`baselines/<org>/<project>/<KEY>.json`), contain no credentials, and are replaced only by another `\-\-save\-baseline`; a diff never mutates the baseline

Choices: `true`, `false`

### `--since`

Compare against this server\-reported revision (the ETag value) instead of the stored baseline. The Public API v1 contract exposes no Work Item revision snapshots, so this mode reports the revision delta plus the server\-reported field changes visible in the item's activity feed, with explicit partial coverage — it never fabricates a diff

### `--fail-on-change`

Exit with the conflict exit code (6) when differences exist instead of 0; unchanged comparisons always exit 0 in every mode

Choices: `true`, `false`


Supports: `--json`, `--no-input`
