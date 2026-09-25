# `hamstik api rate-limit`

Perform one cheap authenticated Public API read and print the current rate-limit snapshot (`Limit`, `Remaining`, `ResetIn`)

Perform one cheap authenticated Public API read and print the current rate-limit snapshot (`Limit`, `Remaining`, `ResetIn`).

The snapshot mirrors the `meta.rateLimit` fields of `api request --json` and is a point-in-time observation: limits can change between calls, so it is not a guarantee against future 429s.


Supports: `--json`, `--no-input`
