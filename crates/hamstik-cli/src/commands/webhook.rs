// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! First-class Organization webhook commands backed by Public API v1
//! operation identifiers.

use crate::app::Session;
use crate::args::{ContractBodyArgs, InvokeArgs, WebhookArgs, WebhookCommand};
use crate::error::CliError;

use super::api;

fn invoke_args(
    operation_id: &str,
    parameters: Vec<String>,
    query: Vec<String>,
    body: ContractBodyArgs,
    if_match: Option<String>,
    idempotency_key: Option<String>,
) -> InvokeArgs {
    InvokeArgs {
        operation_id: operation_id.to_string(),
        param: parameters,
        query,
        body_file: body.body_file,
        field: body.field,
        upload: None,
        content_type: None,
        output: None,
        if_match,
        idempotency_key,
    }
}

/// Runs `hamstik webhook`.
pub async fn run(session: &mut Session<'_>, args: &WebhookArgs) -> Result<(), CliError> {
    let invoke = match &args.command {
        WebhookCommand::List { query } => invoke_args(
            "listOrganizationWebhooks",
            vec![],
            query.query.clone(),
            ContractBodyArgs::default(),
            None,
            None,
        ),
        WebhookCommand::View { webhook_id } => invoke_args(
            "getOrganizationWebhook",
            vec![format!("webhookId={webhook_id}")],
            vec![],
            ContractBodyArgs::default(),
            None,
            None,
        ),
        WebhookCommand::Create {
            body,
            idempotency_key,
        } => invoke_args(
            "createOrganizationWebhook",
            vec![],
            vec![],
            body.clone(),
            None,
            idempotency_key.clone(),
        ),
        WebhookCommand::Edit {
            webhook_id,
            body,
            if_match,
            idempotency_key,
        } => invoke_args(
            "updateOrganizationWebhook",
            vec![format!("webhookId={webhook_id}")],
            vec![],
            body.clone(),
            Some(if_match.clone()),
            idempotency_key.clone(),
        ),
        WebhookCommand::Delete {
            webhook_id,
            idempotency_key,
        } => invoke_args(
            "deleteOrganizationWebhook",
            vec![format!("webhookId={webhook_id}")],
            vec![],
            ContractBodyArgs::default(),
            None,
            idempotency_key.clone(),
        ),
        WebhookCommand::Rotate {
            webhook_id,
            idempotency_key,
        } => invoke_args(
            "rotateOrganizationWebhookSecret",
            vec![format!("webhookId={webhook_id}")],
            vec![],
            ContractBodyArgs::default(),
            None,
            idempotency_key.clone(),
        ),
        WebhookCommand::Test {
            webhook_id,
            idempotency_key,
        } => invoke_args(
            "testOrganizationWebhook",
            vec![format!("webhookId={webhook_id}")],
            vec![],
            ContractBodyArgs::default(),
            None,
            idempotency_key.clone(),
        ),
        WebhookCommand::Deliveries { webhook_id, query } => invoke_args(
            "listOrganizationWebhookDeliveries",
            vec![format!("webhookId={webhook_id}")],
            query.query.clone(),
            ContractBodyArgs::default(),
            None,
            None,
        ),
        WebhookCommand::Redeliver {
            webhook_id,
            delivery_id,
            idempotency_key,
        } => invoke_args(
            "redeliverOrganizationWebhookDelivery",
            vec![
                format!("webhookId={webhook_id}"),
                format!("deliveryId={delivery_id}"),
            ],
            vec![],
            ContractBodyArgs::default(),
            None,
            idempotency_key.clone(),
        ),
    };
    api::run_invoke(session, &invoke).await
}
