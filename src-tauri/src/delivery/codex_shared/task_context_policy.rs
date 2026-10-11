//! Qualification of the supported managed launch, not per-call provider policy.
use super::*;
use crate::utils::codex_messaging::recovery_registration;
use std::path::Path;

#[derive(Debug)]
pub(super) struct TaskContextPolicy {
    pub thread_id: String,
    pub configuration_version: String,
}

impl CodexSharedClient {
    /// Capture applied managed policy for a future owner. Failure disables only
    /// recovery. No config, hook trust or provider approval is written here.
    pub(super) async fn qualify_task_recovery(&self, codex_home: &Path, thread: &Value) {
        self.observation
            .send_modify(|state| state.task_context_policy = None);
        let revision = self.observation.borrow().task_context_policy_revision;
        if self.observation.borrow().provider_version.as_deref() != Some("0.160.0") {
            return;
        }
        let Ok(record) = recovery_registration(codex_home, &self.agent_id) else {
            return;
        };
        let (Some(thread_id), Some(cwd)) =
            (thread["thread"]["id"].as_str(), thread["cwd"].as_str())
        else {
            return;
        };
        let Ok(config) = self
            .request_with_timeout(
                "config/read",
                json!({"cwd":cwd,"includeLayers":true}),
                Duration::from_secs(5),
            )
            .await
        else {
            return;
        };
        let Some(version) =
            managed_output_policy(&config, codex_home, &record.executable, &self.agent_id)
        else {
            return;
        };
        let Ok(hooks) = self
            .request_with_timeout("hooks/list", json!({"cwds":[cwd]}), Duration::from_secs(5))
            .await
        else {
            return;
        };
        let Some(entries) = hooks["data"].as_array() else {
            return;
        };
        let valid = entries
            .iter()
            .filter(|entry| entry["cwd"] == cwd)
            .any(|entry| {
                entry["errors"].as_array().is_some_and(Vec::is_empty)
                    && entry["hooks"].as_array().is_some_and(|hooks| {
                        hooks
                            .iter()
                            .filter(|hook| {
                                hook["key"] == record.hook_key
                                    && hook["currentHash"] == record.hook_hash
                                    && hook["command"] == record.hook_command
                                    && hook["eventName"] == "sessionStart"
                                    && hook["matcher"] == "compact"
                                    && hook["handlerType"] == "command"
                                    && hook["async"] == false
                                    && hook["enabled"] == true
                                    && hook["trustStatus"] == "trusted"
                            })
                            .count()
                            == 1
                    })
            });
        if valid {
            self.observation.send_modify(|state| {
                if !state.closed && !state.stopped && state.task_context_policy_revision == revision
                {
                    state.task_context_policy = Some(TaskContextPolicy {
                        thread_id: thread_id.into(),
                        configuration_version: version,
                    });
                }
            });
        }
    }
}

fn managed_output_policy(
    response: &Value,
    codex_home: &Path,
    executable: &str,
    agent_id: &str,
) -> Option<String> {
    let server = &response["config"]["mcp_servers"]["wardian"];
    if server["command"] != executable
        || server["args"] != json!(["mcp", "serve"])
        || server["env"]["WARDIAN_SESSION_ID"] != agent_id
        || server["enabled"] == false
        || response["config"]["features"]["codex_hooks"] == false
        || server["tools"]["read_task_context"]["output_token_limit"].as_u64()?
            < wardian_core::agent_messaging::TASK_CONTEXT_OUTPUT_TOKENS as u64
    {
        return None;
    }
    let origin =
        &response["origins"]["mcp_servers.wardian.tools.read_task_context.output_token_limit"];
    if origin["name"]["type"] != "user"
        || !origin["name"]["profile"].is_null()
        || Path::new(origin["name"]["file"].as_str()?) != codex_home.join("config.toml")
    {
        return None;
    }
    let version = origin["version"].as_str()?.to_owned();
    if version.is_empty()
        || !response["layers"].as_array()?.iter().any(|layer| {
            layer["name"] == origin["name"]
                && layer["version"] == version
                && layer["disabledReason"].is_null()
        })
    {
        return None;
    }
    Some(version)
}

#[cfg(test)]
mod tests;
