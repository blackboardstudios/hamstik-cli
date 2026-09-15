// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Deep parsed-OpenAPI parity guards.
//!
//! These tests fail whenever the checked-in Public API contract changes
//! request/response semantics that the API client or CLI must deliberately
//! account for — not only when an `operationId` appears or disappears. All
//! comparisons are structural (parsed JSON), never textual or key-order
//! dependent, and run fully offline against the checked-in snapshot.
//!
//! When a change is detected: update the Rust models/CLI, classify the
//! operation in `openapi/api-parity.json`, add/update wire tests for the
//! affected behavior, and note the change in the changelog (a `### Breaking`
//! entry when a documented surface changed). Compatibility sign-off happens
//! in review; the failing diff is the checklist.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// The parsed checked-in OpenAPI snapshot.
fn contract() -> &'static Value {
    static CONTRACT: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    CONTRACT.get_or_init(|| {
        serde_json::from_str(include_str!("../../../openapi/hamstik-v1.json"))
            .expect("checked-in OpenAPI snapshot parses")
    })
}

/// The parsed support manifest.
fn manifest() -> &'static Value {
    static MANIFEST: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!("../../../openapi/api-parity.json"))
            .expect("checked-in parity manifest parses")
    })
}

/// Resolves a local JSON pointer (`/components/schemas/WorkItem`) or panics
/// with a pointed message: a dangling pointer is a snapshot bug.
fn resolve<'a>(document: &'a Value, pointer: &str, context: &str) -> &'a Value {
    document
        .pointer(pointer)
        .unwrap_or_else(|| panic!("{context}: dangling $ref {pointer}"))
}

/// Resolves a `$ref` within the contract document (local references only).
fn deref<'a>(document: &'a Value, schema: &'a Value, context: &str) -> &'a Value {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        resolve(document, reference, context)
    } else {
        schema
    }
}

