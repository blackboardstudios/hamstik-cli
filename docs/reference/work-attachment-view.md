# `hamstik work attachment view`

Preview an image attachment inline on a capable terminal

Preview an image attachment inline on a capable terminal.

When no inline protocol is detected, or under `--json`, `--quiet`, `--no-input`, or non-TTY output, this falls back to the `download` behavior and writes the bytes to a file instead.

### `key`

Work item key

Value: `KEY`

### `attachment_id`

Attachment id (UUID)

Value: `ATTACHMENT_ID`

### ``-o`, --output`

Output path used when inline preview is unavailable (defaults to the attachment's file name in the current directory)


Supports: `--json`, `--no-input`
