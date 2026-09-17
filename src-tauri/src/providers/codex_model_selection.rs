use std::time::{Duration, Instant};

use regex::Regex;

use crate::providers::models::ProviderModelCatalog;
use crate::state::terminal_session::TerminalSessionBroker;
use crate::utils::delivery_profile::delivery_profile;
use crate::utils::delivery_transaction::plan_terminal_payload;

const MODEL_PICKER_TITLE: &str = "Select Model and Effort";
const ADVANCED_REASONING_TITLE: &str = "Advanced Reasoning";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexLiveModelSelection {
    pub model: String,
    pub effort: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexLiveSelectionError {
    DefaultUnavailable(String),
    Invalid(String),
}

impl std::fmt::Display for CodexLiveSelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DefaultUnavailable(message) | Self::Invalid(message) => f.write_str(message),
        }
    }
}

#[derive(Clone, Copy)]
struct PickerTiming {
    screen_timeout: Duration,
    poll_interval: Duration,
    command_submit_delay: Duration,
}

impl Default for PickerTiming {
    fn default() -> Self {
        Self {
            screen_timeout: Duration::from_secs(8),
            poll_interval: Duration::from_millis(40),
            command_submit_delay: Duration::from_millis(delivery_profile("codex").submit_delay_ms),
        }
    }
}

/// Resolves provider defaults before opening Codex's interactive picker.
///
/// Wardian persists an omitted model or effort as "use the provider default",
/// but the live picker requires concrete choices. The provider-owned catalog
/// is therefore the only source used to expand those defaults.
pub fn resolve_live_selection(
    catalog: &ProviderModelCatalog,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<CodexLiveModelSelection, String> {
    resolve_live_selection_for_settings(catalog, model, effort).map_err(|error| error.to_string())
}

/// Resolve the complete concrete pair while retaining whether an unavailable
/// provider default should defer application after persistence.
pub fn resolve_live_selection_for_settings(
    catalog: &ProviderModelCatalog,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<CodexLiveModelSelection, CodexLiveSelectionError> {
    let selected_model = match model.map(str::trim).filter(|value| !value.is_empty()) {
        Some(model) => catalog
            .models
            .iter()
            .find(|option| option.id == model)
            .ok_or_else(|| {
                CodexLiveSelectionError::Invalid(format!(
                    "Codex model {model} is not present in the live catalog"
                ))
            })?,
        None => catalog
            .models
            .iter()
            .find(|option| option.is_default)
            .ok_or_else(|| {
                CodexLiveSelectionError::DefaultUnavailable(
                    catalog.refresh_error.clone().unwrap_or_else(|| {
                        "Codex catalog has no authoritative default model".to_string()
                    }),
                )
            })?,
    };

    let selected_effort = effort
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| selected_model.default_effort.clone())
        .ok_or_else(|| {
            CodexLiveSelectionError::DefaultUnavailable(format!(
                "Codex model {} did not report a default reasoning effort",
                selected_model.id
            ))
        })?;

    if !selected_model.effort_options.contains(&selected_effort) {
        return Err(CodexLiveSelectionError::Invalid(format!(
            "Codex model {} does not support reasoning effort {}",
            selected_model.id, selected_effort
        )));
    }

    Ok(CodexLiveModelSelection {
        model: selected_model.id.clone(),
        effort: selected_effort,
    })
}

/// Applies a model and reasoning-effort pair through Codex's documented
/// interactive `/model` flow. Codex does not support `/model <model> <effort>`.
pub async fn apply_live_selection(
    broker: &TerminalSessionBroker,
    session_id: &str,
    selection: &CodexLiveModelSelection,
) -> Result<(), String> {
    let result =
        apply_live_selection_with_timing(broker, session_id, selection, PickerTiming::default())
            .await;

    if result.is_err() {
        dismiss_picker(broker, session_id).await;
    }
    result
}

async fn apply_live_selection_with_timing(
    broker: &TerminalSessionBroker,
    session_id: &str,
    selection: &CodexLiveModelSelection,
    timing: PickerTiming,
) -> Result<(), String> {
    let command = plan_terminal_payload(&delivery_profile("codex"), "/model");
    send_input(broker, session_id, &command.payload_bytes).await?;
    tokio::time::sleep(timing.command_submit_delay).await;
    send_input(broker, session_id, &command.submit_key).await?;

    let model_screen =
        wait_for_screen(broker, session_id, timing, "Codex model picker", |screen| {
            screen.contains(MODEL_PICKER_TITLE) && option_number(screen, &selection.model).is_some()
        })
        .await?;
    choose_option(broker, session_id, timing, &model_screen, &selection.model).await?;

    let effort_title = format!("Select Reasoning Level for {}", selection.model);
    let effort_label = effort_picker_label(&selection.effort)?;
    let effort_screen = wait_for_screen(
        broker,
        session_id,
        timing,
        "Codex reasoning picker",
        |screen| {
            screen.contains(&effort_title)
                && (option_number(screen, effort_label).is_some()
                    || is_advanced_effort(&selection.effort)
                        && advanced_reasoning_option(screen).is_some())
        },
    )
    .await?;

    if is_advanced_effort(&selection.effort)
        && option_number(&effort_screen, effort_label).is_none()
    {
        let advanced_entry = advanced_reasoning_option(&effort_screen)
            .ok_or_else(|| "Codex picker did not expose advanced reasoning".to_string())?;
        choose_option(broker, session_id, timing, &effort_screen, advanced_entry).await?;
        let advanced_screen = wait_for_screen(
            broker,
            session_id,
            timing,
            "Codex advanced reasoning picker",
            |screen| {
                screen.contains(ADVANCED_REASONING_TITLE)
                    && option_number(screen, effort_label).is_some()
            },
        )
        .await?;
        choose_option(broker, session_id, timing, &advanced_screen, effort_label).await?;
    } else {
        choose_option(broker, session_id, timing, &effort_screen, effort_label).await?;
    }

    wait_for_screen(
        broker,
        session_id,
        timing,
        "Codex model-change confirmation",
        |screen| selection_confirmed(screen, selection, effort_label),
    )
    .await?;
    Ok(())
}

fn effort_picker_label(effort: &str) -> Result<&'static str, String> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "none" => Ok("None"),
        "minimal" => Ok("Minimal"),
        "low" => Ok("Low"),
        "medium" => Ok("Medium"),
        "high" => Ok("High"),
        "xhigh" => Ok("Extra high"),
        "max" => Ok("Max"),
        "ultra" => Ok("Ultra"),
        _ => Err(format!("Unsupported Codex reasoning effort: {effort}")),
    }
}

