# `hamstik work edit`

Edit a work item

### `key`

Work item key

Value: `KEY`

### `--title`

New title

### `--description`

New description text

### `--description-file`

New description source (path, or \- for stdin)

### `--description-editor`

Author the description in $VISUAL/$EDITOR instead of passing text

Choices: `true`, `false`

### `--type`

New work item type

Choices: `task`, `bug`, `story`, `feature`, `epic`

### `--priority`

New priority

Choices: `low`, `medium`, `high`, `urgent`

### `--assignee`

New assignee (or `me`)

### `--sprint`

Sprint id

### `--parent`

New parent key (or parent UUID)

### `--story-points`

New story point estimate

### `--due-date`

New due date (RFC 3339)

### `--clear-description`

Clear the description

Choices: `true`, `false`

### `--clear-assignee`

Unassign the item

Choices: `true`, `false`

### `--clear-sprint`

Remove the sprint

Choices: `true`, `false`

### `--clear-parent`

Detach the parent

Choices: `true`, `false`

### `--clear-story-points`

Clear the story point estimate

Choices: `true`, `false`

### `--clear-due-date`

Clear the due date

Choices: `true`, `false`

### `--force`

Bypass revision conflict protection (If\-Match: *)

Choices: `true`, `false`


Supports: `--json`, `--no-input`
