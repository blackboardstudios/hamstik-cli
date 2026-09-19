# `hamstik project report`

Read a server project report (velocity, ageing-wip, epic-progress, …)

### `report_type`

Report type (for example `velocity`, `cumulative\-flow`, `control\-chart`, `ageing\-wip`, `created\-vs\-resolved`, `distribution`, `epic\-progress`); passed to the server unchanged

Value: `REPORT_TYPE`

### `--project`

Project key (overrides context)

### `--range`

Recent\-Sprint count or calendar\-day window (the server caps the range per report type)

### `--start`

Inclusive ISO calendar date starting the window

### `--end`

Inclusive ISO calendar date ending the window

### `--time-zone`

IANA time zone the calendar window is evaluated in

### `--unit`

Measurement unit

Choices: `count`, `points`, `items`

### `--interval`

Sampling interval

Choices: `day`, `week`, `month`

### `--measure`

Time measure for cycle\-time reports

Choices: `cycle`, `lead`

### `--cycle-start-status`

Canonical status whose first entry starts cycle time

Choices: `backlog`, `todo`, `in_progress`, `in_review`, `done`

### `--window`

Rolling window size for moving measures (1\-100)

### `--group-by`

Grouping dimension

Choices: `status`, `type`, `priority`, `assignee`, `label`

### `--scope`

Which items count toward the report

Choices: `open`, `all`

### `--sprint`

Restrict the report to one Sprint (UUID)

### `--sort`

Result ordering key

Choices: `name`, `progress`, `targetDate`, `age`, `workItem`, `status`, `assignee`, `since`

### `--q`

Free\-text filter on Work Items in scope

### `--squeakql`

SqueakQL filter expression restricting the items in scope

### `--status`

Only count items in this status

Choices: `backlog`, `todo`, `in_progress`, `in_review`, `done`

### `--type`

Only count items of this type

Choices: `task`, `bug`, `story`, `feature`, `epic`

### `--priority`

Only count items at this priority

Choices: `low`, `medium`, `high`, `urgent`

### `--assignee`

Only count items assigned to this user (`me` or a `usr_` public ID)

### `--label`

Only count items carrying this label

### `--buckets`

Bucket count or explicit boundaries for distribution reports

### `--limit`

Results per page (1\-100; the server default is 50)

### `--cursor`

Opaque cursor returned by a preceding page; `\-\-since\-cursor` is the pipeline\-checkpoint spelling of the same option. Cursors are forwarded verbatim


Supports: `--json`, `--no-input`
