//! Answer the native console's initial queries before a renderer is available.

use std::borrow::Cow;

const STARTUP_SEQUENCES: [&[u8]; 3] = [
    b"\x1b[6n\x1b[c",
    b"\x1b[1t\x1b[6n\x1b[c",
    b"\x1b[2t\x1b[6n\x1b[c",
];
const STARTUP_REPLY: &[u8] = b"\x1b[1;1R\x1b[?1;2c";

#[derive(Default)]
pub(super) struct ConptyStartupProbeResponder {
    pending: Vec<u8>,
    finished: bool,
}

impl ConptyStartupProbeResponder {
    /// Recognizes only the opening ConPTY cursor/DA1 handshake, across reads.
    ///
    /// Bundled ConPTY 1.24 waits up to three seconds for DA1 before starting the
    /// child. A fresh embedded PTY starts at cell 1,1 and supports VT100 advanced
    /// video; neither answer needs a mounted frontend or a keyboard lease.
    /// Once startup is answered or other output appears, later provider queries
    /// are left to their existing handlers. The retained prefix is at most 11
    /// bytes, independent of how much application output follows it.
    pub(super) fn process_chunk<'a>(
        &mut self,
        chunk: &'a [u8],
    ) -> (Option<&'static [u8]>, Cow<'a, [u8]>) {
        if self.finished {
            return (None, Cow::Borrowed(chunk));
        }
        for (index, byte) in chunk.iter().enumerate() {
            self.pending.push(*byte);
            if STARTUP_SEQUENCES.contains(&self.pending.as_slice()) {
                self.finished = true;
                // Retain the optional window command, but consume both queries:
                // publishing them would allow a mounted renderer to reply again.
                let window_len = self.pending.len() - STARTUP_SEQUENCES[0].len();
                self.pending.truncate(window_len);
                self.pending.extend_from_slice(&chunk[index + 1..]);
                return (
                    Some(STARTUP_REPLY),
                    Cow::Owned(std::mem::take(&mut self.pending)),
                );
            }
            if !STARTUP_SEQUENCES
                .iter()
                .any(|sequence| sequence.starts_with(&self.pending))
            {
                self.finished = true;
                self.pending.extend_from_slice(&chunk[index + 1..]);
                return (None, Cow::Owned(std::mem::take(&mut self.pending)));
            }
        }
        (None, Cow::Borrowed(&[]))
    }

    /// Release an incomplete opening prefix if the PTY closes before retirement.
    pub(super) fn finish(&mut self) -> Vec<u8> {
        self.finished = true;
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::{ConptyStartupProbeResponder, STARTUP_REPLY, STARTUP_SEQUENCES};

    #[test]
    fn startup_queries_are_consumed_at_every_read_boundary() {
        for sequence in STARTUP_SEQUENCES {
            for split in 0..=sequence.len() {
                let mut responder = ConptyStartupProbeResponder::default();
                let (first_reply, first_output) = responder.process_chunk(&sequence[..split]);
                let mut remainder = sequence[split..].to_vec();
                remainder.extend_from_slice(b"provider output");
                let (second_reply, second_output) = responder.process_chunk(&remainder);
                let replies: Vec<_> = [first_reply, second_reply].into_iter().flatten().collect();
                assert_eq!(replies, vec![STARTUP_REPLY]);
                let mut output = first_output.into_owned();
                output.extend_from_slice(&second_output);
                let mut expected = sequence[..sequence.len() - STARTUP_SEQUENCES[0].len()].to_vec();
                expected.extend_from_slice(b"provider output");
                assert_eq!(output, expected);
                assert_eq!(responder.process_chunk(sequence), (None, sequence.into()));
                assert!(responder.finish().is_empty());
            }
        }
    }

    #[test]
    fn single_byte_reads_never_publish_partial_queries() {
        let sequence = STARTUP_SEQUENCES[0];
        let mut responder = ConptyStartupProbeResponder::default();
        for byte in &sequence[..sequence.len() - 1] {
            let chunk = [*byte];
            let (reply, output) = responder.process_chunk(&chunk);
            assert!(reply.is_none());
            assert!(output.is_empty());
        }
        let (reply, output) = responder.process_chunk(&sequence[sequence.len() - 1..]);
        assert_eq!(reply, Some(STARTUP_REPLY));
        assert!(output.is_empty());
    }

    #[test]
    fn incomplete_queries_are_released_on_eof() {
        for sequence in STARTUP_SEQUENCES {
            for end in 0..sequence.len() {
                let mut responder = ConptyStartupProbeResponder::default();
                let (reply, output) = responder.process_chunk(&sequence[..end]);
                assert!(reply.is_none());
                assert!(output.is_empty());
                assert_eq!(responder.finish(), sequence[..end]);
                assert!(responder.finish().is_empty());
            }
        }
    }

    #[test]
    fn nonmatching_output_is_released_losslessly_at_every_read_boundary() {
        for initial in [
            b"provider output".as_slice(),
            b"\x1b]11;?\x07",
            b"\x1b[3t",
            b"\x1b[6n\x1b[?1004h",
            b"\x1b[c",
        ] {
            for split in 0..=initial.len() {
                let mut responder = ConptyStartupProbeResponder::default();
                let (first_reply, first_output) = responder.process_chunk(&initial[..split]);
                let (second_reply, second_output) = responder.process_chunk(&initial[split..]);
                assert!(first_reply.is_none());
                assert!(second_reply.is_none());
                let mut output = first_output.into_owned();
                output.extend_from_slice(&second_output);
                output.extend(responder.finish());
                assert_eq!(output, initial);
                assert_eq!(
                    responder.process_chunk(STARTUP_SEQUENCES[0]),
                    (None, STARTUP_SEQUENCES[0].into())
                );
            }
        }
    }

    #[test]
    fn empty_reads_preserve_the_pending_prefix() {
        let mut responder = ConptyStartupProbeResponder::default();
        assert!(responder.process_chunk(b"\x1b[").1.is_empty());
        assert!(responder.process_chunk(b"").1.is_empty());
        assert_eq!(responder.process_chunk(b"6n\x1b[c").0, Some(STARTUP_REPLY));
    }

    #[test]
    fn later_queries_in_the_same_read_remain_unchanged() {
        let mut responder = ConptyStartupProbeResponder::default();
        let mut output = STARTUP_SEQUENCES[0].to_vec();
        output.extend_from_slice(STARTUP_SEQUENCES[0]);
        let (reply, forwarded) = responder.process_chunk(&output);
        assert_eq!(reply, Some(STARTUP_REPLY));
        assert_eq!(forwarded.as_ref(), STARTUP_SEQUENCES[0]);
    }

    #[test]
    fn a_new_runtime_gets_its_own_reply() {
        for _ in 0..2 {
            let mut responder = ConptyStartupProbeResponder::default();
            assert_eq!(
                responder.process_chunk(STARTUP_SEQUENCES[0]).0,
                Some(STARTUP_REPLY)
            );
        }
    }

    #[test]
    fn retired_responder_does_not_retain_application_output() {
        let mut responder = ConptyStartupProbeResponder::default();
        let output = [b'x'; 65536];
        assert_eq!(
            responder.process_chunk(&output),
            (None, output.as_slice().into())
        );
        assert!(responder.pending.is_empty());
        assert_eq!(
            responder.process_chunk(&output),
            (None, output.as_slice().into())
        );
    }
}
