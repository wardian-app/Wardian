/// Split a markdown document into (frontmatter, body). The frontmatter is
/// `None` when absent or unparseable; the body always excludes the block.
pub fn parse_frontmatter(content: &str) -> (Option<serde_norway::Value>, &str) {
    let rest = match content.strip_prefix("---") {
        Some(rest) => rest,
        None => return (None, content),
    };
    let rest = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n"));
    let Some(rest) = rest else { return (None, content) };
    if let Some(after_close) = rest.strip_prefix("---") {
        // Empty frontmatter block: the closing fence immediately follows the
        // opening fence's line break, e.g. "---\n---\nBody". `rest.find("\n---")`
        // below can never match this shape because the leading '\n' was already
        // consumed above, so it must be handled explicitly rather than falling
        // through to "no frontmatter".
        let body = after_close.trim_start_matches(['\r', '\n']);
        return (None, body);
    }
    let Some(end) = rest.find("\n---") else { return (None, content) };
    let yaml_text = &rest[..end];
    // A CRLF document's closing fence is found via "\n---", which leaves a
    // dangling '\r' on the last frontmatter line (the fence line's own CR).
    // Strip it deliberately; inner CRLF line breaks within the YAML block are
    // untouched since they don't border the fence.
    let yaml_text = yaml_text.strip_suffix('\r').unwrap_or(yaml_text);
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

    #[test]
    fn crlf_document_strips_dangling_cr_from_yaml() {
        let content = "---\r\ndescription: Plans work\r\ntags:\r\n  - a\r\n---\r\n# Body\r\n";
        let (fm, body) = parse_frontmatter(content);
        let description = fm
            .as_ref()
            .and_then(|value| value.get("description"))
            .and_then(|value| value.as_str())
            .unwrap();
        assert_eq!(description, "Plans work");
        assert!(body.starts_with("# Body"));
    }

    #[test]
    fn empty_frontmatter_block_is_treated_as_absent() {
        assert_eq!(extract_description("---\n---\n# Real heading\n"), "Real heading");
        let (fm, body) = parse_frontmatter("---\n---\nBody");
        assert!(fm.is_none());
        assert_eq!(body, "Body");
    }
}
