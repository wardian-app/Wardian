//! Startup diagnostics for holds without durable creator and containment proof.
//!
//! A desktop lock excludes another cooperating desktop; it is not a receipt
//! that an old provider tree exited. Legacy records encode neither the creating
//! instance nor the actual launch containment. Never infer release authority
//! from a provider name, hold age, or absence of a candidate process.

use crate::state::{AppState, InteractionState};
use tauri::Emitter;
use wardian_core::control::{InboxNotificationKind, InboxNotificationPayload};
use wardian_core::conversation_lease::ConversationLease;
use wardian_core::models::AgentConfig;

struct RetainedHoldReport {
    agent_id: String,
    acquisition_id: String,
    provider: String,
    resume_session: String,
    reason: String,
    recovery_command: Option<String>,
    blocks_saved_session: bool,
}

fn command_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn plan_retained_holds(
    configs: &[AgentConfig],
    leases: &[ConversationLease],
) -> Vec<RetainedHoldReport> {
    leases
        .iter()
        .filter(|hold| hold.owner_kind == "prior_provider_hold" && hold.mode == "uncertain_previous_provider")
        .map(|hold| {
            let config = configs.iter().find(|config| config.session_id == hold.agent_id);
            let blocks_saved_session = config.is_some_and(|config| {
                let session = super::provider_restore_session_identity(config);
                !session.is_empty() && session == hold.resume_session
            });
            let mut reason = "The hold has no durable creating-instance or launch-containment binding. Acquiring the desktop lock or comparing hold age cannot prove that the old provider tree exited.".to_string();
            if hold.provider == "codex" {
                reason.push_str(" Some legacy Codex TUI paths used a breakaway-enabled supervisor, and app-server job assignment followed process spawn. This hold does not identify its launch path.");
                reason.push_str(" Terminal exit also does not exclude a thread writer hosted by an out-of-job Codex daemon. Exact thread ownership, supported unload evidence, and verified owner-tree exit are required for automatic release.");
            } else {
                reason.push_str(" The provider name alone does not establish the historical runtime's launch path.");
            }
            let recovery_command = (config.is_some()
                && command_identifier(&hold.agent_id)
                && command_identifier(&hold.acquisition_id))
            .then(|| format!(
                "wardian agent release-hold {} --acquisition-id {} --operator-verified-exit",
                hold.agent_id, hold.acquisition_id
            ));
            RetainedHoldReport {
                agent_id: hold.agent_id.clone(),
                acquisition_id: hold.acquisition_id.clone(),
                provider: hold.provider.clone(),
                resume_session: hold.resume_session.clone(),
                reason,
                recovery_command,
                blocks_saved_session,
            }
        })
        .collect()
}

async fn persist_reports(
    interactions: &InteractionState,
    reports: Vec<RetainedHoldReport>,
) -> Result<usize, String> {
    let mut notifications = 0;
    for report in reports {
        crate::manager::log_debug(&format!(
            "[WARDIAN] Startup retained prior-provider hold for {} acquisition {} ({} / {}): {} Recovery: {}",
            report.agent_id, report.acquisition_id, report.provider, report.resume_session,
            report.reason, report.recovery_command.as_deref().unwrap_or("inspect exact hold identity before operator recovery")
        ));
        // Older, non-current conversations remain protected and inspectable in
        // the startup log without flooding Inbox with non-blocking holds.
        if !report.blocks_saved_session {
            continue;
        }
        let body = format!(
            "Restart did not release the {} hold for provider session {} (acquisition {}). Reusing this saved session remains blocked. {} Use the recovery command only after independently verifying old-provider exit; an empty process scan is not proof.",
            report.provider, report.resume_session, report.acquisition_id, report.reason
        );
        interactions
            .create_notification_durable(
                report.agent_id,
                InboxNotificationPayload {
                    kind: InboxNotificationKind::Update,
                    title: "Startup could not verify prior-provider exit".into(),
                    body,
                    proposed_action: report.recovery_command,
                    risk: Some("The old provider tree's exit is unproven; its exact hold remains protected.".into()),
                    choices: Vec::new(),
                    expires_at: None,
                },
            )
            .await
            .map_err(|error| format!("Could not persist startup hold diagnostic: {error}"))?;
        notifications += 1;
    }
    Ok(notifications)
}

/// Report every unproven legacy hold before provider restoration. This path is
/// deliberately read-only with respect to leases, conversations, and history.
pub(crate) async fn report_unverified_holds(
    app: &tauri::AppHandle,
    state: &AppState,
    configs: &[AgentConfig],
    leases: &[ConversationLease],
) {
    match persist_reports(&state.interactions, plan_retained_holds(configs, leases)).await {
        Ok(count) if count > 0 => {
            let _ = app.emit("inbox-updated", ());
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("{error}");
            crate::manager::log_debug(&error);
        }
    }
}

#[cfg(test)]
mod tests;
