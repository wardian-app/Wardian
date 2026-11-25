//! Derived display relations. Physical rows, source coordinates and capture
//! cursors are never changed by this index. Every update touches a bounded
//! component; readers use certificates materialized in published row objects.
#[cfg(test)]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::io;

use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind};

#[cfg(test)]
use super::chat_read_store::{digest, valid_ref, Store};
#[cfg(test)]
use super::ConversationArchiveContext;
#[cfg(test)]
use crate::providers::chat_transcript::{
    codex_display_evidence, codex_display_evidence_from_bounded_header, CodexDisplaySide,
};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Index {
    scope: String,
    buckets: Option<String>,
    members: Option<String>,
    pending: Option<String>,
    claims: Option<String>,
    before: Option<String>,
    pub(super) legacy_ready: bool,
    pub(super) complete_prefix: bool,
    admission: String,
    #[serde(default)]
    narrative_deferred: bool,
    #[serde(default)]
    narrative_watermark: Option<u64>,
    #[serde(default)]
    narrative_queue: Option<String>,
    #[serde(default)]
    narrative_before: Option<String>,
    #[serde(default)]
    sequence_ready: bool,
}

#[cfg(test)]
#[path = "chat_logical_index_tests.rs"]
mod tests;

#[cfg(test)]
include!("chat_logical_index_legacy_fixture.inc.rs");
