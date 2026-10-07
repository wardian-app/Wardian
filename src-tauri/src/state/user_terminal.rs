use std::sync::{Arc, Mutex};

pub struct UserTerminalSession {
    pub session_id: String,
    pub shell_id: String,
    pub child_process: Option<Box<dyn portable_pty::Child + Send>>,
    pub pty_master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    pub stdin_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    pub output_buffer: Arc<Mutex<String>>,
    pub process_id: Option<u32>,
    pub exited: Arc<Mutex<bool>>,
    #[cfg(windows)]
    pub job_object: Option<crate::utils::process::RuntimeProcessJob>,
}

impl UserTerminalSession {
    /// Windows replacement requires verified exit of the original child and job
    /// members. Keep the session in its slot if stopping fails or is cancelled.
    pub(crate) async fn take_for_replacement(session: &mut Option<Self>) -> Result<(), String> {
        #[cfg(windows)]
        if let Some(terminal) = session.as_mut() {
            let child = terminal
                .child_process
                .as_mut()
                .ok_or("Previous user terminal has no retained child handle")?;
            let job = terminal
                .job_object
                .as_ref()
                .ok_or("Previous user terminal has no retained containment job")?;
            job.stop_and_join(child.as_mut())
                .await
                .map_err(|error| format!("Failed to stop previous user terminal: {error}"))?;
        }
        session.take();
        Ok(())
    }
}

impl Drop for UserTerminalSession {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            if let Some(job) = &self.job_object {
                let _ = job.terminate();
                self.process_id = None;
            } else if let Some(pid) = self.process_id.take() {
                let _ = crate::utils::process::force_kill_process_tree(pid);
            }
        }

        if let Some(mut child) = self.child_process.take() {
            let _ = child.kill();
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct UnusedMaster;

    impl portable_pty::MasterPty for UnusedMaster {
        fn resize(&self, _size: portable_pty::PtySize) -> anyhow::Result<()> {
            panic!("replacement must not resize the original PTY")
        }

        fn get_size(&self) -> anyhow::Result<portable_pty::PtySize> {
            panic!("replacement must not query the original PTY")
        }

        fn try_clone_reader(&self) -> anyhow::Result<Box<dyn std::io::Read + Send>> {
            panic!("replacement must not replace the original reader")
        }

        fn take_writer(&self) -> anyhow::Result<Box<dyn std::io::Write + Send>> {
            panic!("replacement must not replace the original writer")
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum WaitResult {
        Exited,
        Uncertain,
        Error,
    }

    #[derive(Debug, Clone)]
    struct CapturedChild {
        result: WaitResult,
        polls: Arc<AtomicUsize>,
        kills: Arc<AtomicUsize>,
    }

    impl portable_pty::ChildKiller for CapturedChild {
        fn kill(&mut self) -> std::io::Result<()> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(self.clone())
        }
    }

    impl portable_pty::Child for CapturedChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            match self.result {
                WaitResult::Exited => Ok(Some(portable_pty::ExitStatus::with_exit_code(0))),
                WaitResult::Uncertain => Ok(None),
                WaitResult::Error => Err(std::io::Error::other("original child wait failed")),
            }
        }

        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            panic!("replacement must poll the retained child without blocking")
        }

        fn process_id(&self) -> Option<u32> {
            None
        }

        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    fn captured_session(result: WaitResult) -> (Option<UserTerminalSession>, CapturedChild) {
        let child = CapturedChild {
            result,
            polls: Arc::new(AtomicUsize::new(0)),
            kills: Arc::new(AtomicUsize::new(0)),
        };
        // An empty real job exercises the stop API; the synthetic child models
        // uncertain wait results without opening or terminating an arbitrary PID.
        let mut command = portable_pty::CommandBuilder::new("unused-test-command");
        let job = crate::utils::process::RuntimeProcessJob::prepare(&mut command).unwrap();
        let (stdin_tx, _rx) = tokio::sync::mpsc::channel(1);
        let session = UserTerminalSession {
            session_id: "original-terminal".to_string(),
            shell_id: "pwsh".to_string(),
            child_process: Some(Box::new(child.clone())),
            pty_master: Arc::new(Mutex::new(Box::new(UnusedMaster))),
            stdin_tx,
            output_buffer: Arc::new(Mutex::new("original output".to_string())),
            process_id: None,
            exited: Arc::new(Mutex::new(false)),
            job_object: Some(job),
        };
        (Some(session), child)
    }

    #[tokio::test]
    async fn replacement_wait_error_retains_the_original_session_and_handles() {
        let (mut session, child) = captured_session(WaitResult::Error);

        let error = UserTerminalSession::take_for_replacement(&mut session)
            .await
            .unwrap_err();

        assert!(error.contains("original child wait failed"));
        let original = session.as_ref().unwrap();
        assert_eq!(original.session_id, "original-terminal");
        assert!(original.child_process.is_some());
        assert!(original.job_object.is_some());
        assert_eq!(*original.output_buffer.lock().unwrap(), "original output");
        assert_eq!(child.polls.load(Ordering::SeqCst), 1);
        assert_eq!(child.kills.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancelled_replacement_retains_the_original_session_and_handles() {
        let (mut session, child) = captured_session(WaitResult::Uncertain);

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            UserTerminalSession::take_for_replacement(&mut session),
        )
        .await;

        assert!(result.is_err());
        let original = session.as_ref().unwrap();
        assert_eq!(original.session_id, "original-terminal");
        assert!(original.child_process.is_some());
        assert!(original.job_object.is_some());
        assert!(child.polls.load(Ordering::SeqCst) > 0);
        assert_eq!(child.kills.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn replacement_without_a_containment_job_retains_the_original_session() {
        let (mut session, child) = captured_session(WaitResult::Exited);
        session.as_mut().unwrap().job_object.take();

        let error = UserTerminalSession::take_for_replacement(&mut session)
            .await
            .unwrap_err();

        assert!(error.contains("no retained containment job"));
        assert!(session.as_ref().unwrap().child_process.is_some());
        assert_eq!(child.polls.load(Ordering::SeqCst), 0);
        assert_eq!(child.kills.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn verified_exit_releases_the_session_slot() {
        let (mut session, child) = captured_session(WaitResult::Exited);

        UserTerminalSession::take_for_replacement(&mut session)
            .await
            .unwrap();

        assert!(session.is_none());
        assert_eq!(child.polls.load(Ordering::SeqCst), 1);
    }
}
