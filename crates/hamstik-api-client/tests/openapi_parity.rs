// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Parsed OpenAPI-to-support-manifest parity guard.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

#[test]
fn every_openapi_operation_is_deliberately_supported() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let contract: Value = serde_json::from_slice(
        &std::fs::read(root.join("openapi/hamstik-v1.json")).expect("read OpenAPI snapshot"),
    )
    .expect("parse OpenAPI snapshot");
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(root.join("openapi/api-parity.json")).expect("read parity manifest"),
    )
    .expect("parse parity manifest");

    assert_eq!(manifest["schemaVersion"], 1);
    let mut contract_operations = BTreeMap::new();
    for (path, item) in contract["paths"].as_object().expect("paths object") {
        for method in [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ] {
            let Some(operation) = item.get(method) else {
                continue;
            };
            let operation_id = operation["operationId"]
                .as_str()
                .expect("every operation has operationId")
                .to_string();
            assert!(
                contract_operations
                    .insert(operation_id.clone(), (method.to_uppercase(), path.clone()))
                    .is_none(),
                "duplicate OpenAPI operationId: {operation_id}"
            );
        }
    }

    let entries = manifest["operations"]
        .as_array()
        .expect("manifest operations array");
    let client_source =
        std::fs::read_to_string(root.join("crates/hamstik-api-client/src/client.rs"))
            .expect("read API-client source");
    let mut manifested = BTreeSet::new();
    for entry in entries {
        let operation_id = entry["operationId"].as_str().expect("manifest operationId");
        assert!(
            manifested.insert(operation_id.to_string()),
            "duplicate manifest operationId: {operation_id}"
        );
        let expected = contract_operations
            .get(operation_id)
            .unwrap_or_else(|| panic!("stale manifest operation: {operation_id}"));
        assert_eq!(entry["method"].as_str(), Some(expected.0.as_str()));
        assert_eq!(entry["path"].as_str(), Some(expected.1.as_str()));
        let client_method = entry["client"]
            .as_str()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| panic!("{operation_id} has no API-client method"));
        assert!(
            client_source.contains(&format!("fn {client_method}(")),
            "{operation_id}: API-client method `{client_method}` is not defined in client.rs"
        );
        assert!(
            entry["cli"]
                .as_array()
                .is_some_and(|commands| !commands.is_empty()),
            "{operation_id} has no CLI command"
        );
        assert!(
            entry["tests"]
                .as_array()
                .is_some_and(|tests| !tests.is_empty()),
            "{operation_id} has no classified tests"
        );
        assert_eq!(
            entry["status"].as_str(),
            Some("Complete"),
            "{operation_id} is not classified Complete"
        );
    }

    let contract_ids: BTreeSet<_> = contract_operations.keys().cloned().collect();
    assert_eq!(
        manifested, contract_ids,
        "OpenAPI operations and api-parity.json differ; classify every added/removed operation"
    );
}
