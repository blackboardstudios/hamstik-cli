# `hamstik work export`

Export a work item as a portable Markdown document

Export a work item as a portable Markdown document.

The document is YAML frontmatter (`key`, `title`, `type`, `status`, `priority`, `labels`, and optionally `links`/`comments`) plus the description as the Markdown body. It is suitable for pasting into a GitHub/GitLab issue or handing work to another tracker. `--format markdown` (the default) writes the document; `--json` emits the same fields as a structured envelope.

### `key`

Work item key

Value: `KEY`

### `--comments`

Include the item's comments in the exported frontmatter

Choices: `true`, `false`

### ``-o`, --output`

Write the document to a file instead of stdout


Supports: `--json`, `--no-input`
