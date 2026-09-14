//! Codex query replies share the provider's user-input pipe.
use crate::state::terminal_session::TerminalSessionBroker;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DefaultColorReplies {
    Respond,
    NativeConsoleFallback,
}

impl Default for DefaultColorReplies {
    fn default() -> Self {
        // Codex's Windows probe falls back to GetConsoleScreenBufferInfoEx.
        // Its 100 ms deadline is not observable from our PTY reader: even an
        // immediately dispatched reply may already be late and become user text.
        // Modern ConPTY forwards these queries without answering them itself.
        // Unix keeps its existing replies; other query protocols are unchanged.
        if cfg!(windows) {
            Self::NativeConsoleFallback
        } else {
            Self::Respond
        }
    }
}

#[derive(Default)]
pub(super) struct CodexTerminalThemeProbeResponder {
    default_colors: DefaultColorReplies,
    answered_light_dark: bool,
    answered_foreground: bool,
    answered_background: bool,
    answered_palette_zero: bool,
    tail: Vec<u8>,
}

impl CodexTerminalThemeProbeResponder {
    /// Sends allowed responses through the PTY reader's generation-scoped broker.
    pub(super) fn respond_to_output(
        &mut self,
        broker: &TerminalSessionBroker,
        session_id: &str,
        generation: u64,
        provider_name: &str,
        chunk: &[u8],
        theme: &str,
    ) {
        for response in self.responses_for_chunk(provider_name, chunk, theme) {
            let _ = broker.send_privileged_input_blocking(session_id, generation, response);
        }
    }

    fn responses_for_chunk(
        &mut self,
        provider_name: &str,
        chunk: &[u8],
        theme: &str,
    ) -> Vec<Vec<u8>> {
        if provider_name != "codex" || chunk.is_empty() {
            self.remember_tail(chunk);
            return Vec::new();
        }

        let mut data = self.tail.clone();
        data.extend_from_slice(chunk);
        let terminal_theme = CodexTerminalTheme::from_wardian_theme(theme);
        let mut responses = Vec::new();

        if !self.answered_light_dark && contains_bytes(&data, b"\x1b[?996n") {
            self.answered_light_dark = true;
            responses.push(
                format!(
                    "\x1b[?997;{}n",
                    if terminal_theme.prefers_light { 2 } else { 1 }
                )
                .into_bytes(),
            );
        }

        if self.default_colors == DefaultColorReplies::Respond
            && !self.answered_foreground
            && (contains_bytes(&data, b"\x1b]10;?\x07")
                || contains_bytes(&data, b"\x1b]10;?\x1b\\"))
        {
            self.answered_foreground = true;
            responses.push(format!("\x1b]10;rgb:{}\x1b\\", terminal_theme.foreground).into_bytes());
        }

        if self.default_colors == DefaultColorReplies::Respond
            && !self.answered_background
            && (contains_bytes(&data, b"\x1b]11;?\x07")
                || contains_bytes(&data, b"\x1b]11;?\x1b\\"))
        {
            self.answered_background = true;
            responses.push(format!("\x1b]11;rgb:{}\x1b\\", terminal_theme.background).into_bytes());
        }

        if !self.answered_palette_zero && contains_bytes(&data, b"\x1b]4;0;?\x07") {
            self.answered_palette_zero = true;
            responses.push(format!("\x1b]4;0;rgb:{}\x07", terminal_theme.background).into_bytes());
        }

        self.remember_tail(&data);
        responses
    }

    fn remember_tail(&mut self, data: &[u8]) {
        const MAX_TERMINAL_PROBE_TAIL: usize = 32;
        let start = data.len().saturating_sub(MAX_TERMINAL_PROBE_TAIL);
        self.tail.clear();
        self.tail.extend_from_slice(&data[start..]);
    }
}

struct CodexTerminalTheme {
    foreground: &'static str,
    background: &'static str,
    prefers_light: bool,
}

impl CodexTerminalTheme {
    fn from_wardian_theme(theme: &str) -> Self {
        if theme.trim() == "light" {
            Self {
                foreground: "11/18/27",
                background: "fc/fa/f5",
                prefers_light: true,
            }
        } else {
            Self {
                foreground: "ee/f2/ee",
                background: "02/04/02",
                prefers_light: false,
            }
        }
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|candidate| candidate == needle)
}

#[cfg(test)]
mod tests;
