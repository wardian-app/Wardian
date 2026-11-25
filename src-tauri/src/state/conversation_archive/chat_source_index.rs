//! Independently framed source observations, never a canonical archive seal.
//! The existing background capture owner publishes recent rows before walking
//! older intervals. A restart resumes the last immutable checkpoint.
#[cfg(test)]
use std::fs::File;
#[cfg(test)]
use std::io::{self, Read};
#[cfg(test)]
use std::path::Path;

#[cfg(test)]
use serde::{Deserialize, Serialize};
#[cfg(test)]
use wardian_core::conversations::write_json_atomic;
#[cfg(test)]
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind};

#[cfg(test)]
use super::chat_read::{locations, PAGE_ROWS};
#[cfg(test)]
use super::chat_read_store::{Dependency, Store};
#[cfg(test)]
use super::storage::valid_ref;
#[cfg(test)]
use crate::commands::chat::{
    conversation_archive_context_from_snapshot, AgentArchiveCaptureSnapshot,
};
#[cfg(test)]
use crate::commands::chat_recent_seed::{source_scope, SeedCheckpoint};

fn source_id_parts(agent_id: &str, source_id: &str) -> Option<(String, String, u64)> {
    if source_id.len() > 512 {
        return None;
    }
    let suffix = source_id.strip_prefix(&format!("source:{agent_id}:"))?;
    let mut parts = suffix.split(':');
    let epoch = parts.next()?;
    let offset = parts.next()?;
    let raw_digest = parts.next()?;
    let ordinal = parts.next()?;
    if parts.next().is_some()
        || epoch.len() != 64
        || !epoch
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || raw_digest.len() != 64
        || !raw_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let offset_value = offset.parse::<u64>().ok()?;
    let ordinal_value = ordinal.parse::<u64>().ok()?;
    if offset_value.to_string() != offset || ordinal_value.to_string() != ordinal {
        return None;
    }
    Some((
        epoch.to_owned(),
        format!("{offset_value:020}:{ordinal_value:020}"),
        offset_value,
    ))
}

pub(super) fn valid_source_id(agent_id: &str, source_id: &str) -> bool {
    source_id_parts(agent_id, source_id).is_some()
}

#[cfg(test)]
include!("chat_source_index_legacy_fixture.inc.rs");
