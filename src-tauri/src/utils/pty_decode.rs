#[derive(Default)]
pub struct PtyUtf8Decoder {
    pending: Vec<u8>,
}

impl PtyUtf8Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decode_chunk(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();

        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    output.push_str(valid);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        output.push_str(
                            std::str::from_utf8(&self.pending[..valid_up_to])
                                .expect("valid prefix reported by utf8 decoder"),
                        );
                        self.pending.drain(..valid_up_to);
                        continue;
                    }

                    if let Some(invalid_len) = error.error_len() {
                        output.push('\u{fffd}');
                        self.pending.drain(..invalid_len);
                        continue;
                    }

                    break;
                }
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::PtyUtf8Decoder;

    #[test]
    fn preserves_utf8_codepoints_split_across_pty_reads() {
        let glyph = "▐";
        let bytes = glyph.as_bytes();
        let mut decoder = PtyUtf8Decoder::new();

        assert_eq!(decoder.decode_chunk(&bytes[..1]), "");
        assert_eq!(decoder.decode_chunk(&bytes[1..]), glyph);
    }

    #[test]
    fn preserves_mixed_ascii_and_split_utf8_across_pty_reads() {
        let frame = "A▐B";
        let bytes = frame.as_bytes();
        let mut decoder = PtyUtf8Decoder::new();

        assert_eq!(decoder.decode_chunk(&bytes[..2]), "A");
        assert_eq!(decoder.decode_chunk(&bytes[2..]), "▐B");
    }
}
