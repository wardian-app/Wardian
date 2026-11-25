//! Bounded, provisional observations from the independently owned recent source.
//! No archive cursor, complete-history or speculative alias claim is made here.
#[cfg(test)]
use std::fs::File;
#[cfg(test)]
use std::io::{self, Read, Seek, SeekFrom};

#[cfg(test)]
use serde_json::json;
use sha2::{Digest, Sha256};
#[cfg(test)]
use wardian_core::models::chat::{AgentChatDetail, AgentChatEvent};

#[cfg(test)]
use super::chat::AgentArchiveCaptureSnapshot;
#[cfg(test)]
use super::provider_log_acquisition::native_file_identity;
use super::provider_log_acquisition::ProviderLogNativeIdentity;

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The exact native file identity, framed byte position, row digest and adapter
/// output ordinal prove a shared observation. Equal text never proves a bridge.
pub(crate) fn source_reference(
    agent_id: &str,
    identity: &ProviderLogNativeIdentity,
    offset: u64,
    raw: &str,
    ordinal: usize,
) -> String {
    format!(
        "source:{agent_id}:{}:{offset}:{}:{ordinal}",
        hash(&serde_json::to_vec(identity).unwrap_or_default()),
        hash(raw.as_bytes())
    )
}

#[cfg(test)]
pub(crate) use test_support::read;

#[cfg(test)]
mod test_support {
    use super::*;

    pub(crate) fn read(
        snapshot: &AgentArchiveCaptureSnapshot,
        previous: Option<&str>,
    ) -> io::Result<Seed> {
        read_window(snapshot, previous, None)
    }
}

#[cfg(test)]
include!("chat_recent_seed_legacy_fixture.inc.rs");
