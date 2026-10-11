//! Current Claude composer recognition, separate from startup/trust publication.
use crate::utils::strip_ansi_controls;

pub(super) fn claude_output_has_ready_prompt(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    let mut trailing_metadata_lines = 0usize;
    for line in cleaned.lines().rev().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('❯') {
            return true;
        }
        if trailing_metadata_lines < 4 && claude_ready_prompt_trailing_metadata_line(line) {
            trailing_metadata_lines += 1;
            continue;
        }
        return false;
    }
    false
}

fn claude_ready_prompt_trailing_metadata_line(line: &str) -> bool {
    if line.contains('⏵') {
        return true;
    }
    line.chars()
        .all(|ch| ch == '─' || ch == '-' || ch.is_whitespace())
        || claude_ready_prompt_model_status_line(line)
}

// Claude renders Wardian's model/workspace/name status alongside its native
// effort hint below the composer. Recognize that bounded metadata shape only;
// arbitrary prose and approval choices must still stop the reverse tail scan.
fn claude_ready_prompt_model_status_line(line: &str) -> bool {
    let fields = line.split('|').map(str::trim).collect::<Vec<_>>();
    if !(3..=4).contains(&fields.len()) || fields[1].is_empty() {
        return false;
    }
    let mut model = fields[0].split_whitespace();
    if !matches!(model.next(), Some("Opus" | "Sonnet" | "Haiku")) {
        return false;
    }
    let Some(version) = model.next() else {
        return false;
    };
    if model.next().is_some()
        || !version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
    {
        return false;
    }
    if fields.len() == 4 {
        let rate = fields[2]
            .strip_prefix("rate:")
            .and_then(|rate| rate.strip_suffix('%'))
            .filter(|rate| {
                rate.split('.').count() <= 2
                    && rate
                        .split('.')
                        .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
            })
            .and_then(|rate| rate.parse::<f64>().ok());
        if !rate.is_some_and(|rate| (0.0..=100.0).contains(&rate)) {
            return false;
        }
    }
    let Some((name, effort)) = fields[fields.len() - 1].rsplit_once('\u{25cf}') else {
        // A second captured Wardian status variant has a numeric rate and
        // agent name without the native effort decoration.
        return fields.len() == 4 && !fields[3].is_empty();
    };
    !name.trim().is_empty()
        && matches!(
            effort.trim(),
            "low · /effort" | "medium · /effort" | "high · /effort" | "max · /effort"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{
        provider_output_has_startup_ready_prompt, provider_output_requires_startup_action,
    };

    #[test]
    fn claude_ready_prompt_accepts_captured_model_status_footer() {
        // Native Claude 2.1.296 current-screen capture, with private identity
        // sanitized. The status row sits between composer border and footer.
        let screen = "\u{276f}\u{a0}Try \"how do I log an error?\"\n\u{2500}\u{2500}\u{2500}\n  Opus 5.5 | worktree-source | Continuity-native-readiness    \u{25cf} high \u{b7} /effort\n  \u{23f5}\u{23f5} bypass permissions on (shift+tab to cycle) \u{b7} \u{2190} 1 agent";
        assert!(provider_output_has_startup_ready_prompt("claude", screen));
        assert!(!provider_output_requires_startup_action("claude", screen));
        assert!(claude_output_has_ready_prompt(screen));
    }

    #[test]
    fn claude_ready_prompt_accepts_captured_rate_only_status_footer() {
        // Another final native capture retained this rate-only Wardian status.
        let screen = "❯ Try a read-only task\n───\nOpus 5.5 | worktree-source | rate:0% | Disposable-agent\n⏵⏵ bypass permissions on (shift+tab to cycle)";
        assert!(provider_output_has_startup_ready_prompt("claude", screen));
        assert!(!provider_output_requires_startup_action("claude", screen));
        assert!(claude_output_has_ready_prompt(screen));
    }

    #[test]
    fn claude_ready_prompt_accepts_model_status_with_rate() {
        for row in [
            "Sonnet 4.6 | workspace | rate:0% | Disposable-agent ● medium · /effort",
            "Haiku 4.5 | workspace | Disposable-agent ● low · /effort",
        ] {
            assert!(claude_output_has_ready_prompt(&format!(
                "❯\n───\n{row}\n⏵⏵ bypass permissions on (shift+tab to cycle)"
            )));
        }
    }

    #[test]
    fn claude_ready_prompt_keeps_unknown_rows_and_choices_blocked() {
        let row = "Opus 5.5 | workspace | Disposable-agent ● high · /effort";
        for blocked in [
            "Unknown status row",
            "Do you want to proceed?\n1. Allow once\n2. No",
            "Quick safety check: Is this a project you created or one you trust?\n1. No, exit\n2. Yes, I trust this folder",
            "Bypass Permissions mode\n1. No, exit\n2. Yes, I accept",
            "Connecting...",
        ] {
            assert!(!claude_output_has_ready_prompt(&format!("❯\n{blocked}\n{row}\n⏵⏵ bypass permissions on (shift+tab to cycle)")));
        }
        for unknown in [
            "Other 5.5 | workspace | Disposable-agent ● high · /effort",
            "Opus preview | workspace | Disposable-agent ● high · /effort",
            "Opus 5.5 | workspace | rate:unknown% | Disposable-agent ● high · /effort",
            "Opus 5.5 | workspace | rate:1e1% | Disposable-agent",
            "Opus 5.5 | workspace | rate:-1% | Disposable-agent",
            "Opus 5.5 | workspace | rate:101% | Disposable-agent",
            "Opus 5.5 | workspace | rate:0% | ",
            "Opus 5.5 | workspace | Disposable-agent",
            "Opus 5.5 | workspace | Disposable-agent approves everything",
        ] {
            assert!(!claude_output_has_ready_prompt(&format!(
                "❯\n{unknown}\n⏵⏵ bypass permissions on (shift+tab to cycle)"
            )));
        }
    }

    #[test]
    fn claude_ready_prompt_preserves_bounded_metadata_tail() {
        let row = "Opus 5.5 | workspace | Disposable-agent ● high · /effort";
        assert!(claude_output_has_ready_prompt(&format!(
            "❯\n───\n───\n{row}\n⏵⏵ bypass permissions on (shift+tab to cycle)"
        )));
        assert!(!claude_output_has_ready_prompt(&format!(
            "❯\n───\n───\n───\n{row}\n⏵⏵ bypass permissions on (shift+tab to cycle)"
        )));
        assert!(!claude_output_has_ready_prompt(&format!(
            "{row}\n⏵⏵ bypass permissions on (shift+tab to cycle)"
        )));
    }
}
