use std::fs::OpenOptions;
use std::io::Write;

fn debug_log_path() -> std::path::PathBuf {
    // Write to ~/.wardian/wardian_debug.log so logs are reachable from any cwd,
    // including production builds launched from read-only install directories.
    std::env::var("WARDIAN_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| std::path::PathBuf::from(v).join("wardian_debug.log"))
        .or_else(|| {
            dirs::home_dir().map(|h| h.join(".wardian").join("wardian_debug.log"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("wardian_debug.log"))
}

pub fn log_debug(msg: &str) {
    let path = debug_log_path();
    for _ in 0..5 {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(file, "{}", msg);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

// No-op stubs when terminal-trace feature is disabled.
// These allow call sites to remain unconditional without cfg guards.
#[cfg(not(feature = "terminal-trace"))]
pub fn log_terminal_trace_note(_session_id: &str, _provider: &str, _note: &str) {}

#[cfg(not(feature = "terminal-trace"))]
pub fn log_terminal_trace_bytes(
    _session_id: &str,
    _provider: &str,
    _direction: &str,
    _bytes: &[u8],
) {
}

#[cfg(feature = "terminal-trace")]
mod terminal_trace {
    use crate::utils::fs::get_wardian_home;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;

    fn trace_dir() -> Option<PathBuf> {
        get_wardian_home().map(|home| home.join("debug").join("terminal-traces"))
    }

    fn safe_trace_name(session_id: &str) -> String {
        let sanitized: String = session_id
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => ch,
                _ => '_',
            })
            .collect();
        if sanitized.is_empty() {
            "unknown-session".to_string()
        } else {
            sanitized
        }
    }

    pub fn terminal_trace_path(session_id: &str) -> Option<PathBuf> {
        trace_dir().map(|dir| dir.join(format!("{}.log", safe_trace_name(session_id))))
    }

    fn trace_timestamp() -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn contains_sequence(bytes: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty() && bytes.windows(needle.len()).any(|window| window == needle)
    }

    fn trace_flags(bytes: &[u8]) -> String {
        let mut flags = Vec::new();
        if contains_sequence(bytes, b"\x1b[3J") {
            flags.push("ED3");
        }
        if contains_sequence(bytes, b"\x1b[2J") {
            flags.push("ED2");
        }
        if contains_sequence(bytes, b"\x1b[?1049h") {
            flags.push("ALT_ENTER");
        }
        if contains_sequence(bytes, b"\x1b[?1049l") {
            flags.push("ALT_EXIT");
        }
        if contains_sequence(bytes, b"\x1b[?1000h")
            || contains_sequence(bytes, b"\x1b[?1002h")
            || contains_sequence(bytes, b"\x1b[?1003h")
            || contains_sequence(bytes, b"\x1b[?1006h")
        {
            flags.push("MOUSE_ON");
        }
        if contains_sequence(bytes, b"\x1b[?1000l")
            || contains_sequence(bytes, b"\x1b[?1002l")
            || contains_sequence(bytes, b"\x1b[?1003l")
            || contains_sequence(bytes, b"\x1b[?1006l")
        {
            flags.push("MOUSE_OFF");
        }
        if flags.is_empty() {
            "-".to_string()
        } else {
            flags.join(",")
        }
    }

    fn escape_bytes_preview(bytes: &[u8], limit: usize) -> String {
        let mut out = String::new();
        for &byte in bytes.iter().take(limit) {
            match byte {
                b'\n' => out.push_str("\\n"),
                b'\r' => out.push_str("\\r"),
                b'\t' => out.push_str("\\t"),
                0x1b => out.push_str("\\x1b"),
                0x20..=0x7e => out.push(byte as char),
                _ => out.push_str(&format!("\\x{:02x}", byte)),
            }
        }
        if bytes.len() > limit {
            out.push_str("...");
        }
        out
    }

    fn append_trace_line(session_id: &str, line: &str) {
        let Some(path) = terminal_trace_path(session_id) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{}", line);
        }
    }

    pub fn log_terminal_trace_note(session_id: &str, provider: &str, note: &str) {
        if provider != "codex" {
            return;
        }
        append_trace_line(
            session_id,
            &format!("[{}] NOTE {}", trace_timestamp(), note),
        );
    }

    pub fn log_terminal_trace_bytes(
        session_id: &str,
        provider: &str,
        direction: &str,
        bytes: &[u8],
    ) {
        if provider != "codex" || bytes.is_empty() {
            return;
        }
        let flags = trace_flags(bytes);
        let preview = escape_bytes_preview(bytes, 512);
        append_trace_line(
            session_id,
            &format!(
                "[{}] {} len={} flags={} {}",
                trace_timestamp(),
                direction,
                bytes.len(),
                flags,
                preview
            ),
        );
    }

    #[cfg(test)]
    mod tests {
        use super::{escape_bytes_preview, trace_flags};

        #[test]
        fn trace_flags_detects_key_terminal_sequences() {
            let bytes = b"\x1b[?1049hhello\x1b[3J\x1b[?1006h";
            assert_eq!(trace_flags(bytes), "ED3,ALT_ENTER,MOUSE_ON");
        }

        #[test]
        fn escape_bytes_preview_escapes_control_bytes() {
            let bytes = b"a\r\n\t\x1b\x00";
            assert_eq!(escape_bytes_preview(bytes, 32), "a\\r\\n\\t\\x1b\\x00");
        }
    }
}

#[cfg(feature = "terminal-trace")]
pub use terminal_trace::{log_terminal_trace_bytes, log_terminal_trace_note};
