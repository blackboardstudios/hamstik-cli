// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik auth` (login / status / list / switch / logout / forget).

use secrecy::SecretString;
use serde_json::{json, value::Value};

use crate::app::Session;
use crate::args::{AuthArgs, AuthCommand};
use crate::config::{self, Profile};
use crate::credentials;
use crate::error::CliError;
use crate::input::read_token;

use super::credential;
use super::{check_columns, emit_json, emit_view, render_list};

/// Runs the `auth` subcommands.
pub async fn run(session: &mut Session<'_>, args: &AuthArgs) -> Result<(), CliError> {
    match &args.command {
        AuthCommand::Login { with_token } => login(session, *with_token).await,
        AuthCommand::Status => status(session).await,
        AuthCommand::List => list(session),
        AuthCommand::Switch { profile } => switch(session, profile),
        AuthCommand::Logout => logout(session),
        AuthCommand::Forget { profile } => forget(session, profile.as_deref()),
    }
}

/// Validates a token and wraps it in a zeroizing [`SecretString`].
///
/// The value is moved into the zeroizing backing directly; intermediate plain
/// copies are avoided.
fn token_secret(raw: String) -> Result<SecretString, CliError> {
    crate::input::token_to_secret(&raw).map_err(CliError::usage)
}

/// Builds an error that also drops a just-stored credential, so a failed login
/// leaves no orphaned secret behind.
fn login_rollback(session: &Session<'_>, account: &str, err: CliError) -> CliError {
    let _ = session.store.delete(account);
    err
}

async fn login(session: &mut Session<'_>, with_token: bool) -> Result<(), CliError> {
    let selection = session.selection()?;

    let raw: String = if with_token {
        let stdin = std::io::stdin();
        read_token(stdin.lock())
            .map_err(|err| CliError::usage(format!("cannot read token: {err}")))?
    } else if session.env.var("HAMSTIK_TOKEN").is_some() {
        // SPEC §27/PRD §6.3: an environment token is ephemeral and must never
        // be written to the credential store. Persisting one is an explicit
        // act (`--with-token`), never an implicit side effect of login.
        return Err(CliError::usage(
            "HAMSTIK_TOKEN is ephemeral and is never stored; use \
             `hamstik auth login --with-token` to persist a token, or unset \
             HAMSTIK_TOKEN to log in interactively",
        ));
    } else if session.can_prompt() {
        session
            .prompt
            .read_secret("Personal Access Token: ")
            .map_err(|err| CliError::general(format!("prompt failed: {err}")))?
    } else {
        return Err(CliError::usage(
            "no token provided; use --with-token or run interactively",
        ));
    };

    let secret = token_secret(raw)?;

    let api = session.build_client(selection.host.clone(), secret.clone())?;
    let me = api.whoami().await.map_err(CliError::from_client)?;
    super::warn_on_context_drift(session, &selection, &me.value);

    let mut config = session.config.load()?;
    let base = config::profile_auto_name(selection.host.as_str(), &me.value.email);
    let name = config::unique_profile_name(&config, &base, &me.value.id, selection.host.as_str());
    let account = credentials::account_key(selection.host.as_str(), &me.value.id);

    session
        .store
        .set(&account, &secret)
        .map_err(|err| CliError::credential(format!("cannot store credential: {err}")))?;

    let profile = Profile {
        host: selection.host.as_str().to_string(),
        user_id: me.value.id.clone(),
        email: me.value.email.clone(),
        default_organization: me
            .value
            .default_organization
            .as_ref()
            .map(|org| org.slug.clone()),
        default_project: None,
    };
    config.profiles.insert(name.clone(), profile);
    config.active_profile = Some(name.clone());
    if let Err(err) = session.config.save(&config) {
        // The profile entry did not land; do not leave an unreachable secret
        // in the OS store.
        return Err(login_rollback(session, &account, err));
    }
    // The profile name only: tokens never reach the audit log.
    crate::audit::record(&session.config, &mut session.out, "auth.login", &name, None);

    if session.json() {
        emit_json(
            session,
            &json!({
                "authenticated": true,
                "profile": name,
                "host": selection.host.as_str(),
                "user": { "id": me.value.id, "email": me.value.email },
                "scopes": me.value.authentication.scopes,
            }),
        )
    } else {
        session
            .out
            .line(&format!(
                "Authenticated as {} ({})",
                me.value.email, selection.host
            ))
            .map_err(CliError::general)?;
        session
            .out
            .line(&format!("Profile: {name}"))
            .map_err(CliError::general)?;
        Ok(())
    }
}

