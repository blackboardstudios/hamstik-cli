// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! First-class Organization server-side automation rule commands.

use crate::app::Session;
use crate::args::{ContractBodyArgs, InvokeArgs, RuleArgs, RuleCommand};
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

/// Runs `hamstik rule`.
pub async fn run(session: &mut Session<'_>, args: &RuleArgs) -> Result<(), CliError> {
    let invoke = match &args.command {
        RuleCommand::List { query } => invoke_args(
            "listOrganizationAutomationRules",
            vec![],
            query.query.clone(),
            ContractBodyArgs::default(),
            None,
            None,
        ),
        RuleCommand::View { rule_id } => invoke_args(
            "getOrganizationAutomationRule",
            vec![format!("ruleId={rule_id}")],
            vec![],
            ContractBodyArgs::default(),
            None,
            None,
        ),
        RuleCommand::Create {
            body,
            idempotency_key,
        } => invoke_args(
            "createOrganizationAutomationRule",
            vec![],
            vec![],
            body.clone(),
            None,
            idempotency_key.clone(),
        ),
        RuleCommand::Edit {
            rule_id,
            body,
            if_match,
            idempotency_key,
        } => invoke_args(
            "updateOrganizationAutomationRule",
            vec![format!("ruleId={rule_id}")],
            vec![],
            body.clone(),
            Some(if_match.clone()),
            idempotency_key.clone(),
        ),
        RuleCommand::Delete {
            rule_id,
            idempotency_key,
        } => invoke_args(
            "deleteOrganizationAutomationRule",
            vec![format!("ruleId={rule_id}")],
            vec![],
            ContractBodyArgs::default(),
            None,
            idempotency_key.clone(),
        ),
        RuleCommand::Runs { rule_id, query } => invoke_args(
            "listOrganizationAutomationRuleRuns",
            vec![format!("ruleId={rule_id}")],
            query.query.clone(),
            ContractBodyArgs::default(),
            None,
            None,
        ),
        RuleCommand::Preview { rule_id, body } => invoke_args(
            "previewOrganizationAutomationRule",
            vec![format!("ruleId={rule_id}")],
            vec![],
            body.clone(),
            None,
            None,
        ),
        RuleCommand::Toggle {
            rule_id,
            body,
            idempotency_key,
        } => invoke_args(
            "toggleOrganizationAutomationRule",
            vec![format!("ruleId={rule_id}")],
            vec![],
            body.clone(),
            None,
            idempotency_key.clone(),
        ),
        RuleCommand::AcceptOwner {
            rule_id,
            if_match,
            idempotency_key,
        } => invoke_args(
            "acceptOrganizationAutomationRuleOwner",
            vec![format!("ruleId={rule_id}")],
            vec![],
            ContractBodyArgs::default(),
            Some(if_match.clone()),
            idempotency_key.clone(),
        ),
    };
    api::run_invoke(session, &invoke).await
}
