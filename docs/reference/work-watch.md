# `hamstik work watch`

Follow a work item's activity and comments, rendering new entries as they appear

Follow a work item's activity and comments, rendering new entries as they appear.

Unlike `work await`, which waits for a single server-reported condition and exits, `watch` polls the existing activity and comment endpoints and streams every new entry until interrupted (Ctrl-C). Network failures back off and reconnect; authentication and authorization failures stop the watch.

### `key`

Work item key (e.g. HAM\-42)

Value: `KEY`

### `--since`

Only stream entries strictly after this time. Accepts RFC 3339, `YYYY\-MM\-DD`, `today`/`yesterday`/`tomorrow`, or a relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset is read in the host's local time zone. When omitted, the watch starts at the current instant and only new entries are streamed

### `--interval`

Poll interval (e.g. 2s, 500ms). Defaults to 2s; bounded to 100ms–1h

Default: `2s`

### `--notify`

Shell command to run once for each new entry. The entry is exposed to the command through `HAMSTIK_WATCH_ITEM`, `HAMSTIK_WATCH_TYPE`, `HAMSTIK_WATCH_ID`, `HAMSTIK_WATCH_ACTION` (activity only), and `HAMSTIK_WATCH_JSON` environment variables. A failing hook is reported and never stops the watch


Supports: `--json`, `--no-input`
