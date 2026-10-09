use super::*;

fn snapshot() -> Value {
    let name = json!({"type":"user","file":"/managed/config.toml","profile":null});
    json!({"config":{"mcp_servers":{"wardian":{"command":"/managed/wardian-cli","args":["mcp","serve"],"env":{"WARDIAN_SESSION_ID":"agent"},"tools":{"read_task_context":{"output_token_limit":2048}}}}},"origins":{"mcp_servers.wardian.tools.read_task_context.output_token_limit":{"name":name,"version":"sha256:owned-layer"}},"layers":[{"name":name,"version":"sha256:owned-layer"}]})
}

#[test]
fn only_explicit_owned_applied_budget_qualifies_and_model_effort_do_not_change_it() {
    let mut config = snapshot();
    let read = |config: &Value| {
        managed_output_policy(
            config,
            Path::new("/managed"),
            "/managed/wardian-cli",
            "agent",
        )
    };
    assert!(read(&config).is_some());
    config["config"]["model"] = json!("another-model");
    config["config"]["model_reasoning_effort"] = json!("high");
    assert!(read(&config).is_some());
    for limit in [json!(2047), json!(0), json!(null), json!("2048")] {
        config["config"]["mcp_servers"]["wardian"]["tools"]["read_task_context"]
            ["output_token_limit"] = limit;
        assert!(read(&config).is_none());
    }
    for change in [
        "profile",
        "project",
        "missing_layer",
        "disabled",
        "wrong_version",
    ] {
        let mut config = snapshot();
        match change {
            "profile" => {
                config["origins"]
                    ["mcp_servers.wardian.tools.read_task_context.output_token_limit"]["name"]
                    ["profile"] = json!("custom")
            }
            "project" => {
                config["origins"]
                    ["mcp_servers.wardian.tools.read_task_context.output_token_limit"]["name"]
                    ["type"] = json!("project")
            }
            "missing_layer" => config["layers"] = json!([]),
            "disabled" => config["layers"][0]["disabledReason"] = json!("disabled"),
            _ => config["layers"][0]["version"] = json!("another-version"),
        }
        assert!(read(&config).is_none(), "{change}");
    }
}
