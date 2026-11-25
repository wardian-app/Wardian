//! Read-only verification of explicitly scoped, offline copies.

use std::path::PathBuf;

use serde_json::Value;

mod archive_equivalence;

pub fn dispatch(arguments: &[String]) -> Result<(Value, bool), String> {
    let mut arguments = arguments.iter().map(String::as_str);
    if arguments.next() != Some("verify-archive") {
        return Err(usage("expected 'verify-archive'"));
    }

    let mut home = None;
    let mut agent_id = None;
    let mut dry_run = false;
    while let Some(argument) = arguments.next() {
        match argument {
            "--home" if home.is_none() => {
                home = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| usage("--home requires an absolute path"))?,
                ));
            }
            "--agent-id" if agent_id.is_none() => {
                agent_id = Some(
                    arguments
                        .next()
                        .ok_or_else(|| usage("--agent-id requires a value"))?,
                );
            }
            "--dry-run" if !dry_run => dry_run = true,
            "--apply" => return Err(usage("legacy Chat maintenance is read-only")),
            _ => return Err(usage("unknown or repeated argument")),
        }
    }
    let home = home.ok_or_else(|| usage("--home is required; no default home is used"))?;
    let agent_id = agent_id.ok_or_else(|| usage("--agent-id is required"))?;
    if !dry_run {
        return Err(usage("--dry-run is required"));
    }
    if !wardian_core::paths::is_safe_path_component(agent_id) {
        return Err(usage("--agent-id is not a safe path component"));
    }
    let home = archive_equivalence::copy::validate(&home)?;
    let report = archive_equivalence::verify(home, agent_id);
    let complete = report.measured_equivalent();
    Ok((
        serde_json::to_value(report).map_err(|_| "cannot encode archive verification")?,
        complete,
    ))
}

fn usage(reason: &str) -> String {
    format!(
        "{reason}\nusage: wardian-chat-cache-admin verify-archive --home <absolute-existing-copy> --agent-id <id> --dry-run"
    )
}