fn is_advanced_effort(effort: &str) -> bool {
    matches!(effort.trim().to_ascii_lowercase().as_str(), "max" | "ultra")
}

fn advanced_reasoning_option(screen: &str) -> Option<&'static str> {
    ["More reasoning", "More reasoning…", "More reasoning..."]
        .into_iter()
        .find(|label| option_number(screen, label).is_some())
}

fn option_number(screen: &str, label: &str) -> Option<usize> {
    let pattern = format!(
        r"(?m)^[^\S\r\n]*(?:›[^\S\r\n]*)?(\d+)\.[^\S\r\n]+{}(?:[^\S\r\n]|$)",
        regex::escape(label)
    );
    Regex::new(&pattern)
        .ok()?
        .captures(screen)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

fn selected_option_number(screen: &str) -> Option<usize> {
    Regex::new(r"(?m)^[^\S\r\n]*›[^\S\r\n]*(\d+)\.")
        .ok()?
        .captures(screen)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

async fn choose_option(
    broker: &TerminalSessionBroker,
    session_id: &str,
    timing: PickerTiming,
    screen: &str,
    label: &str,
) -> Result<(), String> {
    let current = selected_option_number(screen)
        .ok_or_else(|| "Codex picker did not expose its current option".to_string())?;
    let target = option_number(screen, label)
        .ok_or_else(|| format!("Codex picker did not expose option {label}"))?;

    let (key, count) = if target >= current {
        (b"\x1b[B".as_slice(), target - current)
    } else {
        (b"\x1b[A".as_slice(), current - target)
    };
    for _ in 0..count {
        send_input(broker, session_id, key).await?;
    }
    if count > 0 {
        wait_for_screen(
            broker,
            session_id,
            timing,
            &format!("Codex picker to select {label}"),
            |next_screen| {
                option_number(next_screen, label)
                    .zip(selected_option_number(next_screen))
                    .is_some_and(|(target, selected)| target == selected)
            },
        )
        .await?;
    }
    send_input(broker, session_id, b"\r").await
}

async fn wait_for_screen<F>(
    broker: &TerminalSessionBroker,
    session_id: &str,
    timing: PickerTiming,
    description: &str,
    predicate: F,
) -> Result<String, String>
where
    F: Fn(&str) -> bool,
{
    let started = Instant::now();
    loop {
        let snapshot = broker
            .snapshot(session_id)
            .await
            .map_err(|error| format!("Unable to inspect Codex terminal: {error}"))?;
        if predicate(&snapshot.visible_grid) {
            return Ok(snapshot.visible_grid);
        }
        if started.elapsed() >= timing.screen_timeout {
            return Err(format!("Timed out waiting for {description}"));
        }
        tokio::time::sleep(timing.poll_interval).await;
    }
}

fn selection_confirmed(
    screen: &str,
    selection: &CodexLiveModelSelection,
    effort_label: &str,
) -> bool {
    if screen.contains(MODEL_PICKER_TITLE)
        || screen.contains("Select Reasoning Level for")
        || screen.contains(ADVANCED_REASONING_TITLE)
    {
        return false;
    }

    let lower = screen.to_ascii_lowercase();
    let model = selection.model.to_ascii_lowercase();
    let effort = effort_label.to_ascii_lowercase();
    (lower.contains("model changed to") || lower.lines().any(|line| line.contains(&model)))
        && lower.contains(&model)
        && lower.contains(&effort)
}

async fn send_input(
    broker: &TerminalSessionBroker,
    session_id: &str,
    bytes: &[u8],
) -> Result<(), String> {
    broker
        .send_privileged_input(session_id, bytes.to_vec())
        .await
        .map_err(|error| format!("Unable to control Codex model picker: {error}"))
}

async fn dismiss_picker(broker: &TerminalSessionBroker, session_id: &str) {
    for _ in 0..3 {
        if broker
            .send_privileged_input(session_id, b"\x1b".to_vec())
            .await
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::models::{ProviderModelCatalog, ProviderModelOption};
    use crate::state::terminal_session::{TerminalRuntimeHandles, TerminalSessionBroker};
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use wardian_core::models::TerminalGeometry;

    fn catalog() -> ProviderModelCatalog {
        ProviderModelCatalog {
            provider: "codex".to_string(),
            version: Some("codex-cli 0.149.0".to_string()),
            source: "live_catalog".to_string(),
            models: vec![
                ProviderModelOption {
                    id: "gpt-default".to_string(),
                    display_name: "GPT Default".to_string(),
                    effort_options: vec!["low".to_string(), "high".to_string()],
                    default_effort: Some("low".to_string()),
                    is_default: true,
                },
                ProviderModelOption {
                    id: "gpt-target".to_string(),
                    display_name: "GPT Target".to_string(),
                    effort_options: vec![
                        "low".to_string(),
                        "medium".to_string(),
                        "high".to_string(),
                        "xhigh".to_string(),
                        "max".to_string(),
                        "ultra".to_string(),
                    ],
                    default_effort: Some("medium".to_string()),
                    is_default: false,
                },
            ],
            refresh_error: None,
        }
    }

    #[test]
    fn provider_defaults_resolve_to_concrete_picker_choices() {
        assert_eq!(
            resolve_live_selection(&catalog(), None, None).expect("resolve defaults"),
            CodexLiveModelSelection {
                model: "gpt-default".to_string(),
                effort: "low".to_string(),
            }
        );
        assert_eq!(
            resolve_live_selection(&catalog(), Some("gpt-target"), None)
                .expect("resolve model default"),
            CodexLiveModelSelection {
                model: "gpt-target".to_string(),
                effort: "medium".to_string(),
            }
        );
    }

    #[test]
    fn missing_authoritative_default_is_deferred_by_settings_resolution() {
        let mut catalog = catalog();
        catalog.models[0].is_default = false;
        let error = resolve_live_selection_for_settings(&catalog, None, None)
            .expect_err("missing provider default must remain unresolved");
        assert!(matches!(
            error,
            CodexLiveSelectionError::DefaultUnavailable(_)
        ));
    }

    #[test]
    fn picker_parser_matches_exact_options_and_pointer() {
        let screen = "  Select Reasoning Level for gpt-target\n\n    1. Low\n  › 2. High\n    3. Extra high\n";
        assert_eq!(option_number(screen, "High"), Some(2));
        assert_eq!(option_number(screen, "Extra high"), Some(3));
        assert_eq!(selected_option_number(screen), Some(2));
        assert_eq!(
            advanced_reasoning_option("› 5. More reasoning…"),
            Some("More reasoning…")
        );
    }

    #[tokio::test]
    async fn controller_drives_model_effort_and_advanced_reasoning_screens() {
        let broker = Arc::new(TerminalSessionBroker::default());
        let (input_tx, mut input_rx) = mpsc::channel(32);
        let generation = broker
            .start_or_replace_runtime(
                "agent-1",
                TerminalRuntimeHandles::new(input_tx, |_| Ok(())),
                TerminalGeometry {
                    rows: 30,
                    cols: 120,
                },
            )
            .await
            .expect("start runtime");
        let simulated_broker = broker.clone();
        let simulation = tokio::spawn(async move {
            assert_eq!(
                input_rx.recv().await.as_deref(),
                Some(b"\x1b[200~/model\x1b[201~".as_slice())
            );
            assert_eq!(input_rx.recv().await.as_deref(), Some(b"\r".as_slice()));
            render(
                simulated_broker.clone(),
                generation,
                "Select Model and Effort\n\n› 1. gpt-default\n  2. gpt-target",
            )
            .await;
            assert_eq!(input_rx.recv().await.as_deref(), Some(b"\x1b[B".as_slice()));
            render(
                simulated_broker.clone(),
                generation,
                "Select Model and Effort\n\n  1. gpt-default\n› 2. gpt-target",
            )
            .await;
            assert_eq!(input_rx.recv().await.as_deref(), Some(b"\r".as_slice()));
            render(
                simulated_broker.clone(),
                generation,
                "Select Reasoning Level for gpt-target\n\n› 1. Low\n  2. Medium\n  3. High\n  4. Extra high\n  5. More reasoning",
            )
            .await;
            for _ in 0..4 {
                assert_eq!(input_rx.recv().await.as_deref(), Some(b"\x1b[B".as_slice()));
            }
            render(
                simulated_broker.clone(),
                generation,
                "Select Reasoning Level for gpt-target\n\n  1. Low\n  2. Medium\n  3. High\n  4. Extra high\n› 5. More reasoning",
            )
            .await;
            assert_eq!(input_rx.recv().await.as_deref(), Some(b"\r".as_slice()));
            render(
                simulated_broker.clone(),
                generation,
                "Advanced Reasoning\n\n› 1. Max\n  2. Ultra",
            )
            .await;
            assert_eq!(input_rx.recv().await.as_deref(), Some(b"\x1b[B".as_slice()));
            render(
                simulated_broker.clone(),
                generation,
                "Advanced Reasoning\n\n  1. Max\n› 2. Ultra",
            )
            .await;
            assert_eq!(input_rx.recv().await.as_deref(), Some(b"\r".as_slice()));
            render(
                simulated_broker,
                generation,
                "Model changed to gpt-target Ultra\n\n  gpt-target ultra",
            )
            .await;
        });

        apply_live_selection_with_timing(
            &broker,
            "agent-1",
            &CodexLiveModelSelection {
                model: "gpt-target".to_string(),
                effort: "ultra".to_string(),
            },
            PickerTiming {
                screen_timeout: Duration::from_secs(2),
                poll_interval: Duration::from_millis(5),
                command_submit_delay: Duration::ZERO,
            },
        )
        .await
        .expect("apply live selection");
        simulation.await.expect("simulation");
    }

    async fn render(broker: Arc<TerminalSessionBroker>, generation: u64, screen: &str) {
        let bytes = format!("\x1b[2J\x1b[H{}", screen.replace('\n', "\r\n")).into_bytes();
        tokio::task::spawn_blocking(move || {
            broker
                .process_output_blocking("agent-1", generation, bytes)
                .expect("render simulated screen");
        })
        .await
        .expect("join render task");
    }
}
