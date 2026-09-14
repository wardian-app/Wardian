//! Current canonical-screen model choices. Never classify accumulated output.

pub(crate) fn current_screen_requires_choice(screen: &str) -> bool {
    let lines = screen
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    // The live selection and confirmation footer must terminate the current
    // screen. A historical menu above a composer or a quoted transcript is not
    // an input owner. Explanatory columns are the observed provider copy only.
    let selected = |items: &[&str]| items.iter().filter(|line| line.starts_with("› ")).count() == 1;
    let label = |line: &str, expected: &str| {
        let line = line.strip_prefix("› ").unwrap_or(line);
        line == expected
            || line
                .strip_prefix(expected)
                .is_some_and(|rest| rest.starts_with("  "))
    };
    if let Some(tail) = lines.get(lines.len().saturating_sub(6)..) {
        if tail.len() == 6
            && tail[0] == "Approaching rate limits"
            && tail[1] == "Switch to gpt-5.6-luna for lower credit usage?"
            && selected(&tail[2..5])
            && label(tail[2], "1. Switch to gpt-5.6-luna")
            && label(tail[3], "2. Keep current model")
            && label(tail[4], "3. Keep current model (never show again)")
            && tail[5] == "Press enter to confirm or esc to go back"
        {
            return true;
        }
    }
    let tail = &lines[lines.len().saturating_sub(4)..];
    tail.len() == 4
        && tail[0] == "Choose how you'd like Codex to proceed."
        && selected(&tail[1..3])
        && label(tail[1], "1. Try new model")
        && label(tail[2], "2. Use existing model")
        && tail[3] == "Use ↑/↓ to move, press enter to confirm"
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(crate) const RETAINED: &str = include_str!("fixtures/codex-rate-limit-menu.txt");

    #[test]
    fn retained_rate_limit_menu_requires_explicit_choice() {
        assert!(current_screen_requires_choice(RETAINED));
    }

    #[test]
    fn historical_or_quoted_menu_does_not_block_current_composer() {
        assert!(!current_screen_requires_choice(&format!(
            "{RETAINED}\n› Ask Codex to do anything\n"
        )));
        assert!(!current_screen_requires_choice(
            &RETAINED
                .lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
        assert!(!current_screen_requires_choice(
            "Approaching rate limits\n› normal draft\n"
        ));
    }
}
