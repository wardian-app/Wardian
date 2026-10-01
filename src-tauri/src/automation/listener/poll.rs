//! Web-poll listener execution: fetch, fingerprint, and fire on change.
//!
//! This is the only listener variant that survives application downtime. The
//! fingerprint is durable, so a change that happened while Wardian was closed
//! is still visible on the next poll, where a missed filesystem event or an
//! undelivered webhook is simply gone.

use super::launch::{self, ListenerFire};
use serde_json::{Map, Value};
use std::time::Duration;
use tauri::AppHandle;
use wardian_core::listeners::{
    self, poll as poll_rules, secrets, AutomationListener, ListenerTrigger, PollMethod,
    WebPollTrigger,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Body text included in the run payload, independent of the larger cap used
/// for fingerprinting. A megabyte of untrusted HTML in an agent prompt is not
/// useful to anyone.
const MAX_INLINE_BODY_BYTES: usize = 64 * 1024;

fn user_agent() -> String {
    format!(
        "Wardian/{} (automation listener)",
        env!("CARGO_PKG_VERSION")
    )
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Read at most `limit` bytes of the response body.
///
/// Streamed rather than buffered whole so an enormous or endless response
/// cannot exhaust memory before the cap is applied.
async fn read_capped_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("could not read response body: {error}"))?
    {
        let remaining = limit.saturating_sub(body.len());
        if remaining == 0 {
            break;
        }
        let take = remaining.min(chunk.len());
        body.extend_from_slice(&chunk[..take]);
        if body.len() >= limit {
            break;
        }
    }
    Ok(body)
}

fn header_value(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Strip the `kind:` prefix a fingerprint carries, leaving the observed value
/// that an automation actually wants to read.
fn fingerprint_value(fingerprint: &str) -> Option<&str> {
    fingerprint
        .split_once(':')
        .filter(|(kind, _)| matches!(*kind, "json" | "regex"))
        .map(|(_, value)| value)
}

fn payload(
    listener: &AutomationListener,
    trigger: &WebPollTrigger,
    status: u16,
    fingerprint: &str,
    previous: Option<&str>,
    body: &[u8],
) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("listener_id".into(), Value::String(listener.id.clone()));
    map.insert("listener_name".into(), Value::String(listener.name.clone()));
    map.insert("trigger_type".into(), Value::String("web_poll".into()));
    map.insert("url".into(), Value::String(trigger.url.clone()));
    map.insert("status".into(), Value::from(status));
    map.insert("fingerprint".into(), Value::String(fingerprint.to_string()));
    map.insert(
        "previous_fingerprint".into(),
        previous
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );
    map.insert(
        "value".into(),
        fingerprint_value(fingerprint)
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );
    let inline = &body[..body.len().min(MAX_INLINE_BODY_BYTES)];
    map.insert(
        "body".into(),
        String::from_utf8(inline.to_vec())
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    map.insert(
        "body_truncated".into(),
        Value::Bool(body.len() > MAX_INLINE_BODY_BYTES),
    );
    map.insert(
        "observed_at".into(),
        Value::String(chrono::Utc::now().to_rfc3339()),
    );
    map
}

async fn observe(
    listener: &AutomationListener,
    trigger: &WebPollTrigger,
) -> Result<(u16, String, Vec<u8>), String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(user_agent())
        .build()
        .map_err(|error| format!("could not build http client: {error}"))?;

    let mut request = match trigger.method {
        PollMethod::Get => client.get(&trigger.url),
        PollMethod::Head => client.head(&trigger.url),
    };
    for (name, value) in &trigger.headers {
        request = request.header(name, value);
    }
    // Credential headers live outside the inspectable config and are attached
    // only here, at request time.
    if let Some(secret) = secrets::load_secret(&listener.id) {
        for (name, value) in &secret.headers {
            request = request.header(name, value);
        }
    }

    let response = request
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{} returned {status}", trigger.url));
    }
    let headers = response.headers().clone();
    let body = read_capped_body(response, trigger.max_body_bytes as usize).await?;
    let observed = poll_rules::fingerprint(
        trigger,
        &poll_rules::PollResponse {
            etag: header_value(&headers, "etag"),
            last_modified: header_value(&headers, "last-modified"),
            body: body.clone(),
        },
    )?;
    Ok((status.as_u16(), observed, body))
}

/// Poll every due listener once.
///
/// Called from the supervisor tick. Due-planning and pacing live in core, so
/// this function only performs effects.
pub async fn tick(app: &AppHandle) {
    let mut listeners = listeners::load_listeners();
    if listeners.is_empty() {
        return;
    }
    let due = poll_rules::plan_due(&mut listeners, now_ms());
    if due.is_empty() {
        return;
    }
    // Persist the recomputed pacing before any request, so a crash mid-poll
    // cannot make the listener hammer the endpoint on restart.
    for id in &due {
        if let Some(next) = listeners
            .iter()
            .find(|listener| &listener.id == id)
            .and_then(|listener| listener.runtime.next_poll_epoch_ms)
        {
            launch::write_runtime(id, move |runtime| runtime.next_poll_epoch_ms = Some(next));
        }
    }

    for id in due {
        let Some(listener) = listeners.iter().find(|listener| listener.id == id).cloned() else {
            continue;
        };
        let ListenerTrigger::WebPoll(trigger) = listener.trigger.clone() else {
            continue;
        };
        poll_once(app, listener, trigger).await;
    }
}

