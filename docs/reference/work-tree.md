# `hamstik work tree`

Render a work item's parent/child hierarchy with a bounded depth and optional server-reported link annotations. Deep and cyclic structures terminate with an explicit truncation marker

### `key`

Root work item key (e.g. HAM\-42)

Value: `KEY`

### `--depth`

Maximum descendant levels to expand below the root (1 shows direct children only). Deeper structures end with an explicit depth\-limit marker; the value is capped at 10

Default: `3`

### `--links`

Include each node's server\-reported links (`blocks`, `blocked_by`, `relates`) as annotations. These are displayed verbatim and are never used to compute readiness or blocking

Choices: `true`, `false`

### `--max-nodes`

Stop after this many nodes and mark the cut with an explicit truncation marker; the value is capped at 2000

Default: `200`


Supports: `--json`, `--no-input`
