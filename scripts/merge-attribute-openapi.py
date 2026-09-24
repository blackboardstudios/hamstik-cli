#!/usr/bin/env python3
"""Merge the generated HAM-62 Attribute surface into the CLI dev snapshot.

The source must be the JSON emitted by Hamstik's runtime OpenAPI builder.
This scoped merge keeps unrelated, not-yet-supported server operations out of
the CLI contract snapshot while importing the Attribute endpoints and only the
Attribute schema additions to existing Work Item contracts.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SNAPSHOT = ROOT / "openapi" / "hamstik-v1.json"
OPERATION_IDS = {
    "listOrganizationAttributes",
    "createOrganizationAttribute",
    "getOrganizationAttribute",
    "renameOrganizationAttribute",
    "transitionOrganizationAttribute",
    "createOrganizationAttributeOption",
    "renameOrganizationAttributeOption",
    "reorderOrganizationAttributeOptions",
    "retireOrganizationAttributeOption",
    "listProjectAttributes",
    "setProjectAttributeEnablement",
}
ATTRIBUTE_SCHEMAS = {
    "AttributeCatalog",
    "AttributeDefinition",
    "AttributeOption",
    "CreateAttributeDefinitionRequest",
    "CreateAttributeOptionRequest",
    "ProjectAttributeCatalog",
    "ProjectAttributeEnablement",
    "RenameAttributeDefinitionRequest",
    "RenameAttributeOptionRequest",
    "ReorderAttributeOptionsRequest",
    "SetProjectAttributeEnablementRequest",
    "TransitionAttributeDefinitionRequest",
    "WorkItemAttributeChangesRequest",
    "WorkItemAttributeOption",
    "WorkItemAttributeValue",
}
WORK_ITEM_SCHEMAS = {
    "WorkItem",
    "PublicWorkItem",
    "WorkItemSummary",
    "PublicWorkItemSummary",
    "CreateWorkItemRequest",
    "UpdateWorkItemRequest",
    "BulkCreateWorkItemOperation",
}
OPTIONAL_IDEMPOTENCY_HEADERS = {
    "updateWorkItem": "Idempotency-Key",
}


def operation_map(document: dict) -> dict[str, tuple[str, str, dict]]:
    result = {}
    for path, path_item in document.get("paths", {}).items():
        for method, operation in path_item.items():
            if isinstance(operation, dict) and operation.get("operationId"):
                result[operation["operationId"]] = (path, method, operation)
    return result


def merge(snapshot: dict, source: dict) -> dict:
    source_operations = operation_map(source)
    missing = OPERATION_IDS - source_operations.keys()
    if missing:
        raise ValueError(f"generated source is missing Attribute operations: {sorted(missing)}")

    merged = json.loads(json.dumps(snapshot))
    current_operations = operation_map(merged)
    for operation_id in sorted(OPERATION_IDS):
        path, method, operation = source_operations[operation_id]
        existing = current_operations.get(operation_id)
        if existing and existing[:2] != (path, method):
            raise ValueError(
                f"{operation_id} already exists at {existing[1].upper()} {existing[0]}, "
                f"generated source says {method.upper()} {path}"
            )
        merged["paths"].setdefault(path, {})[method] = operation

    # Attribute-bearing Work Item updates require idempotency, while older
    # updates keep their existing wire contract. The runtime documents this as
    # an optional header with conditional-required guidance.
    for operation_id, header_name in OPTIONAL_IDEMPOTENCY_HEADERS.items():
        source_operation = source_operations.get(operation_id)
        merged_operation = current_operations.get(operation_id)
        if source_operation is None or merged_operation is None:
            raise ValueError(f"missing {operation_id} while merging conditional headers")
        source_parameters = source_operation[2].get("parameters", [])
        header = next(
            (
                parameter
                for parameter in source_parameters
                if parameter.get("name") == header_name and parameter.get("in") == "header"
            ),
            None,
        )
        if header is None or header.get("required") is not False:
            raise ValueError(
                f"generated {operation_id} must define optional {header_name} with conditional guidance"
            )
        operation_parameters = merged_operation[2].setdefault("parameters", [])
        operation_parameters[:] = [
            parameter
            for parameter in operation_parameters
            if not (parameter.get("name") == header_name and parameter.get("in") == "header")
        ]
        operation_parameters.append(header)

    components = merged.setdefault("components", {}).setdefault("schemas", {})
    source_schemas = source.get("components", {}).get("schemas", {})
    missing_schemas = ATTRIBUTE_SCHEMAS - source_schemas.keys()
    if missing_schemas:
        raise ValueError(f"generated source is missing Attribute schemas: {sorted(missing_schemas)}")
    for schema_name in sorted(ATTRIBUTE_SCHEMAS):
        components[schema_name] = source_schemas[schema_name]

    # Copy only the Attribute property into existing client schemas. Other
    # unpublished additions in the source (for example Release Versions) are
    # deliberately outside this CLI contract change.
    for schema_name in sorted(WORK_ITEM_SCHEMAS):
        before = components.get(schema_name)
        after = source_schemas.get(schema_name)
        if not isinstance(before, dict) or not isinstance(after, dict):
            raise ValueError(f"missing Work Item schema {schema_name} in one contract")
        attribute_property = after.get("properties", {}).get("attributes")
        if attribute_property is None:
            raise ValueError(f"generated {schema_name} has no Attribute property")
        before.setdefault("properties", {})["attributes"] = attribute_property
        # Full detail response objects always include an empty-or-set array.
        # Summary responses may be sparse, while write properties are optional.
        if schema_name in {"WorkItem", "PublicWorkItem"}:
            required = before.setdefault("required", [])
            if "attributes" not in required:
                required.append("attributes")
                required.sort()

    return merged


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("generated_runtime_openapi", type=Path)
    parser.add_argument("--update", action="store_true", help="write the merged development snapshot")
    args = parser.parse_args()
    try:
        snapshot = json.loads(SNAPSHOT.read_text(encoding="utf-8"))
        source = json.loads(args.generated_runtime_openapi.read_text(encoding="utf-8"))
        result = merge(snapshot, source)
        serialized = json.dumps(result, ensure_ascii=False, separators=(",", ":")) + "\n"
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"cannot merge Attribute OpenAPI contract: {error}", file=sys.stderr)
        return 1

    if not args.update:
        if serialized == SNAPSHOT.read_text(encoding="utf-8"):
            print("HAM-62 Attribute OpenAPI snapshot is current")
            return 0
        print("HAM-62 Attribute OpenAPI contract drift detected; pass --update to write", file=sys.stderr)
        return 1

    SNAPSHOT.write_text(serialized, encoding="utf-8")
    print(f"Updated {SNAPSHOT} from generated runtime OpenAPI")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
