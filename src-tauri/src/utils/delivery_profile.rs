#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitKey {
    CarriageReturn,
    OpenCodeKittyEnter,
}

impl SubmitKey {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::CarriageReturn => b"\r",
            Self::OpenCodeKittyEnter => b"\x1b[13u",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BracketedPasteProfile {
    pub enabled: bool,
    pub min_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryProfile {
    pub provider: String,
    pub submit_key: SubmitKey,
    pub submit_delay_ms: u64,
    pub bracketed_paste: BracketedPasteProfile,
    pub input_ready_markers: &'static [&'static str],
    pub busy_markers: &'static [&'static str],
}

pub fn delivery_profile(provider: &str) -> DeliveryProfile {
    let normalized = provider.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "codex" => DeliveryProfile {
            provider: normalized,
            submit_key: SubmitKey::CarriageReturn,
            submit_delay_ms: 0,
            bracketed_paste: BracketedPasteProfile {
                enabled: true,
                min_bytes: 1,
            },
            input_ready_markers: &["›", "Use /skills"],
            busy_markers: &["esc to interrupt", "Working"],
        },
        "claude" => DeliveryProfile {
            provider: normalized,
            submit_key: SubmitKey::CarriageReturn,
            submit_delay_ms: 500,
            bracketed_paste: BracketedPasteProfile {
                enabled: true,
                min_bytes: 2048,
            },
            input_ready_markers: &["❯"],
            busy_markers: &["esc to interrupt"],
        },
        "gemini" => DeliveryProfile {
            provider: normalized,
            submit_key: SubmitKey::CarriageReturn,
            submit_delay_ms: 500,
            bracketed_paste: BracketedPasteProfile {
                enabled: false,
                min_bytes: usize::MAX,
            },
            input_ready_markers: &[">"],
            busy_markers: &["Working"],
        },
        "opencode" => DeliveryProfile {
            provider: normalized,
            submit_key: SubmitKey::OpenCodeKittyEnter,
            submit_delay_ms: 250,
            bracketed_paste: BracketedPasteProfile {
                enabled: true,
                min_bytes: 2048,
            },
            input_ready_markers: &[">"],
            busy_markers: &["Working"],
        },
        "antigravity" => DeliveryProfile {
            provider: normalized,
            submit_key: SubmitKey::CarriageReturn,
            submit_delay_ms: 500,
            bracketed_paste: BracketedPasteProfile {
                enabled: false,
                min_bytes: usize::MAX,
            },
            input_ready_markers: &[">"],
            busy_markers: &["Working"],
        },
        _ => DeliveryProfile {
            provider: normalized,
            submit_key: SubmitKey::CarriageReturn,
            submit_delay_ms: 1000,
            bracketed_paste: BracketedPasteProfile {
                enabled: false,
                min_bytes: usize::MAX,
            },
            input_ready_markers: &[],
            busy_markers: &[],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_profile_uses_immediate_bracketed_paste_and_return_key() {
        let profile = delivery_profile("codex");

        assert_eq!(profile.provider, "codex");
        assert_eq!(profile.submit_key, SubmitKey::CarriageReturn);
        assert_eq!(profile.submit_delay_ms, 0);
        assert!(profile.bracketed_paste.enabled);
        assert_eq!(profile.bracketed_paste.min_bytes, 1);
    }

    #[test]
    fn opencode_profile_uses_kitty_enter_key() {
        let profile = delivery_profile("opencode");

        assert_eq!(profile.submit_key.bytes(), b"\x1b[13u");
    }

    #[test]
    fn every_user_provider_has_a_profile() {
        for provider in ["codex", "claude", "gemini", "opencode", "antigravity"] {
            let profile = delivery_profile(provider);
            assert_eq!(profile.provider, provider);
        }
    }

    #[test]
    fn unknown_provider_falls_back_conservatively() {
        let profile = delivery_profile("new-provider");

        assert_eq!(profile.provider, "new-provider");
        assert_eq!(profile.submit_key.bytes(), b"\r");
        assert_eq!(profile.submit_delay_ms, 1000);
        assert!(!profile.bracketed_paste.enabled);
    }
}
