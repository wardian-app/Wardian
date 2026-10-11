//! Workspace handover must receive owned runtime-tree exit before log rotation.
use crate::state::ActiveAgent;

pub(super) fn validate_stop_authority(agent: &ActiveAgent) -> Result<(), String> {
    #[cfg(windows)]
    {
        let no_child = agent.child_process.is_none() && agent.background_processes.is_empty();
        if no_child
            && agent.runtime_generation.is_none()
            && agent.process_id.is_none()
            && agent.job_object.is_none()
        {
            return Ok(());
        }
        if agent.child_process.is_some()
            && agent.background_processes.is_empty()
            && agent
                .job_object
                .as_ref()
                .is_some_and(|job| job.contained_from_launch())
        {
            return Ok(());
        }
        Err("Workspace handover requires a provider runtime with verifiable owned shutdown; this runtime has no supported tree-exit receipt".into())
    }
    #[cfg(not(windows))]
    {
        let _ = agent;
        Err("Active worktree handover is not qualified on this platform; turn the agent Off before assigning a different workspace".into())
    }
}

pub(super) async fn stop_with_receipt(
    agent: &mut ActiveAgent,
    hold: Option<&wardian_core::conversation_lease::ConversationLeaseOwner>,
) -> Result<(), String> {
    validate_stop_authority(agent)?;
    #[cfg(windows)]
    if agent.child_process.is_some() {
        let job = agent.job_object.take().expect("validated contained job");
        let mut child = agent.child_process.take().expect("validated owned child");
        agent.process_id = None;
        agent.memory_capability.take();
        // Keep the job and child handles across the await. A timeout, failure,
        // or cancellation is not a receipt; the caller retains its fence.
        job.stop_and_join(child.as_mut()).await?;
    }
    agent.memory_capability.take();
    if let Some(hold) = hold {
        wardian_core::conversation_lease::release_lease_owner_persisted(hold)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "workspace_stop_tests.rs"]
mod tests;
