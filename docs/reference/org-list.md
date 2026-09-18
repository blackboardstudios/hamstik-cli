# `hamstik org list`

List organizations you belong to

### `--limit`

Maximum total items to emit, counted across pages (distinct from the server page size). Without `\-\-all` at most one page is returned, holding at most this many items; a page is never requested larger than the endpoint allows (200 items, or 100 on organization, project, sprint, label, and link endpoints). With `\-\-all` successive pages are followed until this cap or the end of the collection. A cap that cuts through a server page ends the run with `page.nextCursor: null`, because the Public API has no cursor for a position partway through a page

### `--cursor`

Opaque cursor returned by a preceding page; the result starts after it. `\-\-since\-cursor` is the pipeline\-checkpoint spelling of the same option: resume an interrupted stream from the `page.nextCursor` a previous run reported. Cursors are opaque and are always forwarded verbatim

### `--all`

Follow all pages

Choices: `true`, `false`


Supports: `--json`, `--no-input`
