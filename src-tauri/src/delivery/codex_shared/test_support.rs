//! Synthetic native owner observations for no-child lifecycle tests.
use super::{CodexTurnActivity, Observation};

pub(crate) fn observation_with_activity(activity: CodexTurnActivity) -> Observation {
    let mut observation = Observation::default();
    match activity {
        CodexTurnActivity::Pending => {}
        CodexTurnActivity::Processing(id) => observation.active_turn = Some(id),
        CodexTurnActivity::Idle(id) => observation.completed_turn = Some((id, "completed".into())),
        CodexTurnActivity::Closed => observation.closed = true,
        CodexTurnActivity::Stopped => observation.stopped = true,
    }
    observation
}