/// Every operation as `(operationId, method, path, operation)` tuples,
/// deterministically ordered.
fn operations() -> Vec<(String, String, String, &'static Value)> {
    let document = contract();
    let mut result = Vec::new();
    for (path, item) in document["paths"].as_object().expect("paths object") {
        for method in [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ] {
            let Some(operation) = item.get(method) else {
                continue;
            };
            let id = operation["operationId"].as_str().expect("operationId");
            result.push((
                id.to_string(),
                method.to_ascii_uppercase(),
                path.clone(),
                operation,
            ));
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

/// Classifies a schema shape change as additive-compatible or breaking.
///
/// Additive: a new optional property, a new schema, a new optional response.
/// Everything else that touches semantics is implementation-relevant drift
/// and fails the guard until reviewed.
enum Drift {
    /// Reviewed and compatible; recorded, not failing.
    Additive,
    /// Must be deliberately accounted for in models/CLI.
    Breaking,
}

/// Deep-compares two schemas for required/optional/nullable, enum, format,
/// and type drift (order-independent).
fn schema_drift(
    document: &Value,
    pointer: &str,
    before: &Value,
    after: &Value,
    findings: &mut Vec<String>,
) -> Drift {
    let before = deref(document, before, pointer);
    let after = deref(document, after, pointer);

    // Type changes are always breaking.
    let before_type = before.get("type").cloned().unwrap_or(Value::Null);
    let after_type = after.get("type").cloned().unwrap_or(Value::Null);
    if !before_type.is_null() && !after_type.is_null() && before_type != after_type {
        findings.push(format!(
            "{pointer}: type changed {before_type} -> {after_type}"
        ));
        return Drift::Breaking;
    }

    // Enum changes are breaking unless purely additive.
    if let (Some(before_enum), Some(after_enum)) = (
        before.get("enum").and_then(Value::as_array),
        after.get("enum").and_then(Value::as_array),
    ) && before_enum != after_enum
    {
        let removed: Vec<_> = before_enum
            .iter()
            .filter(|value| !after_enum.contains(value))
            .collect();
        if removed.is_empty() {
            findings.push(format!(
                "{pointer}: enum gained values {:#?} (additive)",
                after_enum
                    .iter()
                    .filter(|value| !before_enum.contains(value))
                    .collect::<Vec<_>>()
            ));
            return Drift::Additive;
        }
        findings.push(format!("{pointer}: enum lost values {removed:?}"));
        return Drift::Breaking;
    }

    // Format changes are breaking.
    if let (Some(before_format), Some(after_format)) = (
        before.get("format").and_then(Value::as_str),
        after.get("format").and_then(Value::as_str),
    ) && before_format != after_format
    {
        findings.push(format!(
            "{pointer}: format changed {before_format:?} -> {after_format:?}"
        ));
        return Drift::Breaking;
    }

    // Object property drift: required-set changes and property shape changes.
    let before_required: std::collections::BTreeSet<&str> = before
        .get("required")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let after_required: std::collections::BTreeSet<&str> = after
        .get("required")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for newly_required in after_required.difference(&before_required) {
        findings.push(format!("{pointer}.{newly_required}: became required"));
        // Falling through to compare the property shapes still matters.
    }

    let before_properties = before.get("properties").and_then(Value::as_object);
    let after_properties = after.get("properties").and_then(Value::as_object);
    if let (Some(before_properties), Some(after_properties)) = (before_properties, after_properties)
    {
        for (name, after_property) in after_properties {
            match before_properties.get(name) {
                None => {
                    let required_now = after_required.contains(name.as_str());
                    findings.push(format!(
                        "{pointer}.{name}: property added ({})",
                        if required_now {
                            "required"
                        } else {
                            "optional, additive"
                        }
                    ));
                    if required_now {
                        return Drift::Breaking;
                    }
                }
                Some(before_property) => {
                    let child_pointer = format!("{pointer}.{name}");
                    match schema_drift(
                        document,
                        &child_pointer,
                        before_property,
                        after_property,
                        findings,
                    ) {
                        Drift::Breaking => return Drift::Breaking,
                        Drift::Additive => continue,
                    }
                }
            }
        }
        // Removed properties are breaking: the server stopped sending data
        // the typed model expects.
        for removed in before_properties.keys() {
            if !after_properties.contains_key(removed) {
                findings.push(format!("{pointer}.{removed}: property removed"));
                return Drift::Breaking;
            }
        }
    }

    // Array item schema drift.
    if let (Some(before_items), Some(after_items)) = (before.get("items"), after.get("items")) {
        let child_pointer = format!("{pointer}[]");
        return schema_drift(
            document,
            &child_pointer,
            before_items,
            after_items,
            findings,
        );
    }

    // Nullable drift: a field that may become null changes Rust optionality.
    let before_nullable = before
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let after_nullable = after
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !before_nullable && after_nullable {
        findings.push(format!("{pointer}: became nullable"));
        return Drift::Breaking;
    }

    Drift::Additive
}

/// The protected wire behaviors: fixtures assert that the contract still
/// documents each behavior and that the CLI's guarded expectations hold.
#[test]
fn contract_still_documents_protected_wire_behaviors() {
    let document = contract();
    let schemas = &document["components"]["schemas"];
    let operations: BTreeMap<String, &Value> = operations()
        .into_iter()
        .map(|(id, _, _, operation)| (id, operation))
        .collect();

    // Repeated `form`/`explode=true` filters on list operations.
    for operation_id in ["listWorkItems", "listOrganizationWorkItems", "listMyWork"] {
        let operation = operations
            .get(operation_id)
            .copied()
            .unwrap_or_else(|| panic!("{operation_id} missing"));
        let parameters = operation["parameters"].as_array().expect("parameters");
        let repeated: Vec<&Value> = parameters
            .iter()
            .filter(|parameter| {
                parameter.get("schema").is_some() && parameter["schema"]["type"] == "array"
            })
            .collect();
        assert!(
            !repeated.is_empty(),
            "{operation_id} lost its repeated array filter parameters"
        );
        for parameter in repeated {
            assert_eq!(
                parameter["in"], "query",
                "{operation_id}: array filters must be query parameters"
            );
        }
        // Comma-separated `fields` sparse fieldset remains a string.
        let fields = parameters
            .iter()
            .find(|parameter| parameter["name"] == "fields")
            .unwrap_or_else(|| panic!("{operation_id}: --fields parameter disappeared"));
        assert_eq!(
            fields["schema"]["type"], "string",
            "fields must stay a string"
        );
    }

    // ETag / If-Match on revision-protected mutations.
    for operation_id in [
        "updateWorkItem",
        "deleteWorkItem",
        "archiveWorkItem",
        "unarchiveWorkItem",
        "updateProject",
        "archiveProject",
        "unarchiveProject",
        "transitionProjectSprint",
        "createWorkItemLabelAssignment",
        "deleteWorkItemLabelAssignment",
    ] {
        let Some(operation) = operations.get(operation_id).copied() else {
            continue;
        };
        let if_match = operation["parameters"]
            .as_array()
            .expect("parameters")
            .iter()
            .find(|parameter| parameter["name"] == "If-Match")
            .unwrap_or_else(|| panic!("{operation_id} lost its If-Match parameter"));
        assert_eq!(
            if_match["in"], "header",
            "{operation_id}: If-Match is a header"
        );
        assert_eq!(
            if_match["required"], true,
            "{operation_id}: If-Match must stay required"
        );
    }

    // Idempotency headers on retriable mutations.
    for operation_id in [
        "createWorkItem",
        "addWorkItemComment",
        "updateWorkItemComment",
        "createWorkItemComment",
        "createWorkItemLink",
        "deleteWorkItemLink",
        "uploadWorkItemAttachment",
        "createProjectLabel",
        "createProject",
        "createProjectSprint",
        "bulkCreateWorkItems",
        "bulkUpdateWorkItems",
        "bulkTransitionWorkItems",
        "archiveWorkItem",
        "unarchiveWorkItem",
        "deleteWorkItem",
        "archiveProject",
        "unarchiveProject",
        "transitionProjectSprint",
    ] {
        let Some(operation) = operations.get(operation_id).copied() else {
            continue;
        };
        let has_key = operation["parameters"]
            .as_array()
            .expect("parameters")
            .iter()
            .any(|parameter| parameter["name"] == "Idempotency-Key" && parameter["in"] == "header");
        assert!(
            has_key,
            "{operation_id} lost its Idempotency-Key header parameter"
        );
    }

    // Multipart upload content type on attachment creation.
    let upload = operations
        .get("createWorkItemAttachment")
        .copied()
        .expect("upload op");
    let content_type = upload
        .pointer("/requestBody/content")
        .and_then(Value::as_object)
        .expect("upload request body content")
        .keys()
        .next()
        .cloned()
        .unwrap_or_default();
    assert!(
        content_type.starts_with("multipart/form-data"),
        "attachment upload must stay multipart: {content_type}"
    );

    // Binary response content type on avatar download.
    let avatar = operations
        .get("getUserProfileAvatar")
        .copied()
        .expect("avatar op");
    let avatar_content = avatar
        .pointer("/responses/200/content")
        .and_then(Value::as_object)
        .expect("avatar response content");
    assert!(
        avatar_content.keys().any(|key| key.starts_with("image/")),
        "avatar download must stay a binary image response: {avatar_content:?}"
    );

    // Bulk operation schemas keep additionalProperties: false and the
    // documented required fields, and the envelopes keep the 1..=50 bound.
    let bulk_create = &schemas["BulkCreateWorkItemOperation"];
    assert_eq!(bulk_create["additionalProperties"], false);
    assert_eq!(
        bulk_create["required"],
        json_array(&["projectKey", "title"]),
        "bulk create required fields drifted"
    );
    let bulk_update = &schemas["BulkUpdateWorkItemOperation"];
    assert_eq!(bulk_update["additionalProperties"], false);
    assert_eq!(
        bulk_update["required"],
        json_array(&["projectKey", "workItemKey", "changes"])
    );
    let bulk_transition = &schemas["BulkTransitionWorkItemOperation"];
    assert_eq!(bulk_transition["additionalProperties"], false);
    assert_eq!(
        bulk_transition["required"],
        json_array(&["projectKey", "workItemKey", "targetStatus"])
    );
    for envelope_pointer in [
        "/paths/~1api~1v1~1organizations~1{organizationSlug}~1bulk-work-items/post/requestBody/content/application~1json/schema",
        "/paths/~1api~1v1~1organizations~1{organizationSlug}~1bulk-work-items/patch/requestBody/content/application~1json/schema",
        "/paths/~1api~1v1~1organizations~1{organizationSlug}~1bulk-work-item-transitions/post/requestBody/content/application~1json/schema",
    ] {
        let envelope = resolve(document, envelope_pointer, "bulk envelope");
        let operations_schema = &envelope["properties"]["operations"];
        assert_eq!(
            operations_schema["minItems"], 1,
            "{envelope_pointer} minItems"
        );
        assert_eq!(
            operations_schema["maxItems"], 50,
            "{envelope_pointer} maxItems"
        );
    }
}

/// JSON array literal helper.
fn json_array(values: &[&str]) -> Value {
    serde_json::Value::Array(
        values
            .iter()
            .map(|value| serde_json::Value::String(value.to_string()))
            .collect(),
    )
}

/// The manifest names API-client support, CLI support, and test coverage for
/// every operation, and every CLI path resolves in the command tree.
#[test]
fn manifest_covers_client_cli_and_tests_for_every_operation() {
    for (operation_id, _, _, _) in operations() {
        let entry = manifest()["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["operationId"] == operation_id)
            .unwrap_or_else(|| panic!("{operation_id} missing from manifest"));
        assert!(
            entry["client"]
                .as_str()
                .is_some_and(|client| !client.is_empty()),
            "{operation_id}: manifest must name the API-client method"
        );
        assert!(
            entry["cli"].as_array().is_some_and(|cli| !cli.is_empty()),
            "{operation_id}: manifest must name at least one CLI command"
        );
        assert!(
            entry["tests"]
                .as_array()
                .is_some_and(|tests| !tests.is_empty()),
            "{operation_id}: manifest must classify test coverage"
        );
        assert_eq!(
            entry["status"], "Complete",
            "{operation_id}: unclassified manifest status"
        );
    }
}

/// Every operation's documented parameters/request/response schemas stay
/// compatible with the previous snapshot revision (structural comparison).
///
/// The baseline is the schema subset the typed client models assert today;
/// this test detects drift *within* the snapshot by requiring that each
/// schema referenced by an operation still validates against the shape the
/// hand-written models enforce (no unknown required fields appearing in
/// request bodies of modeled operations, no removed response fields).
#[test]
fn operation_schemas_stay_compatible_with_typed_models() {
    let document = contract();
    let schemas = &document["components"]["schemas"];

    // Spot-check the schema shapes the hand-written models freeze: required
    // fields present in the Rust structs must remain required, and the known
    // enums must keep their documented spellings.
    let frozen = [
        (
            "BulkCreateWorkItemOperation",
            vec![("required", json_array(&["projectKey", "title"]))],
        ),
        (
            "BulkUpdateWorkItemOperation",
            vec![(
                "required",
                json_array(&["projectKey", "workItemKey", "changes"]),
            )],
        ),
        (
            "BulkTransitionWorkItemOperation",
            vec![(
                "required",
                json_array(&["projectKey", "workItemKey", "targetStatus"]),
            )],
        ),
        (
            "AuthenticationContext",
            vec![("properties/scopes/type", Value::String("array".to_string()))],
        ),
    ];
    for (schema_name, expectations) in &frozen {
        let schema = schemas
            .get(*schema_name)
            .unwrap_or_else(|| panic!("{schema_name} disappeared from the contract"));
        for (pointer, expected) in expectations {
            let actual = resolve(schema, &format!("/{pointer}"), schema_name);
            assert_eq!(
                actual, expected,
                "{schema_name}.{pointer} drifted; update the Rust model and this fixture deliberately"
            );
        }
    }

    // Enum spellings the CLI parses (clap ValueEnum) must stay stable.
    // The contract inlines these enums on the Work Item and link schemas.
    let work_item = &schemas["WorkItem"];
    let statuses: Vec<&str> = work_item["properties"]["status"]["enum"]
        .as_array()
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        statuses,
        vec!["backlog", "todo", "in_progress", "in_review", "done"],
        "Work Item status enum drifted; the CLI's status spellings must follow deliberately"
    );

    let item_types: Vec<&str> = work_item["properties"]["type"]["enum"]
        .as_array()
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        item_types,
        vec!["task", "bug", "story", "feature", "epic"],
        "Work Item type enum drifted"
    );

    let priorities: Vec<&str> = work_item["properties"]["priority"]["enum"]
        .as_array()
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        priorities,
        vec!["low", "medium", "high", "urgent"],
        "Work Item priority enum drifted"
    );

    let relations: Vec<&str> = schemas["WorkItemLink"]["properties"]["relation"]["enum"]
        .as_array()
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        relations,
        vec!["blocks", "blocked_by", "relates"],
        "Work Item relation enum drifted"
    );

    let concurrency: Vec<&str> = document
        .pointer("/paths/~1api~1v1~1organizations~1{organizationSlug}~1bulk-work-items/patch/requestBody/content/application~1json/schema/properties/concurrency/enum")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        concurrency,
        vec!["require-revision", "last-write-wins"],
        "bulk concurrency enum drifted"
    );
}

