//! Each waiter owns its exact turn's completion channel. Coalesced activity
//! notifications and later turns cannot overwrite that channel's evidence.
use super::*;
use std::collections::VecDeque;

type Outcome = Option<Result<TurnOutcome, CodexSharedError>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TurnOutcome {
    pub(super) status: String,
    pub(super) answer: String,
    pub(super) task_final_evidence: TaskFinalEvidence,
}

#[derive(Debug, Default)]
pub(super) struct TurnCompletions {
    turns: HashMap<String, watch::Sender<Outcome>>,
    recent: VecDeque<String>,
}

impl TurnCompletions {
    pub(super) fn contains(&self, id: &str) -> bool {
        self.turns.contains_key(id)
    }

    pub(super) fn start(&mut self, id: &str) {
        self.turns
            .entry(id.to_owned())
            .or_insert_with(|| watch::channel(None).0);
    }

    /// Records a turn's terminal outcome once. Returns `false` when the turn
    /// already had one, so callers never report a completion twice.
    pub(super) fn finish(
        &mut self,
        id: &str,
        status: &str,
        answer: &str,
        task_final_evidence: TaskFinalEvidence,
    ) -> bool {
        self.start(id);
        let slot = &self.turns[id];
        if slot.borrow().is_some() {
            return false;
        }
        slot.send_replace(Some(Ok(TurnOutcome {
            status: status.to_owned(),
            answer: answer.to_owned(),
            task_final_evidence,
        })));
        self.recent.push_back(id.to_owned());
        // Keep recent late subscribers, plus every currently registered waiter.
        // Dropping an unobserved old result causes an explicit error, not a wait
        // for a completion that already happened. Existing receivers pin data.
        for _ in 0..self.recent.len() {
            if self.recent.len() <= 64 {
                break;
            }
            let oldest = self.recent.pop_front().unwrap();
            if self.turns[&oldest].receiver_count() == 0 {
                self.turns.remove(&oldest);
            } else {
                self.recent.push_back(oldest);
            }
        }
        true
    }

    pub(super) fn confirm_task_final(&mut self, id: &str, request_id: &str) -> bool {
        let Some(slot) = self.turns.get(id) else {
            return false;
        };
        if !slot
            .borrow()
            .as_ref()
            .is_some_and(|outcome| outcome.is_ok())
        {
            return false;
        }
        let mut confirmed = false;
        slot.send_modify(|outcome| {
            if let Some(Ok(outcome)) = outcome {
                confirmed = outcome.task_final_evidence.confirm(request_id);
            }
        });
        confirmed
    }

    pub(super) fn subscribe(&self, id: &str) -> Result<watch::Receiver<Outcome>, CodexSharedError> {
        self.turns
            .get(id)
            .map(watch::Sender::subscribe)
            .ok_or_else(|| {
                CodexSharedError::uncertain(
                    "exact turn is unknown or its unclaimed completion is no longer retained",
                )
            })
    }

    pub(super) fn close(&self) {
        for slot in self.turns.values() {
            if slot.borrow().is_none() {
                slot.send_replace(Some(Err(CodexSharedError::uncertain(
                    "connection ended before exact turn completion",
                ))));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn coalesced_completions_preserve_each_registered_waiter() {
        let mut turns = TurnCompletions::default();
        turns.start("first");
        let mut first = turns.subscribe("first").unwrap();
        turns.finish(
            "first",
            "completed",
            "first answer",
            TaskFinalEvidence {
                request_id: Some("first-request".into()),
                eligible: false,
            },
        );
        // Exceed even the late-subscriber cache while the first waiter is paused.
        for i in 0..130 {
            turns.finish(
                &format!("later-{i}"),
                "completed",
                "different answer",
                TaskFinalEvidence::default(),
            );
        }
        assert!(turns.confirm_task_final("first", "first-request"));
        first.changed().await.unwrap();
        let first_outcome = first.borrow_and_update().clone().unwrap().unwrap();
        assert_eq!(first_outcome.status, "completed");
        assert_eq!(first_outcome.answer, "first answer");
        assert!(first_outcome.task_final_evidence.is_eligible());
        assert!(turns.subscribe("later-0").is_err());
        assert!(turns.turns.len() <= 65);
    }

    #[test]
    fn closure_fails_pending_but_keeps_completed_evidence() {
        let mut turns = TurnCompletions::default();
        turns.start("pending");
        turns.finish(
            "done",
            "interrupted",
            "partial",
            TaskFinalEvidence::default(),
        );
        turns.close();
        assert!(turns
            .subscribe("pending")
            .unwrap()
            .borrow()
            .as_ref()
            .unwrap()
            .is_err());
        assert_eq!(
            turns
                .subscribe("done")
                .unwrap()
                .borrow()
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .status,
            "interrupted"
        );
    }
}
