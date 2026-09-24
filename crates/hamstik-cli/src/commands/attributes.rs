// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Organization-governed Attribute discovery and administration.

use hamstik_api_client::{
    AttributeDefinition, AttributeState, AttributeType, CreateAttributeDefinitionRequest,
    CreateAttributeOptionRequest, ListAttributesOptions, RenameAttributeDefinitionRequest,
    RenameAttributeOptionRequest, ReorderAttributeOptionsRequest,
    SetProjectAttributeEnablementRequest, TransitionAttributeDefinitionRequest, generate_key,
    validate_key,
};
use serde_json::{Value, json};

use crate::app::Session;
use crate::args::{
    AttributeArgs, AttributeCommand, AttributeOptionCommand, AttributeProjectCommand,
    AttributeStateArg, AttributeTypeArg,
};
use crate::error::CliError;

use super::dryrun;
use super::{check_columns, emit_view, render_list};

/// Runs the `attribute` subcommands.
pub async fn run(session: &mut Session<'_>, args: &AttributeArgs) -> Result<(), CliError> {
    match &args.command {
        AttributeCommand::List { include_retired } => list(session, *include_retired).await,
        AttributeCommand::View { key } => view(session, key).await,
        AttributeCommand::Create {
            key,
            name,
            attribute_type,
            idempotency_key,
        } => {
            create(
                session,
                key,
                name,
                *attribute_type,
                idempotency_key.as_deref(),
            )
            .await
        }
        AttributeCommand::Rename {
            key,
            name,
            reason,
            idempotency_key,
        } => {
            rename(
                session,
                key,
                name,
                reason.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        AttributeCommand::Transition {
            key,
            target,
            reason,
            idempotency_key,
        } => {
            transition(
                session,
                key,
                *target,
                reason.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        AttributeCommand::Option(args) => match &args.command {
            AttributeOptionCommand::Add {
                attribute_key,
                key,
                label,
                idempotency_key,
            } => {
                add_option(
                    session,
                    attribute_key,
                    key,
                    label,
                    idempotency_key.as_deref(),
                )
                .await
            }
            AttributeOptionCommand::Rename {
                attribute_key,
                option_key,
                label,
                reason,
                idempotency_key,
            } => {
                rename_option(
                    session,
                    attribute_key,
                    option_key,
                    label,
                    reason.as_deref(),
                    idempotency_key.as_deref(),
                )
                .await
            }
            AttributeOptionCommand::Reorder {
                attribute_key,
                option_keys,
                reason,
                idempotency_key,
            } => {
                reorder_options(
                    session,
                    attribute_key,
                    option_keys,
                    reason.as_deref(),
                    idempotency_key.as_deref(),
                )
                .await
            }
            AttributeOptionCommand::Retire {
                attribute_key,
                option_key,
                idempotency_key,
            } => {
                retire_option(
                    session,
                    attribute_key,
                    option_key,
                    idempotency_key.as_deref(),
                )
                .await
            }
        },
        AttributeCommand::Project(args) => match &args.command {
            AttributeProjectCommand::List => list_project(session).await,
            AttributeProjectCommand::Enable {
                key,
                reason,
                idempotency_key,
            } => {
                set_project_enablement(
                    session,
                    key,
                    true,
                    reason.as_deref(),
                    idempotency_key.as_deref(),
                )
                .await
            }
            AttributeProjectCommand::Disable {
                key,
                reason,
                idempotency_key,
            } => {
                set_project_enablement(
                    session,
                    key,
                    false,
                    reason.as_deref(),
                    idempotency_key.as_deref(),
                )
                .await
            }
        },
    }
}

async fn list(session: &mut Session<'_>, include_retired: bool) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .list_organization_attributes(&org, ListAttributesOptions { include_retired })
        .await
        .map_err(CliError::from_client)?;
    let rows = response
        .value
        .items
        .iter()
        .map(|definition| {
            vec![
                definition.key.clone(),
                definition.name.clone(),
                definition.attribute_type.as_str().to_string(),
                definition.state.as_str().to_string(),
                definition.options.len().to_string(),
                definition.revision.to_string(),
            ]
        })
        .collect::<Vec<_>>();
    let headers = ["KEY", "NAME", "TYPE", "STATE", "OPTIONS", "REVISION"];
    check_columns(&headers, &session.output_options())?;
    render_list(session, &response.raw, &headers, &rows)
}

async fn view(session: &mut Session<'_>, key: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .get_organization_attribute(&org, key)
        .await
        .map_err(CliError::from_client)?;
    let definition = response.value.clone();
    emit_view(session, &response.raw, &definition.key, |session| {
        render_definition(session, &definition)
    })
}

async fn create(
    session: &mut Session<'_>,
    key: &str,
    name: &str,
    attribute_type: AttributeTypeArg,
    idempotency_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let body = CreateAttributeDefinitionRequest {
        key: key.to_string(),
        name: name.to_string(),
        attribute_type: match attribute_type {
            AttributeTypeArg::SingleSelect => AttributeType::SingleSelect,
            AttributeTypeArg::MultiSelect => AttributeType::MultiSelect,
            AttributeTypeArg::Boolean => AttributeType::Boolean,
        },
    };
    let idempotency = idem_key(idempotency_flag)?;
    let path = format!("/api/v1/organizations/{org}/attributes");
    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "attribute.create",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/attributes",
                path,
                resolved: json!({ "organization": org, "attribute": key }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }
    let api = session.api(&selection)?;
    let response = api
        .create_organization_attribute(&org, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    record_mutation(
        session,
        "attribute.create",
        &response.value.key,
        response.request_id.as_deref(),
    );
    emit_definition_response(session, response.raw, response.value)
}

async fn rename(
    session: &mut Session<'_>,
    key: &str,
    name: &str,
    reason: Option<&str>,
    idempotency_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let body = RenameAttributeDefinitionRequest {
        name: name.to_string(),
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_flag)?;
    let current = api
        .get_organization_attribute(&org, key)
        .await
        .map_err(CliError::from_client)?;
    let etag = required_etag(current.etag)?;
    let path = format!("/api/v1/organizations/{org}/attributes/{key}");
    if session.global.dry_run {
        return mutation_preview(
            session,
            MutationPreview {
                operation: "attribute.rename",
                method: "PATCH",
                path_template: "/api/v1/organizations/{organization}/attributes/{key}",
                path,
                resolved: json!({ "organization": org, "attribute": key }),
                etag: &etag,
                idempotency: &idempotency,
                body: &body,
            },
        );
    }
    let response = api
        .rename_organization_attribute(&org, key, &body, &etag, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    record_mutation(
        session,
        "attribute.rename",
        key,
        response.request_id.as_deref(),
    );
    emit_definition_response(session, response.raw, response.value)
}

async fn transition(
    session: &mut Session<'_>,
    key: &str,
    target: AttributeStateArg,
    reason: Option<&str>,
    idempotency_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let body = TransitionAttributeDefinitionRequest {
        target_state: match target {
            AttributeStateArg::Active => AttributeState::Active,
            AttributeStateArg::Disabled => AttributeState::Disabled,
            AttributeStateArg::Retired => AttributeState::Retired,
        },
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_flag)?;
    let current = api
        .get_organization_attribute(&org, key)
        .await
        .map_err(CliError::from_client)?;
    let etag = required_etag(current.etag)?;
    let path = format!("/api/v1/organizations/{org}/attributes/{key}/transitions");
    if session.global.dry_run {
        return mutation_preview(
            session,
            MutationPreview {
                operation: "attribute.transition",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/attributes/{key}/transitions",
                path,
                resolved: json!({ "organization": org, "attribute": key }),
                etag: &etag,
                idempotency: &idempotency,
                body: &body,
            },
        );
    }
    let response = api
        .transition_organization_attribute(&org, key, &body, &etag, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    record_mutation(
        session,
        "attribute.transition",
        key,
        response.request_id.as_deref(),
    );
    emit_definition_response(session, response.raw, response.value)
}

async fn add_option(
    session: &mut Session<'_>,
    key: &str,
    option_key: &str,
    label: &str,
    idempotency_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let body = CreateAttributeOptionRequest {
        key: option_key.to_string(),
        label: label.to_string(),
    };
    let idempotency = idem_key(idempotency_flag)?;
    let current = api
        .get_organization_attribute(&org, key)
        .await
        .map_err(CliError::from_client)?;
    let etag = required_etag(current.etag)?;
    let path = format!("/api/v1/organizations/{org}/attributes/{key}/options");
    if session.global.dry_run {
        return mutation_preview(
            session,
            MutationPreview {
                operation: "attribute.option.add",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/attributes/{key}/options",
                path,
                resolved: json!({ "organization": org, "attribute": key, "option": option_key }),
                etag: &etag,
                idempotency: &idempotency,
                body: &body,
            },
        );
    }
    let response = api
        .create_organization_attribute_option(&org, key, &body, &etag, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    record_mutation(
        session,
        "attribute.option.add",
        key,
        response.request_id.as_deref(),
    );
    emit_definition_response(session, response.raw, response.value)
}

async fn rename_option(
    session: &mut Session<'_>,
    key: &str,
    option_key: &str,
    label: &str,
    reason: Option<&str>,
    idempotency_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let body = RenameAttributeOptionRequest {
        label: label.to_string(),
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_flag)?;
    let current = api
        .get_organization_attribute(&org, key)
        .await
        .map_err(CliError::from_client)?;
    let etag = required_etag(current.etag)?;
    let path = format!("/api/v1/organizations/{org}/attributes/{key}/options/{option_key}");
    if session.global.dry_run {
        return mutation_preview(
            session,
            MutationPreview {
                operation: "attribute.option.rename",
                method: "PATCH",
                path_template: "/api/v1/organizations/{organization}/attributes/{key}/options/{optionKey}",
                path,
                resolved: json!({ "organization": org, "attribute": key, "option": option_key }),
                etag: &etag,
                idempotency: &idempotency,
                body: &body,
            },
        );
    }
    let response = api
        .rename_organization_attribute_option(&org, key, option_key, &body, &etag, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    record_mutation(
        session,
        "attribute.option.rename",
        key,
        response.request_id.as_deref(),
    );
    emit_definition_response(session, response.raw, response.value)
}

async fn reorder_options(
    session: &mut Session<'_>,
    key: &str,
    option_keys: &[String],
    reason: Option<&str>,
    idempotency_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let body = ReorderAttributeOptionsRequest {
        option_keys: option_keys.to_vec(),
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_flag)?;
    let current = api
        .get_organization_attribute(&org, key)
        .await
        .map_err(CliError::from_client)?;
    let etag = required_etag(current.etag)?;
    let path = format!("/api/v1/organizations/{org}/attributes/{key}/options/reorder");
    if session.global.dry_run {
        return mutation_preview(
            session,
            MutationPreview {
                operation: "attribute.option.reorder",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/attributes/{key}/options/reorder",
                path,
                resolved: json!({ "organization": org, "attribute": key }),
                etag: &etag,
                idempotency: &idempotency,
                body: &body,
            },
        );
    }
    let response = api
        .reorder_organization_attribute_options(&org, key, &body, &etag, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    record_mutation(
        session,
        "attribute.option.reorder",
        key,
        response.request_id.as_deref(),
    );
    emit_definition_response(session, response.raw, response.value)
}

async fn retire_option(
    session: &mut Session<'_>,
    key: &str,
    option_key: &str,
    idempotency_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let idempotency = idem_key(idempotency_flag)?;
    let current = api
        .get_organization_attribute(&org, key)
        .await
        .map_err(CliError::from_client)?;
    let etag = required_etag(current.etag)?;
    let path = format!("/api/v1/organizations/{org}/attributes/{key}/options/{option_key}/retire");
    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "attribute.option.retire",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/attributes/{key}/options/{optionKey}/retire",
                path,
                resolved: json!({ "organization": org, "attribute": key, "option": option_key }),
                if_match: Some(&etag),
                idempotency_key: Some(&idempotency),
                body: Some(json!({})),
                notes: Vec::new(),
            },
        );
    }
    let response = api
        .retire_organization_attribute_option(&org, key, option_key, &etag, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    record_mutation(
        session,
        "attribute.option.retire",
        key,
        response.request_id.as_deref(),
    );
    emit_definition_response(session, response.raw, response.value)
}

async fn list_project(session: &mut Session<'_>) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .list_project_attributes(&org, &project)
        .await
        .map_err(CliError::from_client)?;
    let enabled_keys = response
        .value
        .enabled
        .iter()
        .map(|entry| entry.key.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let rows = response
        .value
        .available
        .iter()
        .map(|definition| {
            vec![
                definition.key.clone(),
                definition.name.clone(),
                definition.attribute_type.as_str().to_string(),
                if enabled_keys.contains(definition.key.as_str()) {
                    "enabled"
                } else {
                    "available"
                }
                .to_string(),
            ]
        })
        .collect::<Vec<_>>();
    let headers = ["KEY", "NAME", "TYPE", "PROJECT STATE"];
    check_columns(&headers, &session.output_options())?;
    render_list(session, &response.raw, &headers, &rows)
}

async fn set_project_enablement(
    session: &mut Session<'_>,
    key: &str,
    enabled: bool,
    reason: Option<&str>,
    idempotency_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    // The Organization definition response is the least-privilege source of
    // the exact current ETag. No project usage outside the selected Project
    // is requested or exposed by this workflow.
    let etag = match api.get_organization_attribute(&org, key).await {
        Ok(current) => required_etag(current.etag)?,
        Err(organization_error)
            if organization_error
                .as_api()
                .is_some_and(|error| matches!(error.status, 403 | 404)) =>
        {
            // A Project-restricted PAT cannot read the Organization catalog.
            // It may still change enablement for an Attribute already exposed
            // by this one Project's authorized catalog; that projection carries
            // the exact definition ETag needed for If-Match. It cannot discover
            // an unenabled definition or another Project's usage.
            let project_catalog = match api.list_project_attributes(&org, &project).await {
                Ok(catalog) => catalog,
                Err(_) => return Err(CliError::from_client(organization_error)),
            };
            let Some(definition) = project_catalog
                .value
                .available
                .iter()
                .find(|definition| definition.key == key)
            else {
                return Err(CliError::from_client(organization_error));
            };
            required_etag(definition.etag.clone())?
        }
        Err(error) => return Err(CliError::from_client(error)),
    };
    let body = SetProjectAttributeEnablementRequest {
        enabled,
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_flag)?;
    let path =
        format!("/api/v1/organizations/{org}/projects/{project}/attributes/{key}/enablement");
    if session.global.dry_run {
        return mutation_preview(
            session,
            MutationPreview {
                operation: if enabled {
                    "attribute.project.enable"
                } else {
                    "attribute.project.disable"
                },
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/attributes/{key}/enablement",
                path,
                resolved: json!({ "organization": org, "project": project, "attribute": key }),
                etag: &etag,
                idempotency: &idempotency,
                body: &body,
            },
        );
    }
    let response = api
        .set_project_attribute_enablement(&org, &project, key, &body, &etag, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    record_mutation(
        session,
        if enabled {
            "attribute.project.enable"
        } else {
            "attribute.project.disable"
        },
        key,
        response.request_id.as_deref(),
    );
    let result = response.value.clone();
    emit_view(session, &response.raw, &result.key.clone(), |session| {
        super::org::render_lines(
            session,
            &[
                ("key", result.key.clone()),
                ("enabled", result.enabled.to_string()),
                ("revision", result.revision.to_string()),
            ],
        )
    })
}

fn required_etag(etag: Option<String>) -> Result<String, CliError> {
    etag.ok_or_else(|| CliError::protocol("server did not return an Attribute ETag"))
}

fn idem_key(flag: Option<&str>) -> Result<String, CliError> {
    match flag {
        Some(key) => {
            validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
            Ok(key.to_string())
        }
        None => Ok(generate_key()),
    }
}

struct MutationPreview<'a, T> {
    operation: &'static str,
    method: &'static str,
    path_template: &'static str,
    path: String,
    resolved: Value,
    etag: &'a str,
    idempotency: &'a str,
    body: &'a T,
}

fn mutation_preview<T: serde::Serialize>(
    session: &mut Session<'_>,
    preview: MutationPreview<'_, T>,
) -> Result<(), CliError> {
    dryrun::emit_preview(
        session,
        dryrun::PreviewRequest {
            operation: preview.operation,
            method: preview.method,
            path_template: preview.path_template,
            path: preview.path,
            resolved: preview.resolved,
            if_match: Some(preview.etag),
            idempotency_key: Some(preview.idempotency),
            body: Some(serde_json::to_value(preview.body).map_err(CliError::general)?),
            notes: Vec::new(),
        },
    )
}

fn emit_definition_response(
    session: &mut Session<'_>,
    raw: Value,
    definition: AttributeDefinition,
) -> Result<(), CliError> {
    emit_view(session, &raw, &definition.key, |session| {
        render_definition(session, &definition)
    })
}

fn render_definition(
    session: &mut Session<'_>,
    definition: &AttributeDefinition,
) -> Result<(), CliError> {
    super::org::render_lines(
        session,
        &[
            ("key", definition.key.clone()),
            ("name", definition.name.clone()),
            ("type", definition.attribute_type.as_str().to_string()),
            ("state", definition.state.as_str().to_string()),
            ("revision", definition.revision.to_string()),
        ],
    )?;
    if !definition.options.is_empty() {
        session.out.line("").map_err(CliError::general)?;
        session.out.line("options:").map_err(CliError::general)?;
        for option in &definition.options {
            let state = if option.state == "retired" {
                " (retired)"
            } else {
                ""
            };
            session
                .out
                .line(&format!("  {}  {}{}", option.key, option.label, state))
                .map_err(CliError::general)?;
        }
    }
    Ok(())
}

fn record_mutation(session: &mut Session<'_>, action: &str, key: &str, request_id: Option<&str>) {
    crate::audit::record(&session.config, &mut session.out, action, key, request_id);
}
