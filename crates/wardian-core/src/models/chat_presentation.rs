//! Additive wire contract for producer-owned Chat turns.
//!
//! This module contains no transcript store or provider identity inference.
//! The archive owner supplies qualified membership and evidence. Unsupported
//! turns retain the legacy representation. Pending/provisional publication
//! additionally requires its exact identity bridge and retirement fencing.

use serde::{Deserialize, Serialize};

pub const CHAT_PRESENTATION_VERSION: u32 = 1;
pub const CHAT_PRESENTATION_MAX_ITEMS: usize = 80;
pub const CHAT_PRESENTATION_MAX_BYTES: usize = 256 * 1024;
pub const CHAT_STEP_OUTPUT_BYTES: usize = 16 * 1024;
pub const CHAT_STEP_INPUT_BYTES: usize = 1024;

/// The payload stays opaque until the version is recognized. A future payload
/// must not prevent an older client from reading the legacy event envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatPresentation {
    pub version: u32,
    pub payload: serde_json::Value,
}

/// One bounded revision of the existing projection, not another transcript.
/// Scope, revision, aliases, removals and cursors belong to the enclosing page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPresentationV1 {
    pub turns: Vec<ChatPresentedTurn>,
    pub items: Vec<ChatPresentedItem>,
    /// Canonical state evidence updates the turn without becoming narration.
    #[serde(default)]
    pub state_event_refs: Vec<ChatStateEventRef>,
    #[serde(default)]
    pub legacy_turns: Vec<ChatLegacyTurn>,
    pub detail: Option<ChatStepDetail>,
    /// Exact recent-owner continuity; grants no canonical detail, Older or ACK authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_bridge: Option<ChatPreviewBridge>,
}

/// The producer checked this immutable Head against the same current admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPreviewBridge {
    pub admission_id: String,
    pub canonical_head: String,
    /// The prior generation this same-source snapshot can replace atomically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<ChatPreviewBridgeReplacement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPreviewBridgeReplacement {
    pub admission_id: String,
    pub generation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatStateEventRef {
    pub event_id: String,
    pub turn_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatLegacyTurnKind {
    Unmigrated,
    EarlierActivity,
}

/// Corrected Work log rows from the same canonical view. A turn appears in
/// exactly one presentation; unproven membership never invents a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatLegacyTurn {
    pub turn_key: String,
    pub order_key: String,
    pub event_refs: Vec<String>,
    pub kind: ChatLegacyTurnKind,
    pub membership_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatTurnStatus {
    Running,
    Completed,
    ActionRequired,
    Failed,
    Cancelled,
    Incomplete,
    Unknown,
}

/// Counts cover the producer's turn, including items outside this page.
/// Prompt/answer references are the bounded references in this page; their
/// completeness flags distinguish them from a fully loaded turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPresentedTurn {
    pub turn_key: String,
    pub order_key: String,
    pub membership_complete: bool,
    /// Every admitted live-prefix item has proven membership; termination remains unknown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prefix_attributed: bool,
    pub status: ChatTurnStatus,
    pub prompt_item_keys: Vec<String>,
    pub answer_item_keys: Vec<String>,
    pub references_complete: bool,
    pub duration_ms: Option<u64>,
    pub reported_changes: Option<ChatReportedChanges>,
    pub step_count: u64,
    pub answer_recorded: bool,
    pub failed_step_count: u64,
    pub counts_complete: bool,
}

/// Agent-reported evidence only; this never asserts Git verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatReportedChanges {
    pub file_count: u64,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatPresentedItemKind {
    Prompt,
    Step,
    Narration,
    Answer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatItemProvenance {
    Pending,
    Provisional,
    Canonical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatSubmissionStatus {
    Queued,
    Delivered,
    Failed,
    Cancelled,
    Uncertain,
}

/// Qualified backing identity, separate from a visible item key. Native
/// correspondence is supplied by the acquisition owner, never display text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvidenceIdentity {
    Source {
        source_key: String,
        source_epoch: String,
        record_key: String,
    },
    Submission {
        submission_id: String,
    },
}

/// One invocation and its results share a step key. The projection retains
/// full evidence separately; these references cover only the bounded page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPresentedItem {
    pub item_key: String,
    pub turn_key: String,
    pub order_key: String,
    pub kind: ChatPresentedItemKind,
    pub provenance: ChatItemProvenance,
    pub evidence: ChatEvidenceIdentity,
    pub evidence_version: String,
    pub admission_id: Option<String>,
    pub submission_status: Option<ChatSubmissionStatus>,
    pub event_refs: Vec<String>,
    pub preview: Option<ChatItemPreview>,
    pub step: Option<ChatPresentedStep>,
}

/// Bounded admitted content before canonical references exist. Attachment
/// references retain their existing owner, authorization and lifetime rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatItemPreview {
    pub text: String,
    pub attachment_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatToolCategory {
    Shell,
    Read,
    Browser,
    Edit,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatStepStatus {
    Running,
    Completed,
    ActionRequired,
    Failed,
    Cancelled,
    Incomplete,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatDetailAvailability {
    Pending,
    Available,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPresentedStep {
    pub tool_name: String,
    pub category: ChatToolCategory,
    pub description: String,
    pub status: ChatStepStatus,
    pub summary: Option<String>,
    pub invocation_ref: Option<String>,
    pub result_ref: Option<String>,
    pub approval_ref: Option<String>,
    pub detail_availability: ChatDetailAvailability,
    pub detail_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatOutputFormat {
    Text,
    Json,
}

/// A single retained tail. There is deliberately no continuation to concatenate
/// in browser state. Existing inspection surfaces own full output/export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatStepDetail {
    pub step_key: String,
    pub revision: String,
    pub text: String,
    pub input_text: Option<String>,
    #[serde(default)]
    pub input_truncated: bool,
    pub result_summary: Option<String>,
    pub format: ChatOutputFormat,
    pub pending: bool,
    pub unavailable: bool,
    pub complete: bool,
    pub truncated_before: bool,
    pub total_bytes: Option<u64>,
}
