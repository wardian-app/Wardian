//! Bounded provider-log reads and identity-bound optional Watch sources.

/// Provider logs are re-parsed whenever they change and grow to hundreds of
/// megabytes for long-lived codex sessions. Status is derived from the most
/// recent lines, so parsing is capped to this tail; files under the cap are
/// read whole (gemini legacy logs are a single JSON document and stay intact).
/// For capped files the query count and last-query timestamp are derived from
/// the retained tail, while the init timestamp falls back to persisted Born
/// time.
pub(super) const LOG_PARSE_TAIL_BYTES: u64 = 4 * 1024 * 1024;

fn read_log_bounded(path: &std::path::Path) -> std::io::Result<String> {
    read_open_log_bounded(std::fs::File::open(path)?)
}

pub(super) fn read_log_with_watch_source(
    path: &std::path::Path,
    provider: &str,
) -> std::io::Result<(
    String,
    Option<crate::commands::chat_recent_database::Source>,
)> {
    if provider != "gemini" {
        return read_log_bounded(path).map(|content| (content, None));
    }
    match crate::commands::chat_recent_database::Source::watch_reader(path) {
        Ok((source, file)) => {
            let content = read_open_log_bounded(file)?;
            let source = source.valid().unwrap_or(false).then_some(source);
            Ok((content, source))
        }
        // This optional observation never disables the existing normal parser.
        Err(_) => read_log_bounded(path).map(|content| (content, None)),
    }
}

fn read_open_log_bounded(mut file: std::fs::File) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let len = file.metadata()?.len();
    if len <= LOG_PARSE_TAIL_BYTES {
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        return Ok(content);
    }
    file.seek(SeekFrom::Start(len - LOG_PARSE_TAIL_BYTES))?;
    let mut bytes = Vec::with_capacity(LOG_PARSE_TAIL_BYTES as usize);
    file.read_to_end(&mut bytes)?;
    let content = String::from_utf8_lossy(&bytes);
    // Drop the first (possibly partial) line so parsing starts on a boundary.
    Ok(content
        .split_once('\n')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default())
}

pub(super) fn read_log_suffix(path: &std::path::Path, max_bytes: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut bytes)?;
    if start > 0 {
        let Some(first_line_end) = bytes.iter().position(|byte| *byte == b'\n') else {
            return Ok(String::new());
        };
        bytes.drain(..=first_line_end);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
