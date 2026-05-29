use crate::blueprint::Blueprint;
use crate::error::{Result, WorkflowError};
use std::path::Path;

const FENCE: &str = "---";

/// Parse a blueprint from a `.md` file on disk.
pub fn parse_file(path: &Path) -> Result<Blueprint> {
    let text = std::fs::read_to_string(path)?;
    parse_str(&text)
}

/// Parse a blueprint from in-memory `.md` text.
///
/// The file must start with a `---` fence, contain a YAML front-matter block,
/// a closing `---` fence, and then an optional markdown body.
pub fn parse_str(text: &str) -> Result<Blueprint> {
    let trimmed = text.strip_prefix('\u{feff}').unwrap_or(text);
    let after_open = trimmed
        .strip_prefix(FENCE)
        .and_then(|rest| {
            rest.strip_prefix('\n')
                .or_else(|| rest.strip_prefix("\r\n"))
        })
        .ok_or(WorkflowError::MissingFrontMatter)?;

    let (yaml, body) = split_front_matter(after_open).ok_or(WorkflowError::MissingFrontMatter)?;

    let mut blueprint: Blueprint =
        serde_norway::from_str(yaml).map_err(|e| WorkflowError::Yaml(e.to_string()))?;
    blueprint.body = body.trim_start_matches(['\n', '\r']).to_string();
    Ok(blueprint)
}

/// Split the text following the opening fence into (yaml, body) using the first
/// line that is exactly `---`.
fn split_front_matter(after_open: &str) -> Option<(&str, &str)> {
    let mut offset = 0usize;
    for line in after_open.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if bare == FENCE {
            let yaml = &after_open[..offset];
            let body = &after_open[offset + line.len()..];
            return Some((yaml, body));
        }
        offset += line.len();
    }
    None
}

/// Serialize a blueprint back to `.md` text: front-matter fences around the YAML
/// graph, then the markdown body.
pub fn to_string(blueprint: &Blueprint) -> Result<String> {
    let yaml =
        serde_norway::to_string(blueprint).map_err(|e| WorkflowError::Serialize(e.to_string()))?;
    let mut out = String::new();
    out.push_str(FENCE);
    out.push('\n');
    out.push_str(&yaml);
    if !yaml.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(FENCE);
    out.push('\n');
    if !blueprint.body.is_empty() {
        out.push('\n');
        out.push_str(&blueprint.body);
        if !blueprint.body.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = include_str!("../tests/fixtures/minimal.md");

    #[test]
    fn parses_front_matter_and_body() {
        let bp = parse_str(MINIMAL).unwrap();
        assert_eq!(bp.id, "minimal");
        assert_eq!(bp.nodes.len(), 2);
        assert_eq!(bp.edges.len(), 1);
        assert!(bp.body.contains("# Minimal"));
        assert!(bp.body.contains("used in tests"));
    }

    #[test]
    fn missing_front_matter_is_an_error() {
        let err = parse_str("no front matter here").unwrap_err();
        assert!(matches!(err, crate::WorkflowError::MissingFrontMatter));
    }

    #[test]
    fn round_trip_preserves_graph_and_body() {
        let bp = parse_str(MINIMAL).unwrap();
        let text = to_string(&bp).unwrap();
        let again = parse_str(&text).unwrap();
        assert_eq!(bp, again);
    }
}
