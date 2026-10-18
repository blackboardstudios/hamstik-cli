# `hamstik api invoke`

Invoke an operation by its Public API v1 `operationId`

Invoke an operation by its Public API v1 `operationId`.

Method, path, parameters, required headers, request content type, and response content type are resolved from the frozen v1.0.0 contract.

### `operation_id`

OpenAPI operationId (for example `listOrganizationWebhooks`)

Value: `OPERATION_ID`

### `--param`

Path parameter as `name=value`; organizationSlug and projectKey default from the resolved CLI context

### `--query`

Query parameter as `name=value`; repeat array parameters as needed

### `--body-file`

Read the JSON request body from this file; `\-` reads stdin

### `--field`

Structured JSON field as `key=value`; repeatable

### `--upload`

File for an operation whose request body is `multipart/form\-data`

### `--content-type`

MIME type for `\-\-upload` (defaults to application/octet\-stream)

### `--output`

Write a binary response to this path. Required for binary operations

### `--if-match`

Revision precondition for operations that document `If\-Match`

### `--idempotency-key`

Idempotency key override. A strong key is generated when the operation contract requires one


Supports: `--json`, `--no-input`
