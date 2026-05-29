use std::path::Path;
use wardian_core::engine::{
    NotifyRequest, ScriptRequest, ShellRequest, StateRequest, StepError, StepOutput,
};

pub async fn run_shell(_ws: &Path, _req: &ShellRequest) -> Result<StepOutput, StepError> {
    Ok(StepOutput(serde_json::json!({})))
}

pub async fn run_script(_ws: &Path, _req: &ScriptRequest) -> Result<StepOutput, StepError> {
    Ok(StepOutput(serde_json::json!({})))
}

pub fn notify(_req: &NotifyRequest) -> Result<(), StepError> {
    Ok(())
}

pub fn state_op(_req: &StateRequest) -> Result<StepOutput, StepError> {
    Ok(StepOutput(serde_json::json!({})))
}