async fn status(session: &mut Session<'_>) -> Result<(), CliError> {
    let selection = session.selection()?;
    let secret = resolve_status_token(session, &selection)?;
    let token_source = if session.env.var("HAMSTIK_TOKEN").is_some() {
        "environment (HAMSTIK_TOKEN; ephemeral, never stored)"
    } else {
        "credential store"
    };

    let api = session.build_client(selection.host.clone(), secret)?;
    let me = match api.whoami().await {
        Ok(me) => me,
        Err(error) => {
            // Structured errors keep code/status/request id verbatim; only
            // the remediation is CLI-added. `CliError` carries its fields in
            // an Arc-style shared core, so a remediated copy is built from
            // the captured fields without exposing any secret material.
            let cli = CliError::from_client(error);
            let message_suffix = match (cli.status, cli.code.as_str()) {
                (Some(401), _) => Some(
                    " (credential rejected by the server; create a new PAT and run `hamstik auth login --with-token`)".to_string(),
                ),
                (Some(403), _) => Some(
                    " (the server rejected authorization for these scopes; request the missing scope from your administrator)".to_string(),
                ),
                _ => None,
            };
            if let Some(suffix) = message_suffix {
                return Err(cli.with_reminded_remediation(suffix));
            }
            return Err(cli);
        }
    };

    super::warn_on_context_drift(session, &selection, &me.value);

    let now_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let expires_at = me.value.authentication.expires_at.clone();
    let scopes = me.value.authentication.scopes.clone();
    let scope_report = credential::scope_report(&scopes);

    if session.json() {
        emit_json(
            session,
            &json!({
                "authenticated": true,
                "host": selection.host.as_str(),
                "profile": selection.profile,
                "credentialSource": token_source,
                "user": {
                    "id": me.value.id,
                    "publicId": me.value.public_id,
                    "email": me.value.email,
                    "name": me.value.name,
                    "organizations": me.value.organizations,
                },
                "credential": {
                    "name": me.value.authentication.credential_name,
                    "type": me.value.authentication.auth_type,
                    "expiresAt": expires_at,
                },
                "scopes": scope_report,
            }),
        )?;

        // Near-expiry warnings are nonblocking advisories; a client-detected
        // expiry fails with the stable authentication exit code because every
        // subsequent request would be rejected.
        let expiry = credential::classify_expiry(&expires_at, now_seconds);
        match expiry {
            credential::Expiry::Expired => {
                session.out.warn(&format!(
                    "credential EXPIRED {expires_at}; every request will be rejected - create a new PAT and run `hamstik auth login --with-token`"
                ));
                session.exit_code = crate::exit::AUTHENTICATION;
            }
            credential::Expiry::Approaching(_) => {
                session
                    .out
                    .warn(&credential::expiry_summary(&expires_at, now_seconds));
            }
            credential::Expiry::Valid(_) | credential::Expiry::Unknown => {}
        }
        return Ok(());
    }

    session
        .out
        .line(&format!("Authenticated as {}", me.value.email))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("  host:      {}", selection.host))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("  source:    {token_source}"))
        .map_err(CliError::general)?;
    if let Some(profile) = &selection.profile {
        session
            .out
            .line(&format!("  profile:   {profile}"))
            .map_err(CliError::general)?;
    }
    session
        .out
        .line(&credential::scope_summary_line(&scopes))
        .map_err(CliError::general)?;
    if !session.out.is_quiet() {
        session
            .out
            .line(&format!(
                "  expiry:    {}",
                credential::expiry_summary(&expires_at, now_seconds)
            ))
            .map_err(CliError::general)?;
        if scopes.is_empty() {
            session
                .out
                .line("  warning:   no scopes granted; ask your administrator for the families you need")
                .map_err(CliError::general)?;
        }
    }
    if !me.value.organizations.is_empty() {
        session
            .out
            .line("  memberships:")
            .map_err(CliError::general)?;
        for org in &me.value.organizations {
            let username = org.username.clone().unwrap_or_else(|| "-".to_string());
            session
                .out
                .line(&format!(
                    "    {} ({}, username: {username})",
                    org.slug, org.name
                ))
                .map_err(CliError::general)?;
        }
    }
    Ok(())
}

