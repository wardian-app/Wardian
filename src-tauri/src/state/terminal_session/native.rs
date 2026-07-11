use super::{TerminalBrokerError, TerminalRuntimeHandles, TerminalSessionBroker};
use portable_pty::{MasterPty, PtySize};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;
use wardian_core::models::TerminalGeometry;

pub type SharedPtyMaster = Arc<Mutex<Box<dyn MasterPty + Send>>>;

static NATIVE_PTY_RESIZE_GATE: OnceLock<Arc<Mutex<()>>> = OnceLock::new();

/// Process-wide native PTY resize gate. ConPTY resize calls are serialized even
/// when several broker actors (or the standalone user terminal) resize at once.
pub fn native_pty_resize_gate() -> Arc<Mutex<()>> {
    NATIVE_PTY_RESIZE_GATE
        .get_or_init(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Moves native input and resize authority into the terminal-session actor.
pub fn native_terminal_runtime(
    input_tx: mpsc::Sender<Vec<u8>>,
    master: SharedPtyMaster,
) -> TerminalRuntimeHandles {
    TerminalRuntimeHandles::new(input_tx, move |geometry| {
        resize_native_master(master.clone(), geometry)
    })
}

fn resize_native_master(master: SharedPtyMaster, geometry: TerminalGeometry) -> Result<(), String> {
    let resize_gate = native_pty_resize_gate();
    let _gate = resize_gate
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let master = master
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    master
        .resize(PtySize {
            rows: geometry.rows,
            cols: geometry.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Failed to resize PTY: {error}"))
}

/// Reader threads capture the generation returned at spawn. The broker rejects
/// bytes from a stale reader after clear/resume replaces the native runtime.
pub fn forward_terminal_output(
    broker: &TerminalSessionBroker,
    session_id: &str,
    runtime_generation: u64,
    bytes: impl AsRef<[u8]>,
) -> Result<(), TerminalBrokerError> {
    broker.process_output_blocking(session_id, runtime_generation, bytes.as_ref().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::terminal_session::{TerminalClientIdentity, TerminalSessionBroker};
    use anyhow::Error;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;
    use wardian_core::models::{
        TerminalActivationAckRequest, TerminalActivationBeginRequest, TerminalClientKind,
        TerminalGeometry, TerminalGeometryRequest, TerminalLeaseIdentity,
        TerminalPresentationRegistration, TerminalRenderState, TerminalRequestedInteraction,
        TerminalVisibility,
    };

    struct ResizeProbeMaster {
        calls: Arc<AtomicUsize>,
    }

    impl portable_pty::MasterPty for ResizeProbeMaster {
        fn resize(&self, _size: portable_pty::PtySize) -> Result<(), Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn get_size(&self) -> Result<portable_pty::PtySize, Error> {
            Ok(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
        }

        fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>, Error> {
            Ok(Box::new(std::io::empty()))
        }

        fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, Error> {
            Ok(Box::new(std::io::sink()))
        }

        #[cfg(unix)]
        fn process_group_leader(&self) -> Option<i32> {
            None
        }

        #[cfg(unix)]
        fn as_raw_fd(&self) -> Option<portable_pty::unix::RawFd> {
            None
        }

        #[cfg(unix)]
        fn tty_name(&self) -> Option<std::path::PathBuf> {
            None
        }
    }

    fn geometry(cols: u16, rows: u16) -> TerminalGeometry {
        TerminalGeometry { cols, rows }
    }

    #[tokio::test]
    async fn terminal_session_native_adapter_keeps_resize_and_input_behind_owner_lease() {
        let broker = TerminalSessionBroker::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let master: SharedPtyMaster = Arc::new(Mutex::new(Box::new(ResizeProbeMaster {
            calls: calls.clone(),
        })));
        let (input_tx, mut input_rx) = mpsc::channel(8);
        let generation = broker
            .start_or_replace_runtime(
                "native-session",
                native_terminal_runtime(input_tx, master),
                geometry(80, 24),
            )
            .await
            .expect("native runtime");

        for presentation_id in ["owner", "mirror"] {
            broker
                .register_presentation(
                    TerminalPresentationRegistration {
                        presentation_id: presentation_id.to_string(),
                        session_id: "native-session".to_string(),
                        client_kind: TerminalClientKind::Desktop,
                        desired_geometry: Some(geometry(100, 30)),
                        visibility: TerminalVisibility::Visible,
                        render_state: TerminalRenderState::Mounted,
                        requested_interaction: TerminalRequestedInteraction::Interactive,
                        observed_lease_epoch: 0,
                    },
                    TerminalClientIdentity::trusted_desktop(),
                )
                .await
                .expect("register presentation");
        }

        let begin = broker
            .begin_activation(TerminalActivationBeginRequest {
                session_id: "native-session".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                observed_lease_epoch: 0,
            })
            .await
            .expect("begin activation");
        let lease_epoch = begin.decision.lease_epoch;
        broker
            .ack_activation(TerminalActivationAckRequest {
                session_id: "native-session".to_string(),
                presentation_id: "owner".to_string(),
                runtime_generation: generation,
                lease_epoch,
                activation_id: begin.activation_id.expect("activation id"),
            })
            .await
            .expect("ack activation");

        let owner_lease = TerminalLeaseIdentity {
            session_id: "native-session".to_string(),
            presentation_id: "owner".to_string(),
            runtime_generation: generation,
            lease_epoch,
        };
        let mirror_lease = TerminalLeaseIdentity {
            presentation_id: "mirror".to_string(),
            ..owner_lease.clone()
        };
        let rejected = broker
            .resize(TerminalGeometryRequest {
                lease: mirror_lease,
                geometry_sequence: 1,
                geometry: geometry(120, 40),
            })
            .await
            .expect("structured mirror rejection");
        assert_eq!(
            rejected.decision.status,
            wardian_core::models::TerminalLeaseDecisionStatus::Rejected
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "only activation geometry reached native resize"
        );

        broker
            .send_input(wardian_core::models::TerminalInputRequest {
                lease: owner_lease,
                bytes: b"owned".to_vec(),
            })
            .await
            .expect("owner input");
        assert_eq!(input_rx.recv().await.as_deref(), Some(b"owned".as_slice()));
    }

    #[tokio::test]
    async fn terminal_session_reader_generation_cannot_cross_runtime_replacement() {
        let broker = Arc::new(TerminalSessionBroker::default());
        let (first_tx, _first_rx) = mpsc::channel(1);
        let first_master: SharedPtyMaster = Arc::new(Mutex::new(Box::new(ResizeProbeMaster {
            calls: Arc::new(AtomicUsize::new(0)),
        })));
        let first_generation = broker
            .start_or_replace_runtime(
                "reader-session",
                native_terminal_runtime(first_tx, first_master),
                geometry(80, 24),
            )
            .await
            .expect("first runtime");
        let (second_tx, _second_rx) = mpsc::channel(1);
        let second_master: SharedPtyMaster = Arc::new(Mutex::new(Box::new(ResizeProbeMaster {
            calls: Arc::new(AtomicUsize::new(0)),
        })));
        let second_generation = broker
            .start_or_replace_runtime(
                "reader-session",
                native_terminal_runtime(second_tx, second_master),
                geometry(100, 30),
            )
            .await
            .expect("replacement runtime");

        let stale = std::thread::spawn({
            let broker = broker.clone();
            move || forward_terminal_output(&broker, "reader-session", first_generation, b"stale")
        })
        .join()
        .expect("stale reader thread");
        assert!(matches!(
            stale,
            Err(crate::state::terminal_session::TerminalBrokerError::StaleRuntimeGeneration {
                expected,
                received,
            }) if expected == second_generation && received == first_generation
        ));
    }

    #[test]
    fn terminal_session_native_resize_gate_serializes_cross_session_calls() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let overlap = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let make_master = || -> SharedPtyMaster {
            Arc::new(Mutex::new(Box::new(ConcurrentResizeProbeMaster {
                in_flight: in_flight.clone(),
                overlap: overlap.clone(),
            })))
        };
        let first = make_master();
        let second = make_master();
        let first_resize =
            std::thread::spawn(move || resize_native_master(first, geometry(120, 40)));
        let second_resize =
            std::thread::spawn(move || resize_native_master(second, geometry(121, 41)));
        first_resize
            .join()
            .expect("first thread")
            .expect("first resize");
        second_resize
            .join()
            .expect("second thread")
            .expect("second resize");
        assert!(!overlap.load(Ordering::SeqCst));
    }

    struct ConcurrentResizeProbeMaster {
        in_flight: Arc<AtomicUsize>,
        overlap: Arc<std::sync::atomic::AtomicBool>,
    }

    impl portable_pty::MasterPty for ConcurrentResizeProbeMaster {
        fn resize(&self, _size: portable_pty::PtySize) -> Result<(), Error> {
            if self.in_flight.fetch_add(1, Ordering::SeqCst) > 0 {
                self.overlap.store(true, Ordering::SeqCst);
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }

        fn get_size(&self) -> Result<portable_pty::PtySize, Error> {
            Ok(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
        }

        fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>, Error> {
            Ok(Box::new(std::io::empty()))
        }

        fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, Error> {
            Ok(Box::new(std::io::sink()))
        }

        #[cfg(unix)]
        fn process_group_leader(&self) -> Option<i32> {
            None
        }

        #[cfg(unix)]
        fn as_raw_fd(&self) -> Option<portable_pty::unix::RawFd> {
            None
        }

        #[cfg(unix)]
        fn tty_name(&self) -> Option<std::path::PathBuf> {
            None
        }
    }
}
