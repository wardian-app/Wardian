use super::strip_ansi_controls;

/// Recognizes readiness only in the current Pi editor frame supplied by the caller.
pub(super) fn pi_output_has_startup_ready_prompt(output: &str) -> bool {
    let cleaned = strip_ansi_controls(output).replace('\r', "\n");
    let screen = cleaned.lines().map(str::trim).collect::<Vec<_>>();
    // Resume can scroll the banner out of the canonical screen. In that case,
    // require the empty editor between its two borders immediately above the
    // workspace footer, rather than treating transcript footer text as ready.
    let editor_start = screen
        .iter()
        .rposition(|line| pi_editor_border(line))
        .and_then(|bottom| {
            let top = screen[..bottom]
                .iter()
                .rposition(|line| pi_editor_border(line))?;
            (bottom > top + 1
                && screen[top + 1..bottom].iter().all(|line| line.is_empty())
                && screen
                    .get(bottom + 1)
                    .is_some_and(|line| pi_line_looks_like_workspace_footer(line)))
            .then_some(top)
        });
    let banner_start = screen
        .iter()
        .rposition(|line| line.to_ascii_lowercase().starts_with("pi v"));
    // A visible banner still owns its loading/error notices above the editor.
    let frame_start = match (banner_start, editor_start) {
        (Some(banner), Some(editor)) => Some(banner.min(editor)),
        (banner, editor) => banner.or(editor),
    };
    let Some(frame_start) = frame_start else {
        return false;
    };
    let frame = screen[frame_start..]
        .iter()
        .copied()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if frame.iter().any(|line| pi_line_is_blocking_status(line)) {
        return false;
    }

    frame.iter().enumerate().any(|(index, line)| {
        if !line.contains("%/") || !line.contains("(auto)") || line.split_whitespace().count() < 2 {
            return false;
        }
        let footer_is_current = index + 1 == frame.len()
            || (index + 2 == frame.len() && frame[index + 1].contains(" • "));
        if !footer_is_current {
            return false;
        }

        let context_start = index.saturating_sub(2);
        let context_end = (index + 4).min(frame.len());
        let context = &frame[context_start..context_end];
        let preceding_context = &frame[context_start..index];
        let has_editor_footer_context = preceding_context
            .iter()
            .any(|candidate| pi_line_looks_like_workspace_footer(candidate));
        let has_current_model_footer = context.iter().any(|candidate| {
            candidate.split_once(") ").is_some_and(|(_, model)| {
                model
                    .split_once(" •")
                    .is_some_and(|(name, _)| !name.trim().is_empty())
            })
        });

        has_editor_footer_context
            && (has_current_model_footer
                || preceding_context
                    .iter()
                    .any(|candidate| candidate.contains(" • ")))
    })
}

fn pi_editor_border(line: &str) -> bool {
    line.chars().count() >= 8 && line.chars().all(|character| character == '─')
}

fn pi_line_looks_like_workspace_footer(line: &str) -> bool {
    let trimmed = line.trim();
    let bytes = trimmed.as_bytes();
    let windows_drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    let absolute_path = trimmed.starts_with('/') || trimmed.starts_with("\\\\");
    let sanitized_fixture_path =
        trimmed.starts_with("<workspace-root>/") || trimmed.starts_with("<workspace-root>\\");
    let home_path = trimmed.starts_with("~/") || trimmed.starts_with("~\\");

    windows_drive || absolute_path || sanitized_fixture_path || home_path
}

fn pi_line_is_blocking_status(line: &str) -> bool {
    let normalized = line.trim().to_ascii_lowercase();
    if normalized.contains("no models available")
        || normalized.starts_with("error:")
        || normalized.contains(" error:")
        || (normalized.starts_with("model:") && normalized.contains("loading"))
    {
        return true;
    }

    let standalone_status = ["loading", "starting", "connecting"].iter().any(|&marker| {
        normalized == marker
            || normalized
                .strip_prefix(marker)
                .and_then(|suffix| suffix.chars().next())
                .is_some_and(|first| {
                    first.is_ascii_whitespace() || matches!(first, ':' | '.' | '…')
                })
    });
    if standalone_status {
        return true;
    }

    let footer_like = normalized.contains("%/") || normalized.contains("(auto)");
    footer_like
        && ["loading", "starting", "connecting"]
            .iter()
            .any(|marker| normalized.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::pi_output_has_startup_ready_prompt;

    #[test]
    fn resumed_editor_is_ready_without_the_scrolled_startup_banner() {
        let ready = "Previous answer\n\n────────────────────────────────\n\n────────────────────────────────\n<workspace-root>/project (fix/startup...)\n↑4.0k ↓9 $0.001 (sub) 1.5%/272k (auto) gpt-5.6-luna • high";
        assert!(pi_output_has_startup_ready_prompt(ready));
        assert!(!pi_output_has_startup_ready_prompt(&format!(
            "pi v0.99.0\nError: provider authentication failed\n{ready}"
        )));
        assert!(!pi_output_has_startup_ready_prompt(&format!(
            "pi v0.99.0\nLoading model…\n{ready}"
        )));
        assert!(!pi_output_has_startup_ready_prompt(&format!(
            "pi v0.99.0\nNo models available. Use /login.\n{ready}"
        )));
        assert!(pi_output_has_startup_ready_prompt(
            &ready.replace(" • high", " •")
        ));
        assert!(!pi_output_has_startup_ready_prompt(&ready.replace(
            "\n\n────────────────",
            "\nLoading model…\n────────────────"
        )));
        assert!(!pi_output_has_startup_ready_prompt(
            &ready
                .replace("Previous answer", "Draft: quoted footer")
                .replace("────────────────────────────────\n\n", "")
        ));
        assert!(!pi_output_has_startup_ready_prompt(&format!(
            "{ready}\nLoading model…"
        )));
    }

    #[test]
    fn loading_status_blocks_readiness_before_and_after_footer() {
        let before_footer = "pi v0.84.2\n────────────────\nC:\\workspace • Wardian-Pi\nLoading model…\n0.0%/33k (auto) echo";
        assert!(!pi_output_has_startup_ready_prompt(before_footer));

        let after_footer = "pi v0.84.2\n────────────────\nC:\\workspace • Wardian-Pi\n0.0%/33k (auto) echo\nLoading model…";
        assert!(!pi_output_has_startup_ready_prompt(after_footer));

        let ready =
            "pi v0.84.2\n────────────────\nC:\\workspace • Wardian-Pi\n0.0%/33k (auto) echo";
        assert!(pi_output_has_startup_ready_prompt(ready));
    }
}
