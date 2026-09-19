# `hamstik`

Official command-line interface for Hamstik

This command has subcommands; see their own pages.

### `--host`

Hamstik host origin (overrides profile/context/default)

### `--profile`

Profile name to use for credentials and defaults

### `--org`

Organization slug override

### `--project`

Project key override

### `--json`

Emit machine\-readable JSON

Choices: `true`, `false`

### `--jsonl`

Emit JSON Lines (one JSON object per line, NDJSON)

Choices: `true`, `false`

### `--tsv`

Emit tab\-separated values

Choices: `true`, `false`

### `--quiet`

Emit only essential identifiers

Choices: `true`, `false`

### `--jq`

Apply a jq filter expression to structured output (requires \-\-json, \-\-jsonl, or \-\-tsv)

### `--columns`

Restrict list output to these columns (header names), in order

### `--no-header`

Suppress the header row in list output (TSV and human table modes)

Choices: `true`, `false`

### `--verbose`

Show diagnostic details on stderr

Choices: `true`, `false`

### `--no-color`

Disable colored output

Choices: `true`, `false`

### `--no-input`

Never prompt interactively; fail instead

Choices: `true`, `false`

### `--no-retry`

Disable automatic retries

Choices: `true`, `false`

### `--dry-run`

Preview the mutation instead of sending it: resolves identifiers and validates local input, prints a versioned preview (or `\-\-json` envelope), and sends no request. Supported by mutation commands only

Choices: `true`, `false`

### `--ca-bundle`

Additional PEM root certificate bundle


Supports: `--json`, `--no-input`
