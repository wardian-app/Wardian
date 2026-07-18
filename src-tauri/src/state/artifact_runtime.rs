use crate::artifact_service::ArtifactPresentationAckV1;
use std::{collections::HashMap, time::Duration};
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactAckWaitError {
    Timeout,
    SenderDropped,
}

#[derive(Debug, Default)]
pub struct ArtifactRuntime {
    pending: Mutex<HashMap<String, oneshot::Sender<ArtifactPresentationAckV1>>>,
}

impl ArtifactRuntime {
    pub async fn register(
        &self,
        presentation_id: &str,
    ) -> Result<oneshot::Receiver<ArtifactPresentationAckV1>, String> {
        if presentation_id.trim().is_empty() {
            return Err("presentation id must not be empty".to_string());
        }
        let (sender, receiver) = oneshot::channel();
        let mut pending = self.pending.lock().await;
        if pending
            .insert(presentation_id.to_string(), sender)
            .is_some()
        {
            return Err("presentation id is already awaiting acknowledgement".to_string());
        }
        Ok(receiver)
    }

    pub async fn acknowledge(&self, ack: ArtifactPresentationAckV1) -> bool {
        let sender = self.pending.lock().await.remove(&ack.presentation_id);
        sender.is_some_and(|sender| sender.send(ack).is_ok())
    }

    pub async fn wait(
        &self,
        presentation_id: &str,
        receiver: oneshot::Receiver<ArtifactPresentationAckV1>,
        timeout: Duration,
    ) -> Result<ArtifactPresentationAckV1, ArtifactAckWaitError> {
        let received = tokio::time::timeout(timeout, receiver).await;
        self.pending.lock().await.remove(presentation_id);
        match received {
            Ok(Ok(ack)) => Ok(ack),
            Ok(Err(_)) => Err(ArtifactAckWaitError::SenderDropped),
            Err(_) => Err(ArtifactAckWaitError::Timeout),
        }
    }

    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acknowledgement_is_exactly_scoped_and_consumed() {
        let runtime = ArtifactRuntime::default();
        let receiver = runtime.register("presentation-1").await.expect("register");
        assert!(
            !runtime
                .acknowledge(ArtifactPresentationAckV1 {
                    presentation_id: "other".into(),
                    routed: true,
                    error: None,
                })
                .await
        );
        assert!(
            runtime
                .acknowledge(ArtifactPresentationAckV1 {
                    presentation_id: "presentation-1".into(),
                    routed: true,
                    error: None,
                })
                .await
        );
        let ack = runtime
            .wait("presentation-1", receiver, Duration::from_secs(1))
            .await
            .expect("ack");
        assert!(ack.routed);
        assert_eq!(runtime.pending_count().await, 0);
    }

    #[tokio::test]
    async fn timeout_removes_pending_sender() {
        let runtime = ArtifactRuntime::default();
        let receiver = runtime.register("presentation-1").await.expect("register");
        assert_eq!(
            runtime
                .wait("presentation-1", receiver, Duration::from_millis(1))
                .await,
            Err(ArtifactAckWaitError::Timeout)
        );
        assert_eq!(runtime.pending_count().await, 0);
    }
}
