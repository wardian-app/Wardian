pub fn strip_terminal_controls(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = String::with_capacity(normalized.len());
    let mut chars = normalized.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            output.push(ch);
            continue;
        }

        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for code in chars.by_ref() {
                    if ('@'..='~').contains(&code) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(code) = chars.next() {
                    if code == '\u{7}' {
                        break;
                    }
                    if code == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some('(' | ')' | '*' | '+' | '-' | '.' | '/') => {
                chars.next();
                let _ = chars.next();
            }
            Some(_) => {
                let _ = chars.next();
            }
            None => {}
        }
    }

    output
        .lines()
        .map(|line| line.trim_end_matches('\u{8}'))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_terminal_controls_removes_sgr_color() {
        assert_eq!(strip_terminal_controls("\u{1b}[31mred\u{1b}[0m"), "red");
    }

    #[test]
    fn strip_terminal_controls_removes_csi_cursor_and_clear_controls() {
        assert_eq!(
            strip_terminal_controls("\u{1b}[2J\u{1b}[Hanswer\u{1b}[K"),
            "answer"
        );
    }

    #[test]
    fn strip_terminal_controls_removes_osc_title() {
        assert_eq!(
            strip_terminal_controls("\u{1b}]0;Wardian\u{7}ready"),
            "ready"
        );
    }

    #[test]
    fn strip_terminal_controls_normalizes_crlf_and_cr() {
        assert_eq!(strip_terminal_controls("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn strip_terminal_controls_makes_bibtex_sample_readable() {
        let sample = "\u{1b}[2J\u{1b}[H\u{1b}[1m@article{smith2026,\u{1b}[0m\r\n  title={Wardian},\u{1b}[K\r\n}";

        let cleaned = strip_terminal_controls(sample);

        assert!(!cleaned.contains('\u{1b}'));
        assert!(cleaned.contains("@article{smith2026,"));
        assert!(cleaned.contains("title={Wardian}"));
    }
}
