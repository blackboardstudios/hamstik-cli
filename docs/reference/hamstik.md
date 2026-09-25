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

### `--format`

Select the output format by name. `ndjson`/`jsonl` stream one JSON resource per line, `tsv` and `csv` render the command's table, `table` (or `human`) is the aligned human table, `json` is one pretty\-printed document, and `markdown` renders a GitHub\-flavored table for list\-shaped output. Conflicts with the dedicated output\-mode flags

Choices: `table`, `human`, `json`, `ndjson`, `jsonl`, `tsv`, `csv`, `markdown`

### `--quiet`

Emit only essential identifiers

Choices: `true`, `false`

### `--jq`

Apply a jq filter expression to structured output (requires \-\-json, \-\-jsonl, or \-\-tsv)

### `--columns`

Restrict list output to these columns (header names), in order

### `--fields`

Restrict output to these comma\-separated fields, in order. An alias for `\-\-columns` on commands that do not take a server\-side sparse fieldset; on the Work Item list commands the value is the server sparse fieldset. Unknown names fail as a usage error listing the valid names

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

### `--confirm-destructive`

Consent to the destructive operation this invocation performs (`work delete`, `project archive`, or completing a Sprint). Required when interactive confirmation is unavailable; `\-\-yes` is the scripting override

Choices: `true`, `false`

### `--yes`

Scripting override: consent to the destructive operation this invocation performs without prompting. Prefer `\-\-confirm\-destructive` in interactive sessions

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
