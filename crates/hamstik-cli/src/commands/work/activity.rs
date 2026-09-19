// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work activity`.

use serde_json::{Value, json};

use hamstik_api_client::{ActivityOptions, PageItems, follow_with};

use crate::app::Session;
use crate::error::CliError;
use crate::time_arg::{self, TimeArg};

use super::emit_table;
use super::follow_policy;
pub(super) async fn activity(
    session: &mut Session<'_>,
    key: &str,
    since: Option<&TimeArg>,
    pagination: &crate::args::PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    time_arg::report_resolved(&mut session.out, &[("--since", since)]);
    let opts = ActivityOptions {
        limit: pagination.page_size(),
        cursor: pagination.cursor.clone(),
        since: since.map(|d| d.to_string()),
    };
    let json_value: Value = if pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let key = key.to_string();
        let since = opts.since.clone();
        let limit = pagination.page_size();
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let project = project.clone();
            let key = key.clone();
            let since = since.clone();
            async move {
                let opts = ActivityOptions {
                    limit,
                    cursor,
                    since: since.clone(),
                };
                let response = fetch_api
                    .list_work_item_activity(&org, &project, &key, opts)
                    .await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await
        .map_err(CliError::from_client)?;
        json!({ "items": page.raw_items, "page": page.page })
    } else {
        let response = api
            .list_work_item_activity(&org, &project, key, opts)
            .await
            .map_err(CliError::from_client)?;
        response.raw
    };
    let rows: Vec<Vec<String>> = json_value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(activity_row_from_raw).collect())
        .unwrap_or_default();
    emit_table(
        session,
        &json_value,
        &["ID", "ACTION", "ACTOR", "DETAIL", "CREATED"],
        &rows,
    )
}

fn activity_row_from_raw(raw: &Value) -> Vec<String> {
    let detail = raw
        .get("detail")
        .filter(|d| !d.is_null())
        .map(Value::to_string)
        .unwrap_or_else(|| "-".to_string());
    vec![
        raw.get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        raw.get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        raw.get("actor")
            .and_then(|a| a.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        detail,
        raw.get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    ]
}
