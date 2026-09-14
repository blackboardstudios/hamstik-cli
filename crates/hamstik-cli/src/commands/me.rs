// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik me` — authenticated identity and credential context.

use crate::app::Session;
use crate::error::CliError;

use super::credential;
use super::emit_view;

/// Shows the complete `GET /me` projection.
pub async fn run(session: &mut Session<'_>) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.api(&selection)?;
    let response = api.whoami().await.map_err(CliError::from_client)?;
    let me = response.value.clone();
    let now_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let expires_at = me.authentication.expires_at.clone();
    let scopes = me.authentication.scopes.clone();
    emit_view(session, &response.raw, &me.public_id, |session| {
        session
            .out
            .line(&format!("{} <{}>", me.name, me.email))
            .map_err(CliError::general)?;
        session
            .out
            .line(&format!("  public id:  {}", me.public_id))
            .map_err(CliError::general)?;
        session
            .out
            .line(&format!(
                "  credential: {} ({})",
                me.authentication.credential_name, me.authentication.auth_type
            ))
            .map_err(CliError::general)?;
        session
            .out
            .line(&format!(
                "  expires:    {}",
                credential::expiry_summary(&expires_at, now_seconds)
            ))
            .map_err(CliError::general)?;
        session
            .out
            .line(&format!(
                "  scopes:     {}",
                if scopes.is_empty() {
                    "(none granted; mutations and most reads will be rejected)".to_string()
                } else {
                    scopes.join(", ")
                }
            ))
            .map_err(CliError::general)?;
        let default_org = me
            .default_organization
            .as_ref()
            .map(|org| org.slug.as_str())
            .unwrap_or("-");
        session
            .out
            .line(&format!("  default org: {default_org}"))
            .map_err(CliError::general)?;
        if !me.organizations.is_empty() {
            session
                .out
                .line("  memberships:")
                .map_err(CliError::general)?;
            for organization in &me.organizations {
                let username = organization.username.as_deref().unwrap_or("-");
                session
                    .out
                    .line(&format!("    {} ({username})", organization.slug))
                    .map_err(CliError::general)?;
            }
        }
        Ok(())
    })
}
