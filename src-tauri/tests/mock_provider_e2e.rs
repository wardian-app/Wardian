//! Integration test: spawns the mock-agent.cjs script and verifies
//! that its stdout events parse correctly through MockProvider::parse_output().

use std::io::BufRead;
use std::process::{Command, Stdio};

/// Resolves the mock-agent.cjs script relative to the repo root.
fn mock_script_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let script = std::path::Path::new(manifest_dir)
        .join("..")
        .join("scripts")
        .join("mock-agent.cjs");
    assert!(script.exists(), "mock-agent.cjs not found at {:?}", script);
    script.to_string_lossy().to_string()
}

#[test]
fn basic_scenario_emits_expected_events() {
    use wardian_app_lib::models::provider::{AgentEvent, AgentProvider};
    use wardian_app_lib::providers::mock::MockProvider;

    let provider = MockProvider::new();
    let script = mock_script_path();

    let mut child = Command::new("node")
        .arg(&script)
        .env("WARDIAN_MOCK_SCENARIO", "basic")
        .env("WARDIAN_MOCK_DELAY_MS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn mock-agent.cjs (is Node.js installed?)");

    let stdout = child.stdout.take().unwrap();
    let reader = std::io::BufReader::new(stdout);

    let events: Vec<AgentEvent> = reader
        .lines()
        .filter_map(|line| {
            let line = line.ok()?;
            provider.parse_output(&line)
        })
        .collect();
    let status = child.wait().expect("mock agent exits");
    assert!(status.success(), "mock agent should exit successfully");

    assert!(
        events.len() >= 4,
        "Expected at least 4 events for basic scenario, got {}",
        events.len()
    );

    // Verify event sequence: Init, UserQuery, Generating, (ModelResponse or TurnCompleted)
    assert!(
        matches!(events[0], AgentEvent::Init { .. }),
        "First event should be Init, got {:?}",
        events[0]
    );
    assert_eq!(events[1], AgentEvent::UserQuery);
    assert_eq!(events[2], AgentEvent::Generating);
    // Last event should be TurnCompleted
    assert_eq!(
        events.last().unwrap(),
        &AgentEvent::TurnCompleted,
        "Last event should be TurnCompleted"
    );
}

#[test]
fn headless_scenario_emits_json_and_exits() {
    let script = mock_script_path();

    let output = Command::new("node")
        .arg(&script)
        .arg("--print")
        .arg("test prompt")
        .env("WARDIAN_MOCK_DELAY_MS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to spawn mock-agent.cjs");

    assert!(output.status.success(), "Headless mode should exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("Headless output should be valid JSON");

    assert!(
        parsed.get("response").is_some(),
        "Headless output should have a 'response' field"
    );
    assert_eq!(
        parsed.get("status").and_then(|v| v.as_str()),
        Some("ok"),
        "Headless output status should be 'ok'"
    );
}

#[test]
fn failure_scenario_exits_nonzero() {
    let script = mock_script_path();

    let output = Command::new("node")
        .arg(&script)
        .env("WARDIAN_MOCK_SCENARIO", "failure")
        .env("WARDIAN_MOCK_DELAY_MS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to spawn mock-agent.cjs");

    assert!(
        !output.status.success(),
        "Failure scenario should exit with non-zero code"
    );
}

#[test]
fn long_output_scenario_emits_many_lines() {
    use wardian_app_lib::models::provider::AgentProvider;
    use wardian_app_lib::providers::mock::MockProvider;

    let provider = MockProvider::new();
    let script = mock_script_path();

    let mut child = Command::new("node")
        .arg(&script)
        .env("WARDIAN_MOCK_SCENARIO", "long_output")
        .env("WARDIAN_MOCK_DELAY_MS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn mock-agent.cjs");

    let stdout = child.stdout.take().unwrap();
    let reader = std::io::BufReader::new(stdout);

    let mut total_lines = 0;
    let mut parsed_events = 0;
    for line in reader.lines() {
        let line = line.unwrap();
        total_lines += 1;
        if provider.parse_output(&line).is_some() {
            parsed_events += 1;
        }
    }
    let status = child.wait().expect("mock agent exits");
    assert!(status.success(), "mock agent should exit successfully");

    // 200 text lines + ~5 JSON event lines
    assert!(
        total_lines > 200,
        "Expected > 200 lines total, got {}",
        total_lines
    );
    assert!(
        parsed_events >= 4,
        "Expected at least 4 parsed events, got {}",
        parsed_events
    );
}