/// Resolves the token for status without surfacing a usage error when absent.
fn resolve_status_token(
    session: &Session<'_>,
    selection: &crate::app::Selection,
) -> Result<SecretString, CliError> {
    if let Some(token) = session.env.var("HAMSTIK_TOKEN") {
        return token_secret(token);
    }
    let profile = selection
        .profile_meta
        .as_ref()
        .ok_or_else(|| CliError::auth("not authenticated; run `hamstik auth login`"))?;
    // Try the selected host first, then the profile host (see `Session::token_for`).
    let lookup_hosts = if selection.host.as_str() == profile.host.as_str() {
        vec![selection.host.as_str()]
    } else {
        vec![selection.host.as_str(), profile.host.as_str()]
    };
    for host in &lookup_hosts {
        let account = credentials::account_key(host, &profile.user_id);
        if let Some(secret) = session
            .store
            .get(&account)
            .map_err(|err| CliError::credential(format!("credential store unavailable: {err}")))?
        {
            return Ok(secret);
        }
    }
    Err(CliError::auth(
        "no stored credential; run `hamstik auth login`",
    ))
}

/// The `auth list` table columns, in their documented order.
///
/// The first column marks the selected profile with `*`. It carries a real
/// header name (rather than a blank one) so `--tsv` never emits an unnamed
/// column and `--columns ACTIVE` can select or reorder it like any other.
const AUTH_LIST_COLUMNS: &[&str] = &["ACTIVE", "NAME", "HOST", "EMAIL", "ORG"];

fn list(session: &mut Session<'_>) -> Result<(), CliError> {
    let config = session.config.load()?;
    let active = session
        .global
        .profile
        .clone()
        .or_else(|| session.env.var("HAMSTIK_PROFILE"))
        .or_else(|| config.active_profile.clone());

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut items: Vec<Value> = Vec::new();
    for (name, profile) in &config.profiles {
        let is_active = active.as_deref() == Some(name.as_str());
        rows.push(vec![
            if is_active {
                "*".to_string()
            } else {
                String::new()
            },
            name.clone(),
            profile.host.clone(),
            profile.email.clone(),
            profile.default_organization.clone().unwrap_or_default(),
        ]);
        items.push(json!({
            "name": name,
            "host": profile.host,
            "email": profile.email,
            "defaultOrganization": profile.default_organization,
            "active": is_active,
        }));
    }

    let json_value = json!({ "profiles": items });
    check_columns(AUTH_LIST_COLUMNS, &session.output_options())?;
    render_list(session, &json_value, AUTH_LIST_COLUMNS, &rows)
}

fn switch(session: &mut Session<'_>, name: &str) -> Result<(), CliError> {
    let mut config = session.config.load()?;
    if !config.profiles.contains_key(name) {
        // Report invalid names clearly; existing (possibly legacy) names are
        // always switchable, so validation only guards genuinely bad input.
        crate::config::validate_profile_name(name)?;
        return Err(CliError::config(format!(
            "no such profile: {name}{}",
            configured_profiles_hint(&config)
        )));
    }
    config.active_profile = Some(name.to_string());
    session.config.save(&config)?;
    crate::audit::record(&session.config, &mut session.out, "auth.switch", name, None);
    emit_view(
        session,
        &json!({ "activeProfile": name }),
        name,
        |session| {
            session
                .out
                .line(&format!("Active profile: {name}"))
                .map_err(CliError::general)
        },
    )
}

fn logout(session: &mut Session<'_>) -> Result<(), CliError> {
    if session.env.var("HAMSTIK_TOKEN").is_some() {
        return Err(CliError::usage(
            "HAMSTIK_TOKEN is set; logout only removes stored credentials (unset the variable to stop using it)",
        ));
    }
    let selection = session.selection()?;
    let name = selection
        .profile
        .clone()
        .ok_or_else(|| CliError::usage("no active profile to log out"))?;
    let profile = selection
        .profile_meta
        .as_ref()
        .ok_or_else(|| CliError::config(format!("profile {name:?} is not configured")))?;

    // The credential is keyed by the resolved host (the key every command
    // reads), so a host/profile mismatch means the selected profile owns no
    // credential for this host and nothing must be deleted.
    if profile.host != selection.host.as_str() {
        return Err(CliError::usage(format!(
            "profile {name:?} belongs to {}, not {}; choose a profile with --profile",
            profile.host, selection.host
        )));
    }

    let account = credentials::account_key(selection.host.as_str(), &profile.user_id);
    // Read first so a second logout says "already logged out" instead of
    // claiming a removal that never happened.
    let had_credential = remove_credential(session, &account)?;
    crate::audit::record(
        &session.config,
        &mut session.out,
        "auth.logout",
        &name,
        None,
    );

    if session.json() {
        return emit_json(
            session,
            &json!({
                "logout": true,
                "profile": name,
                "host": selection.host.as_str(),
                "credentialRemoved": had_credential,
                // SPEC §29: local logout never revokes the PAT server-side.
                "revoked": false,
                "profileRetained": true,
            }),
        );
    }

    let summary = if had_credential {
        format!(
            "Removed stored credential for profile {name:?} ({})",
            selection.host
        )
    } else {
        format!("No stored credential for profile {name:?} (already logged out)")
    };
    session.out.line(&summary).map_err(CliError::general)?;
    if !session.out.is_quiet() {
        session
            .out
            .line("  local logout only; the PAT is not revoked server-side")
            .map_err(CliError::general)?;
        session
            .out
            .line(&format!(
                "  profile metadata remains in {}; `auth list` still shows it",
                session.config.path().display()
            ))
            .map_err(CliError::general)?;
    }
    Ok(())
}

