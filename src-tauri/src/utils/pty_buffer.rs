pub const PTY_OUTPUT_BUFFER_MAX_BYTES: usize = 1_048_576;

pub fn append_bounded_pty_output(buffer: &mut String, chunk: &str) {
    append_bounded_text(buffer, chunk, PTY_OUTPUT_BUFFER_MAX_BYTES);
}

fn append_bounded_text(buffer: &mut String, chunk: &str, max_bytes: usize) {
    if max_bytes == 0 {
        buffer.clear();
        return;
    }

    buffer.push_str(chunk);
    if buffer.len() <= max_bytes {
        return;
    }

    let mut split_at = buffer.len() - max_bytes;
    while split_at < buffer.len() && !buffer.is_char_boundary(split_at) {
        split_at += 1;
    }
    buffer.drain(..split_at);
}

#[cfg(test)]
mod tests {
    use super::append_bounded_text;

    #[test]
    fn append_bounded_text_keeps_recent_output() {
        let mut buffer = String::from("12345");

        append_bounded_text(&mut buffer, "67890", 6);

        assert_eq!(buffer, "567890");
    }

    #[test]
    fn append_bounded_text_keeps_valid_utf8() {
        let mut buffer = String::from("abc");

        append_bounded_text(&mut buffer, "🙂def", 7);

        assert_eq!(buffer, "🙂def");
    }

    #[test]
    fn append_bounded_text_clears_when_limit_is_zero() {
        let mut buffer = String::from("abc");

        append_bounded_text(&mut buffer, "def", 0);

        assert!(buffer.is_empty());
    }
}
