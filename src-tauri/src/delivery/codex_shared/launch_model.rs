//! Resolve a launch-only model so an explicit effort survives stock TUI cold resume.

use std::collections::HashSet;
use std::future::Future;
use std::path::Path;

use serde_json::{json, Value};

use super::{CodexSharedClient, CodexSharedError, STARTUP_TIMEOUT};

const MODEL_PAGE_SIZE: usize = 100;
const MAX_MODEL_PAGES: usize = 32;

/// Resolve before loading a thread, using only the owned daemon's read APIs.
/// The caller must check that the daemon remains empty before its first load.
/// This value is for launch arguments, not persisted agent configuration.
pub(super) async fn resolve_launch_model(
    client: &CodexSharedClient,
    configured_model: Option<&str>,
    configured_effort: Option<&str>,
    resume_id: Option<&str>,
    cwd: &Path,
) -> Result<Option<String>, CodexSharedError> {
    // One deadline covers the entire lookup, including catalog pagination.
    tokio::time::timeout(
        STARTUP_TIMEOUT,
        resolve_with_request(
            configured_model,
            configured_effort,
            resume_id,
            cwd,
            |method, params| client.request_with_timeout(method, params, STARTUP_TIMEOUT),
        ),
    )
    .await
    .map_err(|_| CodexSharedError::unsupported("Codex launch model lookup timed out"))?
}

async fn resolve_with_request<F, Fut>(
    configured_model: Option<&str>,
    configured_effort: Option<&str>,
    resume_id: Option<&str>,
    cwd: &Path,
    mut request: F,
) -> Result<Option<String>, CodexSharedError>
where
    F: FnMut(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Value, CodexSharedError>>,
{
    if let Some(model) = configured_model {
        return Ok(Some(model.to_owned()));
    }
    if configured_effort.is_none() {
        return Ok(None);
    }

    if let Some(id) = resume_id {
        if id.trim().is_empty() {
            return Err(invalid("resume thread ID"));
        }
        let response = request("thread/read", json!({"threadId":id,"includeTurns":false})).await?;
        let thread = response
            .get("thread")
            .filter(|value| value.is_object())
            .ok_or_else(|| invalid("thread/read thread"))?;
        if thread.get("id").and_then(Value::as_str) != Some(id) {
            return Err(invalid("thread/read identity"));
        }
        if let Some(model) = optional_model(thread.get("model"), "thread/read model")? {
            return Ok(Some(model));
        }
    }

    let cwd = cwd
        .to_str()
        .ok_or_else(|| invalid("config/read working directory"))?;
    let response = request("config/read", json!({"cwd":cwd,"includeLayers":false})).await?;
    let config = response
        .get("config")
        .filter(|value| value.is_object())
        .ok_or_else(|| invalid("config/read config"))?;
    if let Some(model) = optional_model(config.get("model"), "config/read model")? {
        return Ok(Some(model));
    }

    // Stock alpha6 TUI bootstrap prefers the catalog default, then the first
    // entry's `model` (not its display `id`), and includes hidden models.
    // Retain only those candidates while checking every bounded page.
    let mut first = None;
    let mut default = None;
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    for _ in 0..MAX_MODEL_PAGES {
        let response = request(
            "model/list",
            json!({"cursor":cursor,"limit":MODEL_PAGE_SIZE,"includeHidden":true}),
        )
        .await?;
        let models = response
            .get("data")
            .and_then(Value::as_array)
            .filter(|models| !models.is_empty() && models.len() <= MODEL_PAGE_SIZE)
            .ok_or_else(|| invalid("model/list page"))?;
        for entry in models {
            let model = optional_model(entry.get("model"), "model/list model")?
                .ok_or_else(|| invalid("model/list model"))?;
            let is_default = entry
                .get("isDefault")
                .and_then(Value::as_bool)
                .ok_or_else(|| invalid("model/list isDefault"))?;
            if first.is_none() {
                first = Some(model.clone());
            }
            if is_default && default.replace(model).is_some() {
                return Err(invalid("model/list multiple defaults"));
            }
        }
        match response.get("nextCursor") {
            None | Some(Value::Null) => return Ok(default.or(first)),
            Some(Value::String(next)) if !next.trim().is_empty() => {
                if !seen_cursors.insert(next.clone()) {
                    return Err(invalid("model/list repeated cursor"));
                }
                cursor = Some(next.clone());
            }
            _ => return Err(invalid("model/list nextCursor")),
        }
    }
    Err(invalid("model/list page limit"))
}

fn optional_model(value: Option<&Value>, field: &str) -> Result<Option<String>, CodexSharedError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(model)) if !model.trim().is_empty() => Ok(Some(model.clone())),
        _ => Err(invalid(field)),
    }
}