async fn poll_once(app: &AppHandle, listener: AutomationListener, trigger: WebPollTrigger) {
    let previous = listener.runtime.poll_fingerprint.clone();
    match observe(&listener, &trigger).await {
        Err(error) => {
            // A flaky endpoint is not a runaway listener: back off, stay
            // enabled, and record why so the failure is visible.
            let failures = listener.runtime.consecutive_failures.saturating_add(1);
            let next = poll_rules::next_poll_epoch_ms(&trigger, failures, now_ms());
            let reason = error.clone();
            launch::write_runtime(&listener.id, move |runtime| {
                runtime.consecutive_failures = failures;
                runtime.next_poll_epoch_ms = Some(next);
                runtime.last_rejection = Some(wardian_core::listeners::ListenerRejection {
                    reason,
                    at_epoch_ms: now_ms(),
                });
            });
            crate::utils::logging::log_debug(&format!(
                "[automation] listener {} poll failed ({failures} consecutive): {error}",
                listener.id
            ));
            launch::emit_listeners_updated(app);
        }
        Ok((status, observed, body)) => {
            let changed = poll_rules::decide(previous.as_deref(), &observed);
            let stored = observed.clone();
            let next = poll_rules::next_poll_epoch_ms(&trigger, 0, now_ms());
            launch::write_runtime(&listener.id, move |runtime| {
                runtime.consecutive_failures = 0;
                runtime.poll_fingerprint = Some(stored);
                runtime.next_poll_epoch_ms = Some(next);
            });
            if !changed {
                launch::emit_listeners_updated(app);
                return;
            }
            let fire = ListenerFire {
                // The fingerprint is the event identity, so "fire on change"
                // and "never run the same change twice" are one property.
                event_identity: observed.clone(),
                payload: payload(
                    &listener,
                    &trigger,
                    status,
                    &observed,
                    previous.as_deref(),
                    &body,
                ),
            };
            launch::fire(app.clone(), listener, fire).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use wardian_core::listeners::{FingerprintSource, DEFAULT_POLL_MAX_BODY_BYTES};

    fn trigger() -> WebPollTrigger {
        WebPollTrigger {
            url: "https://example.invalid/releases".into(),
            interval_seconds: 300,
            method: PollMethod::Get,
            headers: BTreeMap::new(),
            fingerprint: FingerprintSource::JsonPointer,
            json_pointer: Some("/0/tag_name".into()),
            regex: None,
            max_body_bytes: DEFAULT_POLL_MAX_BODY_BYTES,
        }
    }

    fn listener() -> AutomationListener {
        AutomationListener {
            id: "poll".into(),
            blueprint_id: "audit".into(),
            name: "Release watch".into(),
            enabled: true,
            trigger: ListenerTrigger::WebPoll(trigger()),
            provider: None,
            workspace: None,
            input: serde_json::json!({}),
            bindings: Default::default(),
            assignments: Default::default(),
            overlap: None,
            runtime: Default::default(),
        }
    }

    #[test]
    fn an_extracted_value_is_surfaced_separately_from_the_raw_fingerprint() {
        assert_eq!(fingerprint_value("json:\"v1.3.0\""), Some("\"v1.3.0\""));
        assert_eq!(fingerprint_value("regex:4.11.2"), Some("4.11.2"));
        assert_eq!(
            fingerprint_value("etag:W/\"abc\""),
            None,
            "an opaque validator is not a value an automation can use"
        );
        assert_eq!(fingerprint_value("body:deadbeef"), None);
    }

    #[test]
    fn the_payload_carries_both_fingerprints_so_a_run_can_see_what_moved() {
        let map = payload(
            &listener(),
            &trigger(),
            200,
            "json:\"v1.3.0\"",
            Some("json:\"v1.2.0\""),
            br#"[{"tag_name":"v1.3.0"}]"#,
        );
        assert_eq!(map["status"], serde_json::json!(200));
        assert_eq!(map["previous_fingerprint"], "json:\"v1.2.0\"");
        assert_eq!(map["value"], "\"v1.3.0\"");
        assert_eq!(map["body_truncated"], serde_json::json!(false));
        assert_eq!(map["trigger_type"], "web_poll");
    }

    #[test]
    fn a_large_body_is_truncated_in_the_payload_and_says_so() {
        let body = vec![b'x'; MAX_INLINE_BODY_BYTES * 2];
        let map = payload(&listener(), &trigger(), 200, "body:abc", None, &body);
        assert_eq!(
            map["body"].as_str().expect("body text").len(),
            MAX_INLINE_BODY_BYTES
        );
        assert_eq!(map["body_truncated"], serde_json::json!(true));
        assert_eq!(map["previous_fingerprint"], Value::Null);
    }

    #[test]
    fn a_non_utf8_body_is_reported_as_absent_rather_than_mangled() {
        let map = payload(
            &listener(),
            &trigger(),
            200,
            "body:abc",
            None,
            &[0xff, 0xfe],
        );
        assert_eq!(map["body"], Value::Null);
    }
}
