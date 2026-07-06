/// Split a markdown document into (frontmatter, body). The frontmatter is
/// `None` when absent or unparseable; the body always excludes the block.
pub fn parse_frontmatter(content: &str) -> (Option<serde_norway::Value>, &str) {
    let rest = match content.strip_prefix("---") {
        Some(rest) => rest,
        None => return (None, content),
    };
    let rest = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n"));
    let Some(rest) = rest else { return (None, content) };
    let Some(end) = rest.find("\n---") else { return (None, content) };
    let yaml_text = &rest[..end];
    let after = &rest[end + 4..];
    let body = after.trim_start_matches(['\r', '\n']);
    match serde_norway::from_str::<serde_norway::Value>(yaml_text) {
        Ok(value) => (Some(value), body),
        Err(_) => (None, body),
    }
}

/// Human description for list rows: frontmatter `description`, else the
/// first non-empty body line without markdown heading markers.
pub fn extract_description(content: &str) -> String {
    let (frontmatter, body) = parse_frontmatter(content);
    if let Some(description) = frontmatter
        .as_ref()
        .and_then(|value| value.get("description"))
        .and_then(|value| value.as_str())
    {
        let trimmed = description.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    body.lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_from_frontmatter() {
        let content = "---\nname: planner\ndescription: Plans work\n---\n# Planner\nBody";
        assert_eq!(extract_description(content), "Plans work");
    }

    #[test]
    fn description_falls_back_to_first_body_line() {
        assert_eq!(extract_description("# Planner skill\nBody"), "Planner skill");
        assert_eq!(extract_description("---\nname: x\n---\n\n## Heading\n"), "Heading");
    }

    #[test]
    fn malformed_frontmatter_never_panics() {
        let content = "---\n: : bad yaml [\n---\nFallback line";
        let (fm, body) = parse_frontmatter(content);
        assert!(fm.is_none());
        assert!(body.contains("Fallback line"));
        assert_eq!(extract_description(content), "Fallback line");
    }

    #[test]
    fn no_frontmatter_returns_whole_body() {
        let (fm, body) = parse_frontmatter("just text");
        assert!(fm.is_none());
        assert_eq!(body, "just text");
    }
}