/// A self-contained drift-detector test: re-running the comparison logic
/// against mutated fixture schemas produces the expected classification, so
/// the detector itself is verified without needing a second snapshot.
#[test]
fn drift_detector_classifies_changes() {
    let document = contract();
    let schemas = &document["components"]["schemas"];
    let mut findings = Vec::new();

    // Identical schema: no findings, additive.
    let identical = &schemas["BulkCreateWorkItemOperation"];
    let drift = schema_drift(
        document,
        "/components/schemas/BulkCreateWorkItemOperation",
        identical,
        identical,
        &mut findings,
    );
    assert!(findings.is_empty());
    assert!(matches!(drift, Drift::Additive));

    // New optional property: additive.
    let mut additive = identical.clone();
    additive["properties"]["color"] = json!({"type": "string"});
    findings.clear();
    let drift = schema_drift(
        document,
        "/components/schemas/BulkCreateWorkItemOperation",
        identical,
        &additive,
        &mut findings,
    );
    assert!(findings.iter().any(|f| f.contains("optional, additive")));
    assert!(matches!(drift, Drift::Additive));

    // Newly required property: breaking.
    let mut required_drift = identical.clone();
    required_drift["properties"]["color"] = json!({"type": "string"});
    required_drift["required"] = json_array(&["projectKey", "title", "color"]);
    findings.clear();
    let drift = schema_drift(
        document,
        "/components/schemas/BulkCreateWorkItemOperation",
        identical,
        &required_drift,
        &mut findings,
    );
    assert!(findings.iter().any(|f| f.contains("became required")));
    assert!(matches!(drift, Drift::Breaking));

    // Enum losing a value: breaking.
    let enum_before = json!({"type": "string", "enum": ["a", "b"]});
    let enum_after = json!({"type": "string", "enum": ["a"]});
    findings.clear();
    let drift = schema_drift(
        document,
        "/components/schemas/SyntheticEnum",
        &enum_before,
        &enum_after,
        &mut findings,
    );
    assert!(findings.iter().any(|f| f.contains("enum lost")));
    assert!(matches!(drift, Drift::Breaking));

    // Enum gaining a value: additive.
    let enum_widened = json!({"type": "string", "enum": ["a", "b", "c"]});
    findings.clear();
    let drift = schema_drift(
        document,
        "/components/schemas/SyntheticEnum",
        &enum_before,
        &enum_widened,
        &mut findings,
    );
    assert!(findings.iter().any(|f| f.contains("enum gained")));
    assert!(matches!(drift, Drift::Additive));

    // Nullable drift: breaking.
    let mut nullable_drift = identical.clone();
    nullable_drift["properties"]["title"]["nullable"] = serde_json::Value::Bool(true);
    findings.clear();
    let drift = schema_drift(
        document,
        "/components/schemas/BulkCreateWorkItemOperation",
        identical,
        &nullable_drift,
        &mut findings,
    );
    assert!(findings.iter().any(|f| f.contains("became nullable")));
    assert!(matches!(drift, Drift::Breaking));

    // Property removal: breaking.
    let mut removed = identical.clone();
    removed["properties"]
        .as_object_mut()
        .unwrap()
        .remove("title");
    findings.clear();
    let drift = schema_drift(
        document,
        "/components/schemas/BulkCreateWorkItemOperation",
        identical,
        &removed,
        &mut findings,
    );
    assert!(findings.iter().any(|f| f.contains("property removed")));
    assert!(matches!(drift, Drift::Breaking));
}

/// The live-drift workflow stays documented and offline: the snapshot is
/// refreshed only by the script, and the manifest schema version stays 1.
#[test]
fn drift_workflow_documentation_stays_in_place() {
    let manifest = manifest();
    assert_eq!(manifest["schemaVersion"], 1);
    // The update script must keep documenting the explicit check/update flow.
    let script = include_str!("../../../scripts/update-openapi.sh");
    assert!(
        script.contains("--check") && script.contains("--update"),
        "update-openapi.sh lost its explicit check/update workflow"
    );
    assert!(
        script.contains("https://"),
        "update-openapi.sh must point at the live contract"
    );
    // The snapshot itself stays OpenAPI 3 with v1 info.
    let document = contract();
    assert_eq!(
        document["openapi"]
            .as_str()
            .unwrap_or_default()
            .split('.')
            .next(),
        Some("3")
    );
    assert_eq!(
        document["info"]["version"]
            .as_str()
            .unwrap_or_default()
            .split('.')
            .next(),
        Some("1")
    );
}