fn invalid(field: &str) -> CodexSharedError {
    CodexSharedError::unsupported(format!("invalid {field} during Codex launch model lookup"))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::future::ready;

    use super::*;

    type Reply = Result<Value, CodexSharedError>;

    struct Script(VecDeque<(&'static str, Value, Reply)>);

    impl Script {
        async fn resolve(
            mut self,
            model: Option<&str>,
            effort: Option<&str>,
            resume_id: Option<&str>,
        ) -> Result<Option<String>, CodexSharedError> {
            let result = resolve_with_request(
                model,
                effort,
                resume_id,
                Path::new("workspace"),
                |method, params| {
                    let (expected_method, expected_params, response) =
                        self.0.pop_front().expect("unexpected RPC");
                    assert_eq!(method, expected_method);
                    assert_eq!(params, expected_params);
                    ready(response)
                },
            )
            .await;
            assert!(self.0.is_empty(), "unconsumed expected RPCs");
            result
        }
    }

    fn config_reply(response: Value) -> (&'static str, Value, Reply) {
        (
            "config/read",
            json!({"cwd":"workspace","includeLayers":false}),
            Ok(response),
        )
    }

    fn catalog_reply(cursor: Option<&str>, response: Value) -> (&'static str, Value, Reply) {
        (
            "model/list",
            json!({"cursor":cursor,"limit":100,"includeHidden":true}),
            Ok(response),
        )
    }

    #[tokio::test]
    async fn explicit_model_and_unspecified_policy_do_not_request() {
        for effort in [None, Some("low")] {
            assert_eq!(
                Script(VecDeque::new())
                    .resolve(Some(" exact-model "), effort, Some("historic"))
                    .await
                    .unwrap(),
                Some(" exact-model ".into())
            );
        }
        assert_eq!(
            Script(VecDeque::new())
                .resolve(None, None, Some("historic"))
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn historic_model_precedes_current_config() {
        let script = Script(VecDeque::from([(
            "thread/read",
            json!({"threadId":"historic","includeTurns":false}),
            Ok(json!({"thread":{"id":"historic","model":"saved-model"}})),
        )]));
        assert_eq!(
            script
                .resolve(None, Some("low"), Some("historic"))
                .await
                .unwrap(),
            Some("saved-model".into())
        );
    }

    #[tokio::test]
    async fn metadata_only_seed_uses_effective_config() {
        for thread in [json!({"id":"seed","model":null}), json!({"id":"seed"})] {
            let script = Script(VecDeque::from([
                (
                    "thread/read",
                    json!({"threadId":"seed","includeTurns":false}),
                    Ok(json!({"thread":thread})),
                ),
                config_reply(json!({"config":{"model":"configured-default"}})),
            ]));
            assert_eq!(
                script
                    .resolve(None, Some("low"), Some("seed"))
                    .await
                    .unwrap(),
                Some("configured-default".into())
            );
        }
    }

    #[tokio::test]
    async fn invalid_historic_lookup_never_falls_back() {
        for response in [
            Err(CodexSharedError::unsupported("thread missing")),
            Ok(json!({})),
            Ok(json!({"thread":{"id":"other","model":"wrong"}})),
            Ok(json!({"thread":{"id":"historic","model":42}})),
            Ok(json!({"thread":{"id":"historic","model":" "}})),
        ] {
            let expected_error = response.as_ref().err().cloned();
            let error = Script(VecDeque::from([(
                "thread/read",
                json!({"threadId":"historic","includeTurns":false}),
                response,
            )]))
            .resolve(None, Some("low"), Some("historic"))
            .await
            .unwrap_err();
            if let Some(expected) = expected_error {
                assert_eq!(error, expected);
            }
        }
        assert!(Script(VecDeque::new())
            .resolve(None, Some("low"), Some(""))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn catalog_prefers_later_default_and_uses_model_not_id() {
        let script = Script(VecDeque::from([
            config_reply(json!({"config":{"model":null}})),
            catalog_reply(
                None,
                json!({"data":[{"id":"display-a","model":"first","isDefault":false}],"nextCursor":"page2"}),
            ),
            catalog_reply(
                Some("page2"),
                json!({"data":[{"id":"display-b","model":"default","isDefault":true}],"nextCursor":null}),
            ),
        ]));
        assert_eq!(
            script.resolve(None, Some("low"), None).await.unwrap(),
            Some("default".into())
        );
    }

    #[tokio::test]
    async fn catalog_without_default_preserves_first_across_pages() {
        let script = Script(VecDeque::from([
            config_reply(json!({"config":{}})),
            catalog_reply(
                None,
                json!({"data":[{"model":"first","isDefault":false}],"nextCursor":"page2"}),
            ),
            catalog_reply(
                Some("page2"),
                json!({"data":[{"model":"second","isDefault":false}],"nextCursor":null}),
            ),
        ]));
        assert_eq!(
            script.resolve(None, Some("low"), None).await.unwrap(),
            Some("first".into())
        );
    }

    #[tokio::test]
    async fn malformed_config_does_not_fall_back_to_catalog() {
        for response in [
            json!({}),
            json!({"config":null}),
            json!({"config":{"model":7}}),
            json!({"config":{"model":""}}),
        ] {
            assert!(Script(VecDeque::from([config_reply(response)]))
                .resolve(None, Some("low"), None)
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn config_read_preserves_supplied_working_directory() {
        let cwd = std::env::temp_dir().join("launch-model-é中");
        let mut calls = 0;
        let result = resolve_with_request(None, Some("low"), None, &cwd, |method, params| {
            calls += 1;
            assert_eq!(method, "config/read");
            assert_eq!(params, json!({"cwd":cwd,"includeLayers":false}));
            ready(Ok(json!({"config":{"model":"effective-model"}})))
        })
        .await
        .unwrap();
        assert_eq!(result, Some("effective-model".into()));
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn config_and_catalog_rpc_errors_are_preserved_without_retry() {
        let error = CodexSharedError::unsupported("owned daemon read failed");
        for method in ["config/read", "model/list"] {
            let mut replies = VecDeque::new();
            let params = if method == "model/list" {
                replies.push_back(config_reply(json!({"config":{}})));
                json!({"cursor":null,"limit":100,"includeHidden":true})
            } else {
                json!({"cwd":"workspace","includeLayers":false})
            };
            replies.push_back((method, params, Err(error.clone())));
            assert_eq!(
                Script(replies)
                    .resolve(None, Some("low"), None)
                    .await
                    .unwrap_err(),
                error
            );
        }
    }

    #[tokio::test]
    async fn malformed_or_empty_catalog_is_rejected() {
        for response in [
            json!({}),
            json!({"data":[]}),
            json!({"data":[null]}),
            json!({"data":[{"model":null,"isDefault":true}]}),
            json!({"data":[{"model":" ","isDefault":true}]}),
            json!({"data":[{"model":"a","isDefault":"true"}]}),
            json!({"data":[{"model":"a"}]}),
            json!({"data":[{"model":"a","isDefault":true}],"nextCursor":7}),
            json!({"data":[{"model":"a","isDefault":true}],"nextCursor":""}),
            json!({"data":[{"model":"a","isDefault":true},{"model":"b","isDefault":true}]}),
            json!({"data":vec![json!({"model":"a","isDefault":false}); MODEL_PAGE_SIZE + 1]}),
        ] {
            assert!(Script(VecDeque::from([
                config_reply(json!({"config":{}})),
                catalog_reply(None, response),
            ]))
            .resolve(None, Some("low"), None)
            .await
            .is_err());
        }
    }

    #[tokio::test]
    async fn catalog_repeated_cursor_is_rejected_even_after_default() {
        let script = Script(VecDeque::from([
            config_reply(json!({"config":{}})),
            catalog_reply(
                None,
                json!({"data":[{"model":"a","isDefault":true}],"nextCursor":"repeat"}),
            ),
            catalog_reply(
                Some("repeat"),
                json!({"data":[{"model":"b","isDefault":false}],"nextCursor":"repeat"}),
            ),
        ]));
        assert!(script
            .resolve(None, Some("low"), None)
            .await
            .unwrap_err()
            .message
            .contains("repeated cursor"));
    }

    #[tokio::test]
    async fn catalog_unique_cursor_chain_is_bounded() {
        let mut replies = VecDeque::from([config_reply(json!({"config":{}}))]);
        for index in 0..MAX_MODEL_PAGES {
            let cursor = (index > 0).then(|| index.to_string());
            replies.push_back(catalog_reply(
                cursor.as_deref(),
                json!({
                    "data":[{"model":"a","isDefault":false}],"nextCursor":(index + 1).to_string()
                }),
            ));
        }
        assert!(Script(replies)
            .resolve(None, Some("low"), None)
            .await
            .unwrap_err()
            .message
            .contains("page limit"));
    }
}
