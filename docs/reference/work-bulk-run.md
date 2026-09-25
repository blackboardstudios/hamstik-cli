# `hamstik work bulk run`

Run a large operation set in resumable batches of at most 50

Run a large operation set in resumable batches of at most 50.

Reads a JSON array or JSON-lines operations file (use `-` for stdin), preflights every operation, and sends batches of at most 50 in sequence. Each batch's request body and idempotency key are recorded in a local journal before any request is sent, so an interrupted run resumes from the journal without redoing completed batches or duplicating creates. Failed batches are never retried automatically; pass `--retry-failed` after reviewing the journal. Separate batches are separate API requests and are not one atomic transaction.

### `--op`

Operation kind the batches perform

Choices: `create`, `update`, `transition`

### `--operations-file`

Operations as a JSON array or JSON\-lines file (path, or \- for stdin)

### `--journal`

Local journal recording per\-batch plans and results (created or resumed)

### `--concurrency`

Concurrency mode for update/transition batches

Choices: `require-revision`, `last-write-wins`

### `--retry-failed`

Re\-send batches that previously failed (explicit review action)

Choices: `true`, `false`

### `--restart`

Start a new run, replacing any journal already at `\-\-journal`

Choices: `true`, `false`


Supports: `--json`, `--no-input`
