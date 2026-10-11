//! Only a top-level appendix can attest a task; examples remain ordinary content.
use pulldown_cmark::{Event, Parser, Tag};

#[derive(Default)]
struct RawHtml {
    containers: Vec<String>,
    comment: bool,
    pending: String,
    ambiguous: bool,
}

/// Find a reserved marker in root Markdown and outside unresolved raw HTML.
/// CommonMark code spans, fenced/indented code and quoted/list examples never
/// enter the HTML tracker. The returned byte offset belongs to the input text.
pub(super) fn appendix_start(answer: &str) -> Option<usize> {
    let mut markdown = Vec::new();
    let mut html = RawHtml::default();
    for (event, range) in Parser::new(answer).into_offset_iter() {
        match event {
            Event::Start(tag) => markdown.push(matches!(tag, Tag::HtmlBlock | Tag::Paragraph)),
            Event::End(_) => {
                markdown.pop();
            }
            Event::Html(_) | Event::InlineHtml(_) | Event::Text(_)
                if markdown.is_empty() || (markdown.len() == 1 && markdown[0]) =>
            {
                let html_event = matches!(event, Event::Html(_) | Event::InlineHtml(_));
                let mut offset = range.start;
                for part in answer[range].split_inclusive('\n') {
                    // The reserved underscore-bearing name is literal text in
                    // CommonMark. Event boundaries may split it into fragments.
                    let line = &answer[offset..];
                    let column_zero = offset == 0 || answer.as_bytes()[offset - 1] == b'\n';
                    if column_zero
                        && html.top_level()
                        && (line.starts_with("<wardian_task_outcomes")
                            || line.starts_with("</wardian_task_outcomes"))
                    {
                        return Some(offset);
                    }
                    if html_event {
                        html.advance(part);
                    }
                    offset += part.len();
                }
            }
            _ => {}
        }
    }
    None
}

impl RawHtml {
    fn advance(&mut self, chunk: &str) {
        let mut text = std::mem::take(&mut self.pending);
        text.push_str(chunk);
        let mut cursor = 0;
        while cursor < text.len() {
            let rest = &text[cursor..];
            if self.comment {
                let Some(end) = rest.find("-->") else { break };
                self.comment = false;
                cursor += end + 3;
                continue;
            }
            if let Some(name) = self
                .containers
                .last()
                .filter(|name| matches!(name.as_str(), "script" | "style" | "textarea"))
            {
                let lower = rest.to_ascii_lowercase();
                let needle = format!("</{name}");
                let found = lower.match_indices(&needle).find(|(position, _)| {
                    lower
                        .as_bytes()
                        .get(position + needle.len())
                        .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>')
                });
                let Some((position, _)) = found else { break };
                cursor += position;
            } else {
                let Some(position) = rest.find('<') else {
                    break;
                };
                cursor += position;
            }
            let rest = &text[cursor..];
            if rest.starts_with("<!--") {
                self.comment = true;
                cursor += 4;
                continue;
            }
            let mut quote = None;
            let end = rest.bytes().enumerate().skip(1).find_map(|(index, byte)| {
                if quote == Some(byte) {
                    quote = None;
                } else if quote.is_none() && matches!(byte, b'\'' | b'"') {
                    quote = Some(byte);
                } else if quote.is_none() && byte == b'>' {
                    return Some(index);
                }
                None
            });
            let Some(end) = end else {
                self.pending.push_str(rest);
                break;
            };
            self.token(&rest[..=end]);
            cursor += end + 1;
        }
    }

    fn top_level(&self) -> bool {
        self.containers.is_empty() && !self.comment && self.pending.is_empty() && !self.ambiguous
    }

    fn token(&mut self, token: &str) {
        let Some(text) = token
            .strip_prefix('<')
            .and_then(|text| text.strip_suffix('>'))
        else {
            return;
        };
        if text.starts_with(['!', '?']) {
            return;
        }
        let closing = text.starts_with('/');
        let text = text.strip_prefix('/').unwrap_or(text);
        if !text.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
            return;
        }
        let width = text
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-')
            .count();
        let tail = &text[width..];
        if (!tail.is_empty() && !tail.as_bytes()[0].is_ascii_whitespace() && tail != "/")
            || (closing && !tail.trim().is_empty())
        {
            return;
        }
        let name = text[..width].to_ascii_lowercase();
        if closing {
            if self.containers.last() == Some(&name) {
                self.containers.pop();
            } else {
                self.ambiguous = true;
            }
        } else if !matches!(
            name.as_str(),
            "area"
                | "base"
                | "br"
                | "col"
                | "embed"
                | "hr"
                | "img"
                | "input"
                | "link"
                | "meta"
                | "param"
                | "source"
                | "track"
                | "wbr"
        ) {
            self.containers.push(name);
        }
    }
}
