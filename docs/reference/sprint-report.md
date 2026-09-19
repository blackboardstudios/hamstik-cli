# `hamstik sprint report`

Read a server Sprint delivery report (commitment, scope changes, carryover, and burndown)

### `id`

Sprint id (UUID)

Value: `ID`

### `--project`

Project key (overrides context)

### `--limit`

Results per page (1\-100; the server default is 50)

### `--cursor`

Opaque cursor returned by a preceding page; `\-\-since\-cursor` is the pipeline\-checkpoint spelling of the same option. Cursors are forwarded verbatim


Supports: `--json`, `--no-input`
