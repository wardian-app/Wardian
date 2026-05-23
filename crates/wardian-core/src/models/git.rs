use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileEntry {
    pub path: String,
    /// Status code: "M", "A", "D", "R", "C", "U", "?"
    pub status: String,
    pub is_staged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatusResult {
    pub branch: String,
    pub upstream: Option<String>,
    pub files: Vec<GitFileEntry>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLogEntry {
    pub hash: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
    pub message: String,
    pub author: String,
    pub author_email: String,
    pub date: String,
}
