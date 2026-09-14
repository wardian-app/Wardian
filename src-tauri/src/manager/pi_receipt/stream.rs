//! Bounded provider records and exact-content matching, independent of file flush.
use serde::Deserialize;

pub(super) const MAX_STREAM_BYTES: u64 = 262_144;
pub(super) const MAX_RECORD_BYTES: usize = 2048;

#[derive(Deserialize)]
pub(super) struct Record {
    pub v: u8,
    pub launch: String,
    pub stream: String,
    pub seq: u64,
    pub native_session_id: String,
    pub kind: String,
    pub text_sha256: Option<String>,
    pub text_bytes: Option<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct UserStart {
    pub offset: u64,
    pub text_sha256: Option<String>,
    pub text_bytes: Option<usize>,
}

pub(super) struct Pending {
    pub id: u64,
    pub offset: u64,
    pub digest: String,
    pub bytes: usize,
    pub accepted: bool,
}

#[derive(Default)]
pub(super) struct Stream {
    pub offset: u64,
    pub ready: bool,
    pub failure: Option<String>,
    pub pending: Option<Pending>,
    pub next_pending: u64,
    stream: Option<String>,
    sequence: u64,
    active: bool,
}

impl Stream {
    pub fn accept_record(
        &mut self,
        line: &[u8],
        start: u64,
        nonce: &str,
        native: &str,
    ) -> Result<Option<UserStart>, String> {
        let record: Record = serde_json::from_slice(line).map_err(|_| "Invalid Pi receipt JSON")?;
        if record.v != 1 || record.launch != nonce || record.native_session_id != native {
            return Err("Pi receipt identity/version mismatch".into());
        }
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| stream != &record.stream)
        {
            return Err("Pi receipt extension reloaded; restart required".into());
        }
        if record.seq <= self.sequence {
            return Ok(None);
        }
        if record.seq != self.sequence + 1 {
            return Err("Pi receipt sequence gap".into());
        }
        self.sequence = record.seq;
        match record.kind.as_str() {
            "ready" if !self.ready && record.seq == 1 => {
                self.stream = Some(record.stream);
                self.ready = true;
            }
            "loop_start" if self.ready => self.active = true,
            "loop_end" if self.ready => self.active = false,
            "user_start" if self.ready && self.active => {
                if record.text_sha256.as_ref().is_some_and(|hash| {
                    hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit())
                }) {
                    return Err("Invalid Pi content digest".into());
                }
                return Ok(Some(UserStart {
                    offset: start,
                    text_sha256: record.text_sha256,
                    text_bytes: record.text_bytes,
                }));
            }
            _ => return Err("Pi receipt stream invalidated or event out of order".into()),
        }
        Ok(None)
    }

    pub fn published(&mut self, event: &UserStart) {
        if let Some(pending) = self.pending.as_mut() {
            if event.offset >= pending.offset
                && event.text_sha256.as_ref() == Some(&pending.digest)
                && event.text_bytes == Some(pending.bytes)
            {
                pending.accepted = true;
            }
        }
    }
}
