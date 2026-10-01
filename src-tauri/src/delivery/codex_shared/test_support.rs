//! Synthetic native owner observations for no-child lifecycle tests.
use super::{CodexTurnActivity, Observation};

pub(crate) fn observation_with_activity(activity: CodexTurnActivity) -> Observation {
    let mut observation = Observation::default();
    match activity {
        CodexTurnActivity::Pending => {}
        CodexTurnActivity::Processing(id) => observation.active_turn = Some(id),
        CodexTurnActivity::ProcessingWithoutTurn => {
            observation.thread_runtime_status = Some(super::ThreadRuntimeStatus::Active)
        }
        CodexTurnActivity::Idle(id) => observation.completed_turn = Some((id, "completed".into())),
        CodexTurnActivity::IdleWithoutTurn => {
            observation.thread_runtime_status = Some(super::ThreadRuntimeStatus::Idle)
        }
        CodexTurnActivity::ActionRequiredWithoutTurn => {
            observation.thread_runtime_status = Some(super::ThreadRuntimeStatus::ActionRequired)
        }
        CodexTurnActivity::Closed => observation.closed = true,
        CodexTurnActivity::Stopped => observation.stopped = true,
    }
    observation
}