/// Logs out and forgets a profile: removes its stored credential and its entry
/// in the config file.
///
/// Unlike `logout`, this addresses a profile by identity (the host recorded in
/// the profile), so `--host` plays no part. An unreachable credential store
/// must not leave the user stuck, so the profile entry is removed either way
/// and the exact account key is reported for manual cleanup.
fn forget(session: &mut Session<'_>, requested: Option<&str>) -> Result<(), CliError> {
    let mut config = session.config.load()?;
    let name = requested
        .map(str::to_string)
        .or_else(|| config.active_profile.clone())
        .ok_or_else(|| {
            CliError::usage(
                "no profile to forget; name one (see `hamstik auth list`) or log in first",
            )
        })?;

    let Some(profile) = config::forget_profile(&mut config, &name) else {
        return Err(CliError::usage(format!(
            "no such profile: {name}{}",
            configured_profiles_hint(&config)
        )));
    };

    let account = credentials::account_key(&profile.host, &profile.user_id);
    let (credential_removed, store_problem) = match remove_credential(session, &account) {
        Ok(removed) => (removed, None),
        Err(err) => (false, Some(err.message)),
    };

    session.config.save(&config)?;
    crate::audit::record(
        &session.config,
        &mut session.out,
        "auth.forget",
        &name,
        None,
    );

    if let Some(problem) = &store_problem {
        session.out.warn(&format!(
            "the credential for {account:?} was NOT removed ({problem}); delete it from the OS store by hand"
        ));
    }

    if session.json() {
        let mut value = json!({
            "forget": true,
            "profile": name,
            "host": profile.host,
            "credentialRemoved": credential_removed,
            // Local-only removal: the PAT itself stays valid server-side.
            "revoked": false,
            "activeProfile": config.active_profile,
        });
        if let Some(problem) = store_problem {
            value["credentialError"] = json!(problem);
        }
        return emit_json(session, &value);
    }

    if !session.out.is_quiet() {
        let credential_state = if store_problem.is_some() {
            "removal failed (see above)"
        } else if credential_removed {
            "removed"
        } else {
            "was not stored"
        };
        session
            .out
            .line(&format!(
                "Forgot profile {name:?} ({}) - credential {credential_state}",
                profile.host
            ))
            .map_err(CliError::general)?;
        let active_note = match &config.active_profile {
            Some(active) => format!("  active profile is now {active:?}"),
            None => {
                "  no active profile; run `hamstik auth login` or `hamstik auth switch`".to_string()
            }
        };
        session.out.line(&active_note).map_err(CliError::general)?;
        session
            .out
            .line("  local only; the PAT is not revoked server-side")
            .map_err(CliError::general)?;
    }
    Ok(())
}

/// Deletes `account`, reporting whether a credential was there to delete.
fn remove_credential(session: &Session<'_>, account: &str) -> Result<bool, CliError> {
    let existing = session
        .store
        .get(account)
        .map_err(|err| CliError::credential(format!("credential store unavailable: {err}")))?;
    if existing.is_none() {
        return Ok(false);
    }
    session
        .store
        .delete(account)
        .map_err(|err| CliError::credential(format!("cannot delete credential: {err}")))?;
    Ok(true)
}

/// Lists the configured profiles for "no such profile" messages.
fn configured_profiles_hint(config: &config::ConfigFile) -> String {
    if config.profiles.is_empty() {
        return "; no profiles are configured (run `hamstik auth login`)".to_string();
    }
    let mut names: Vec<&str> = config.profiles.keys().map(String::as_str).collect();
    names.sort_unstable();
    format!("; configured profiles: {}", names.join(", "))
}
