# Hamstik CLI automation reference

Read this file when a task needs nontrivial output handling, pagination, bulk operations,
imports/exports, or scheduled CLI execution. For exact flags, the installed
`hamstik <command> --help` remains authoritative.

## Output modes

Prefer `--json --no-input` for agent reads and structured automation.

The CLI also supports:

- `--jsonl` / `--format ndjson|jsonl` for one JSON resource per line;
- `--tsv`;
- `--format csv`;
- `--format markdown`;
- `--format table|human`;
- `--quiet` when only an identifier is required.

`--json`, `--jsonl`, `--tsv`, and `--quiet` are mutually exclusive. The umbrella
`--format` form conflicts with the dedicated output switches.

Use `--columns NAME...` or `--fields a,b,c` for supported table/list projections.
On Work Item list commands, `--fields` is also the server-side sparse fieldset.

Use `--jq EXPR` with supported structured modes when filtering the command's
server-shaped result. Do not combine `--jq` with `--columns`.

Structured modes are ANSI-free. For captured human output use `--color=never`,
`NO_COLOR=1`, or `HAMSTIK_NO_COLOR=1`. `HAMSTIK_TERM=ascii` selects ASCII decoration.

Parse stdout only as command output. Normal diagnostics and structured errors go to
stderr. `doctor --json` is the notable exception: its report remains on stdout even
when the process exits nonzero.

## Pagination and deterministic reads

Treat cursors as opaque. Pass `.page.nextCursor` back unchanged with `--cursor` /
`--since-cursor`.

Use `--all` when complete traversal is required. Resume flags work with `--all`.

`--limit N` caps total emitted items, not merely a page. If a cap cuts through a page
and no resume cursor exists for that exact position, restart from the last complete
checkpoint with a smaller cap or use `--all`.

For Work Item ordering use `--sort KEY[:DIR]` where supported. When ordering must span a
whole collection, combine sorting with `--all`.

Do not quote aggregate totals from a truncated result.

## Long-form text

Prefer file/stdin inputs instead of shell-quoted Markdown:

```bash
hamstik --json --no-input --org <ORG> --project <KEY> work comment add \
  <ITEM-KEY> --body-file <COMMENT.md>

hamstik --json --no-input --org <ORG> --project <KEY> work comment add \
  <ITEM-KEY> --body-file - < COMMENT.md

hamstik --json --no-input --org <ORG> squeakql validate --file QUERY.sqql
```

## Date expressions

Date-like flags such as `--due-date`, `--start-date`, and `--updated-after` accept
RFC 3339, `YYYY-MM-DD`, `today`, `yesterday`, `tomorrow`, and supported relative values
such as `30m`, `7d`, `2w`, `+3h`, `1mo`, and `1y`.

Prefer these expressions over precomputing timestamps when the user gives a relative
deadline. The CLI converts values to RFC 3339 UTC and rejects invalid input locally.

## Bulk operations

Typed bulk commands accept Public API operation arrays with at most 50 operations:

```bash
hamstik --json --no-input --org <ORG> work bulk create \
  --project <KEY> --operations-file <OPERATIONS.json>

hamstik --json --no-input --org <ORG> work bulk update \
  --operations-file <OPERATIONS.json> --dry-run
```

Bulk files are preflighted locally. Fix preflight failures rather than retrying them.
Server authorization and business rules are still authoritative.

`bulk update` and `bulk transition` default to revision-aware concurrency. Do not choose
last-write-wins automatically.

Spreadsheet-derived operations can be converted with the CLI's CSV converter rather
than hand-writing JSON:

```bash
hamstik work bulk from-csv items.csv --op create --project <KEY> > operations.json
```

For more than 50 operations use the resumable runner:

```bash
hamstik --json --no-input --org <ORG> work bulk run \
  --op create --operations-file <OPERATIONS.json> --journal <JOURNAL.jsonl>

hamstik --json --no-input --org <ORG> work bulk run \
  --op create --journal <JOURNAL.jsonl>
```

The journal records batch state and idempotency information without credentials.
Completed batches are skipped on resume; uncertain batches are replayed with the
original key/body. Failed batches are not automatically retried. Review them before
using `--retry-failed`.

Separate batches are separate API requests and are not one atomic transaction.

## Create from existing content

Work Item creation can pre-fill from an existing item or a Markdown template. Exact
copied fields are defined by the CLI; do not assume every Work Item field is copied.

```bash
hamstik --json --no-input --org <ORG> --project <KEY> work create \
  --from <ITEM-KEY> --title "Overridden title"

hamstik --json --no-input --org <ORG> --project <KEY> work create \
  --template <TEMPLATE.md>
```

Preview unfamiliar mappings with `--dry-run`.

## Import and export

Export a portable Work Item document:

```bash
hamstik --no-input --org <ORG> --project <KEY> work export <ITEM-KEY> \
  --format markdown --comments --output <ITEM.md>
```

Import with:

```bash
hamstik --json --no-input --org <ORG> --project <KEY> work import --file <ITEM.md>
```

Preview imports with `--dry-run`. Existing embedded keys may update an item; otherwise
the CLI can create idempotently. Revision conflicts still surface normally.

Query exports can use CSV/JSONL/TSV/JSON/Markdown/table output and `--output`.

## Saved schedules

The CLI stores schedule definitions but does not run a daemon. An external scheduler
must invoke them.

```bash
hamstik schedule save nightly -- work export --org <ORG> \
  --query 'status = todo' --format csv --output todos.csv

hamstik schedule list --json
hamstik schedule run nightly
```

Definitions store the Hamstik argument vector, not credentials. Authentication must be
available in the environment/context used by the external scheduler.
