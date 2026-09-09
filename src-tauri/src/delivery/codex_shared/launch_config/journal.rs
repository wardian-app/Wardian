//! Owned write-ahead intent. Never stores the complete Codex config or MCP data.
use super::{failure, leaves, storage, CodexSharedError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

pub(super) const FILE: &str = ".wardian-launch-config.json";
const OWNER: &str = "wardian_codex_interactive_launch";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    schema_version: u32,
    owner: String,
    home_identity: (u64, u64),
    pub(super) token: String,
    pub(super) changes: Vec<leaves::Change>,
}

impl Journal {
    pub(super) fn new(home: &Path, changes: Vec<leaves::Change>) -> Result<Self, CodexSharedError> {
        Ok(Self {
            schema_version: 1,
            owner: OWNER.into(),
            home_identity: storage::home_identity(home)?,
            token: uuid::Uuid::new_v4().to_string(),
            changes,
        })
    }

    pub(super) fn decode(text: &str, home: &Path) -> Result<Self, CodexSharedError> {
        let journal: Self =
            serde_json::from_str(text).map_err(|_| failure("malformed Codex launch journal"))?;
        if journal.schema_version != 1
            || journal.owner != OWNER
            || journal.home_identity != storage::home_identity(home)?
        {
            return Err(failure("foreign Codex launch journal"));
        }
        let token = uuid::Uuid::parse_str(&journal.token)
            .map_err(|_| failure("invalid Codex launch journal token"))?;
        if token.get_version_num() != 4 || token.to_string() != journal.token {
            return Err(failure("invalid Codex launch journal token"));
        }
        let mut paths = BTreeSet::new();
        for change in &journal.changes {
            leaves::validate(&change.path, &change.applied)?;
            if let Some(before) = &change.before {
                leaves::validate(&change.path, before)?;
            }
            if !paths.insert(&change.path) || change.before.as_ref() == Some(&change.applied) {
                return Err(failure("duplicate or unchanged Codex launch journal leaf"));
            }
        }
        Ok(journal)
    }

    pub(super) fn encode(&self, home: &Path) -> Result<String, CodexSharedError> {
        let text = serde_json::to_string(self)
            .map_err(|_| failure("cannot encode Codex launch journal"))?;
        Self::decode(&text, home)?; // Validate complete serialized output before publication.
        Ok(text)
    }
}
