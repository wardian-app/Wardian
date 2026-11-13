//! Writer evidence comes from the captured private owner, never terminal exit alone.
use super::{attachment, CodexSharedOwner};
use crate::delivery::codex_shared::{json, CodexSharedError};
use std::sync::Arc;
use std::time::Duration;
use wardian_core::conversation_lease::ConversationLease;

/// Receipt issued only after supported unload evidence and a verified owner-tree join.
/// Its identity cannot be supplied by a caller claiming that a terminal exited.
///
/// ```compile_fail
/// use wardian_app_lib::delivery::codex_shared::CodexThreadExit;
/// let forged = CodexThreadExit {
///     agent_id: "agent".into(), generation: 7, thread_id: "thread".into(),
/// };
/// ```
#[derive(Debug)]
pub struct CodexThreadExit {
    agent_id: String,
    generation: u64,
    thread_id: String,
}

impl CodexThreadExit {
    /// Compare the runtime incarnation and complete persisted hold under the
    /// lease locks. A retargeted or replacement acquisition cannot be released.
    pub(crate) fn release_hold(
        &self,
        hold: &ConversationLease,
        generation: u64,
    ) -> Result<(), String> {
        wardian_core::conversation_lease::release_previous_provider_hold_after_exit(
            hold,
            |current| {
                if self.generation != generation
                    || self.agent_id != current.agent_id
                    || self.thread_id != current.resume_session
                    || current.provider != "codex"
                {
                    return Err("Codex writer exit receipt does not match the held session".into());
                }
                Ok(())
            },
        )
    }
}

impl CodexSharedOwner {
    /// Caller holds the generation's owner gate and has already joined its TUI
    /// tree. Unavailable evidence retains the hold, but does not prevent cleanup.
    pub(crate) async fn stop_after_tui(
        &self,
        agent_id: &str,
        generation: u64,
        thread_id: Option<&str>,
        tui_tree_exited: bool,
    ) -> Result<Option<Arc<CodexThreadExit>>, CodexSharedError> {
        let identity = self.client.receipt("stopping").ok().filter(|receipt| {
            receipt.wardian_agent_id == agent_id
                && receipt.generation == generation
                && Some(receipt.provider_session_id.as_str()) == thread_id
                && !receipt.provider_session_id.is_empty()
                && generation != 0
                && self
                    .verified_thread
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .as_deref()
                    == thread_id
        });
        // Only Windows has retained whole-tree containment for this owner.
        let (unloaded, reason) = if !cfg!(windows) {
            (
                false,
                "owner-tree exit verification is unavailable on this platform",
            )
        } else if !tui_tree_exited {
            (false, "contained terminal-tree exit was not verified")
        } else if let Some(identity) = &identity {
            (
                require_unloaded(&self.client, &identity.provider_session_id)
                    .await
                    .is_ok(),
                "the captured owner's supported API did not prove thread unload",
            )
        } else {
            (
                false,
                "the thread is not verified on this exact private owner and generation",
            )
        };
        self.shutdown().await?;
        if unloaded {
            let identity = identity.expect("unload evidence requires an exact bound owner");
            Ok(Some(Arc::new(CodexThreadExit {
                agent_id: identity.wardian_agent_id,
                generation: identity.generation,
                thread_id: identity.provider_session_id,
            })))
        } else {
            if thread_id.is_some() {
                crate::manager::log_debug(&format!("[WARDIAN] Codex hold retained: {reason}"));
            }
            Ok(None)
        }
    }
}

async fn require_unloaded(
    client: &crate::delivery::codex_shared::CodexSharedClient,
    thread_id: &str,
) -> Result<(), CodexSharedError> {
    tokio::time::timeout(Duration::from_secs(5), async {
        // An empty cache is authoritative only on the exact captured private
        // owner. Do not discover or query a machine-wide daemon in its place.
        if attachment::observed_loaded_thread(client, Some(thread_id))
            .await?
            .is_none()
        {
            return Ok(());
        }
        let response = client
            .request_with_timeout(
                "thread/unsubscribe",
                json!({"threadId": thread_id}),
                Duration::from_secs(5),
            )
            .await?;
        if response["status"] != "unsubscribed" {
            return Err(CodexSharedError::unsupported(
                "thread unload was not acknowledged",
            ));
        }
        loop {
            if attachment::observed_loaded_thread(client, Some(thread_id))
                .await?
                .is_none()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| CodexSharedError::uncertain("thread unload evidence timed out"))?
}

#[cfg(all(test, windows))]
#[path = "owner_stop_tests.rs"]
pub(crate) mod tests;
