//! Retained process handles let ordinary removal keep its retryable map entry.

use super::*;

pub(super) struct RemovalProcesses {
    child: Option<Box<dyn portable_pty::Child + Send>>,
    background: Vec<std::process::Child>,
    process_id: Option<u32>,
    generation: Option<u64>,
    status: Arc<std::sync::Mutex<String>>,
    memory: Option<wardian_core::memory::MemoryCapabilityLease>,
    #[cfg(windows)]
    job: Option<crate::utils::process::RuntimeProcessJob>,
}

impl RemovalProcesses {
    pub(super) fn take(agent: &mut ActiveAgent) -> Self {
        Self {
            child: agent.child_process.take(),
            background: std::mem::take(&mut agent.background_processes),
            process_id: agent.process_id.take(),
            generation: agent.runtime_generation,
            status: agent.current_status.clone(),
            memory: agent.memory_capability.take(),
            #[cfg(windows)]
            job: agent.job_object.take(),
        }
    }

    /// Reattach only after the caller verifies the original Arc and generation.
    pub(super) fn restore(mut self, agent: &mut ActiveAgent) {
        agent.process_id = self.process_id;
        agent.child_process = self.child.take();
        agent.background_processes = std::mem::take(&mut self.background);
        #[cfg(windows)]
        {
            agent.job_object = self.job.take();
        }
    }

    pub(super) async fn join(&mut self) -> Result<(), String> {
        manager::clear_agent_interrupted(&self.status);
        drop(self.memory.take());
        if (self.process_id.is_some() || self.generation.is_some()) && self.child.is_none() {
            return Err("Cannot prove TUI exit: process handle is missing".into());
        }
        for child in &mut self.background {
            #[cfg(windows)]
            let _ = crate::utils::process::force_kill_process_tree(child.id());
            let _ = child.kill();
        }
        #[cfg(windows)]
        if let (Some(job), Some(child)) = (self.job.as_ref(), self.child.as_mut()) {
            if job.contained_from_launch() {
                job.stop_and_join(child.as_mut()).await?;
            } else {
                let _ = job.terminate();
                let _ = child.kill();
                return Err("Provider closure was not contained before launch".into());
            }
        }
        #[cfg(windows)]
        if self.job.is_none() {
            if let (Some(pid), Some(child)) = (self.process_id, self.child.as_mut()) {
                if child
                    .try_wait()
                    .map_err(|error| error.to_string())?
                    .is_none()
                {
                    crate::utils::process::force_kill_process_tree(pid)?;
                }
            }
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
        super::wait_for_removal_process_exit(Duration::from_secs(5), || {
            let mut exited = true;
            if let Some(child) = self.child.as_mut() {
                exited &= child
                    .try_wait()
                    .map_err(|error| error.to_string())?
                    .is_some();
            }
            for child in &mut self.background {
                exited &= child
                    .try_wait()
                    .map_err(|error| error.to_string())?
                    .is_some();
            }
            Ok(exited)
        })
        .await
    }
}
