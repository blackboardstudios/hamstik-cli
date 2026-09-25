# `hamstik work export`

Export a work item as a portable Markdown document, or a `--query` result set as a CSV/JSONL/TSV/JSON snapshot

Export a work item as a portable Markdown document, or a `--query` result set as a CSV/JSONL/TSV/JSON snapshot.

With a `<KEY>`, the document is YAML frontmatter (`key`, `title`, `type`, `status`, `priority`, `labels`, and optionally `links`/`comments`) plus the description as the Markdown body. It is suitable for pasting into a GitHub/GitLab issue or handing work to another tracker. `--format markdown` (the default) writes the document; `--json` emits the same fields as a structured envelope.

With `--query <SQUEAKQL>` (or `--query-file`/`--query-saved`), the matching Organization Work Items are exported as a collection in the active output mode, which is what an external scheduler snapshots. `--output <FILE>` writes the exact bytes a stdout run would emit.

### `key`

Work item key (single\-item Markdown handoff)

Value: `KEY`

### `--query`

SqueakQL expression selecting the Work Items to export as a collection

### `--query-file`

Read the SqueakQL expression from a file (`\-` for stdin)

### `--query-saved`

Run a saved SqueakQL query by name instead of an inline expression

### `--comments`

Include the item's comments in the single\-item Markdown export

Choices: `true`, `false`

### `--limit`

Maximum total items to emit, counted across pages (distinct from the server page size). Without `\-\-all` at most one page is returned, holding at most this many items; a page is never requested larger than the endpoint allows (200 items, or 100 on organization, project, sprint, label, and link endpoints). With `\-\-all` successive pages are followed until this cap or the end of the collection. A cap that cuts through a server page ends the run with `page.nextCursor: null`, because the Public API has no cursor for a position partway through a page

### `--cursor`

Opaque cursor returned by a preceding page; the result starts after it. `\-\-since\-cursor` is the pipeline\-checkpoint spelling of the same option: resume an interrupted stream from the `page.nextCursor` a previous run reported. Cursors are opaque and are always forwarded verbatim

### `--all`

Follow all pages

Choices: `true`, `false`

### ``-o`, --output`

Write the output to a file instead of stdout; `\-` writes stdout. For a collection export the bytes match the same invocation without `\-\-output`, so a scheduled run is byte\-identical to a manual one


Supports: `--json`, `--no-input`
