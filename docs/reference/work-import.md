# `hamstik work import`

Create or update work items from an exported Markdown document

Create or update work items from an exported Markdown document.

Reads the format `work export` writes and maps its fields onto the existing create/edit operations. When the embedded `key` already resolves, the item is updated in place; otherwise a new item is created with an idempotency key derived from the embedded key, so re-importing the same document does not duplicate items. `--dry-run` previews the mapped operations without sending them.

### `--file`

Exported Markdown document (path, or `\-` for stdin)

### `--idempotency-key`

Explicit idempotency key for the create. Overrides the key derived from the document's embedded `key`; the derived key keeps re\-imports from duplicating items


Supports: `--json`, `--no-input`
