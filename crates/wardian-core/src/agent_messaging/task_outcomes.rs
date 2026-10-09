//! Positive task-result attribution, separate from a provider turn ending.
//! Routing eligibility belongs to the durable binding transaction, not this parser.

use super::{AgentMessagingError, MAX_MESSAGE_BYTES, MAX_RECEIVE_SERIALIZED_BYTES};
use crate::control::ReplyStatus;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const TASK_OUTCOME_OPEN: &str = "<wardian_task_outcomes>";
pub const TASK_OUTCOME_CLOSE: &str = "</wardian_task_outcomes>";
pub const MAX_TASK_OUTCOMES: usize = 16;

/// Uniform host protocol for native start, active steering and compaction recovery.
/// This constrains peer-task attribution without overriding human output requests.
pub const TASK_OUTCOME_INSTRUCTIONS: &str = "For each Wardian peer task, preserve its exact request_id. Finish it with Wardian reply(request_id,status,message), or append one task-outcome block at column zero after a blank line at the end of your final answer: <wardian_task_outcomes> on its own line, then JSON {\"schema_version\":1,\"outcomes\":[{\"request_id\":\"the exact request_id\",\"status\":\"done\",\"result\":\"the task-specific result\"}]}, then </wardian_task_outcomes> on its own line. Use status done, failed or blocked for each result you actually supply. Include at most 16 unique IDs; each nonempty result is limited to 64 KiB and the complete JSON to 512 KiB. Never place the block in a code fence, quotation or HTML example. The whole block must be valid and all IDs must belong to tasks accepted in this exact turn. Omitted tasks and ordinary final prose remain unresolved; a turn ending does not itself complete any task. Human instructions take priority. If their required output format cannot contain this appendix, use explicit reply instead. Never guess an ID or describe another task's answer as this task's result.";

/// An agent attestation. The host must still match every ID to an eligible binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskOutcome {
    pub request_id: String,
    pub status: ReplyStatus,
    pub result: String,
}

/// One complete appendix is validated before any of its entries can be published.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskOutcomePacket {
    pub schema_version: u8,
    pub outcomes: Vec<TaskOutcome>,
}

/// Ordinary final prose is retained byte-for-byte, including its separator.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedTaskOutcomes<'a> {
    pub prose: &'a str,
    pub packet: TaskOutcomePacket,
}

fn invalid(message: &str) -> AgentMessagingError {
    AgentMessagingError::new("invalid_task_outcome", message)
}

impl TaskOutcomePacket {
    /// Validate syntax and bounds only. This never authorizes a request ID.
    pub fn validate(&self) -> Result<(), AgentMessagingError> {
        if self.schema_version != 1 || self.outcomes.len() > MAX_TASK_OUTCOMES {
            return Err(invalid("Unsupported task-result version or entry count."));
        }
        let mut ids = HashSet::new();
        for outcome in &self.outcomes {
            if outcome.request_id.is_empty()
                || outcome.request_id.len() > 128
                || !ids.insert(outcome.request_id.as_str())
            {
                return Err(invalid(
                    "Task-result IDs must be nonempty, bounded and unique.",
                ));
            }
            if outcome.result.trim().is_empty() || outcome.result.len() > MAX_MESSAGE_BYTES {
                return Err(invalid(
                    "Task-result bodies must be nonempty and within the reply byte limit.",
                ));
            }
        }
        Ok(())
    }
}

mod framing;

/// Recognize only a reserved, unquoted final appendix outside Markdown fences.
/// Arbitrary JSON, fenced examples, malformed packets and unknown versions
/// cannot turn prose into a canonical task result.
pub fn parse_task_outcomes(
    answer: &str,
) -> Result<Option<ParsedTaskOutcomes<'_>>, AgentMessagingError> {
    let Some(start) = framing::appendix_start(answer) else {
        return Ok(None);
    };
    let mut markers: Vec<(usize, usize, &str)> = Vec::new();
    let mut offset = start;
    for part in answer[start..].split_inclusive('\n') {
        let line = part.trim_end_matches(['\r', '\n']);
        if line.starts_with("<wardian_task_outcomes") || line.starts_with("</wardian_task_outcomes")
        {
            markers.push((offset, offset + part.len(), line));
            if markers.len() > 2 {
                return Err(invalid(
                    "Multiple reserved task-result blocks are ambiguous.",
                ));
            }
        }
        offset += part.len();
    }
    if markers.is_empty() {
        return Ok(None);
    }
    if markers.len() != 2 || markers[0].2 != TASK_OUTCOME_OPEN || markers[1].2 != TASK_OUTCOME_CLOSE
    {
        return Err(invalid(
            "A task-result final must contain one unambiguous reserved appendix.",
        ));
    }
    let (start, payload_start, _) = markers[0];
    let (payload_end, end, _) = markers[1];
    if !answer[end..].trim().is_empty()
        || (start != 0
            && !answer[..start].ends_with("\n\n")
            && !answer[..start].ends_with("\r\n\r\n"))
    {
        return Err(invalid(
            "The task-result appendix must be the final block, separated from prose.",
        ));
    }
    let payload = &answer[payload_start..payload_end];
    if payload.len() > MAX_RECEIVE_SERIALIZED_BYTES {
        return Err(invalid(
            "The task-result appendix exceeds its serialized byte limit.",
        ));
    }
    let packet: TaskOutcomePacket = serde_json::from_str(payload)
        .map_err(|_| invalid("The task-result appendix does not match its JSON schema."))?;
    packet.validate()?;
    Ok(Some(ParsedTaskOutcomes {
        prose: &answer[..start],
        packet,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(result: &str) -> TaskOutcomePacket {
        TaskOutcomePacket {
            schema_version: 1,
            outcomes: vec![TaskOutcome {
                request_id: "ask_owned".into(),
                status: ReplyStatus::Done,
                result: result.into(),
            }],
        }
    }

    fn appendix(packet: &TaskOutcomePacket) -> String {
        format!(
            "{TASK_OUTCOME_OPEN}\n{}\n{TASK_OUTCOME_CLOSE}",
            serde_json::to_string(packet).unwrap()
        )
    }

    #[test]
    fn generic_old_work_finals_and_arbitrary_json_do_not_attest_a_task() {
        for answer in [
            "Earlier review B approved.",
            "The old CRM question C is answered.",
            r#"{"schema_version":1,"outcomes":[{"request_id":"ask_owned","status":"done","result":"B"}]}"#,
        ] {
            assert_eq!(parse_task_outcomes(answer).unwrap(), None);
        }
    }

    #[test]
    fn exact_final_packet_preserves_prose_and_result_bytes() {
        let body = "\r\n café 日本語 'quoted' \\ \r\n";
        let prose = "Human answer.\r\n\r\n";
        let final_answer = format!(
            "{prose}{}\r\n",
            appendix(&packet(body)).replace('\n', "\r\n")
        );
        let parsed = parse_task_outcomes(&final_answer).unwrap().unwrap();
        assert_eq!(parsed.prose, prose);
        assert_eq!(parsed.packet, packet(body));
        let literal_syntax = "Literal <!-- and ``` inside a JSON string.";
        let answer = appendix(&packet(literal_syntax));
        assert_eq!(
            parse_task_outcomes(&answer).unwrap().unwrap().packet,
            packet(literal_syntax)
        );
    }

    #[test]
    fn underscore_delimiters_are_recognized_from_literal_source_offsets() {
        use pulldown_cmark::{Event, Parser};
        let answer = appendix(&packet("actual result"));
        assert!(!Parser::new(&answer)
            .any(|event| matches!(event, Event::Html(_) | Event::InlineHtml(_))));
        assert_eq!(framing::appendix_start(&answer), Some(0));
        assert_eq!(
            parse_task_outcomes(&answer).unwrap().unwrap().packet,
            packet("actual result")
        );
    }

    #[test]
    fn code_fences_quotes_and_indentation_cannot_supply_an_outcome() {
        let result = appendix(&packet("example"));
        for answer in [
            format!("```json\n{result}\n```"),
            format!("~~~~example\n{result}\n~~~~"),
            result
                .lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            result
                .lines()
                .map(|line| format!("    {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ] {
            assert_eq!(parse_task_outcomes(&answer).unwrap(), None);
        }
    }

    #[test]
    fn commented_examples_are_not_outcomes_and_fenced_comments_do_not_hide_a_final() {
        let result = appendix(&packet("example"));
        for answer in [
            format!("<!--\n{result}\n-->"),
            format!("<!-- unclosed example\n{result}"),
            format!("<!-- first --> <!-- second\n{result}\n-->"),
        ] {
            assert_eq!(parse_task_outcomes(&answer).unwrap(), None);
        }
        for prose in ["<!-- example -->\n\n", "```html\n<!--\n```\n\n"] {
            let answer = format!("{prose}{result}");
            let parsed = parse_task_outcomes(&answer).unwrap().unwrap();
            assert_eq!(parsed.prose, prose);
            assert_eq!(parsed.packet, packet("example"));
        }
    }

    #[test]
    fn framing_ambiguity_trailing_prose_and_missing_separator_reject_the_packet() {
        let result = appendix(&packet("done"));
        for answer in [
            format!("{result}\n\n{result}"),
            format!("{result}\nMore prose."),
            format!("Prose.\n{result}"),
            format!("{TASK_OUTCOME_OPEN}\n{{}}"),
            "<wardian_task_outcomes_v2>\n{}\n</wardian_task_outcomes_v2>".to_owned(),
        ] {
            assert!(parse_task_outcomes(&answer).is_err());
        }
    }

    #[test]
    fn raw_html_and_nested_examples_cannot_attest_unfinished_tasks() {
        let result = appendix(&packet("example"));
        for prefix in [
            "<pre>\nExample packet:\n\n",
            "<script>\nExample packet:\n\n",
            "<div>\n<pre>\nExample packet:\n\n",
            "<div>\n<pre>\n</pre>\n\n",
            "<pre>\n<!-- </pre> -->\n\n",
            "<div>\n<//div>\n\n",
            "<div>\n< /div>\n\n",
            "<div>\n</div_ignored>\n\n",
            "<div/>\n\n",
        ] {
            assert_eq!(
                parse_task_outcomes(&format!("{prefix}{result}")).unwrap(),
                None
            );
        }
        let prose = format!("<div>\n<pre>\n{result}\n</pre>\n</div>\n\n");
        let answer = format!("{prose}{result}");
        let parsed = parse_task_outcomes(&answer).unwrap().unwrap();
        assert_eq!(parsed.prose, prose);
        assert_eq!(parsed.packet, packet("example"));
    }

    #[test]
    fn inline_comment_tokens_are_code_and_closed_examples_allow_a_real_appendix() {
        let result = appendix(&packet("actual result"));
        for prose in [
            "The token `<!--` opens an HTML comment.\n\n",
            "The token ``\nliteral <!--\n`` opens a comment.\n\n",
            "The token `<pre>` opens a container.\n\n",
            "<div title=\"a > b\">\nExample\n</div>\n\n",
            "> <pre>\n> quoted example\n\n",
            "A line.<br>\n\n",
        ] {
            let answer = format!("{prose}{result}");
            let parsed = parse_task_outcomes(&answer)
                .unwrap_or_else(|error| panic!("prefix {prose:?}: {error:?}"))
                .unwrap_or_else(|| panic!("appendix missing after prefix {prose:?}"));
            assert_eq!(parsed.prose, prose);
            assert_eq!(parsed.packet, packet("actual result"));
        }
    }

    #[test]
    fn standalone_comment_interrupts_multiline_code_and_hides_an_appendix() {
        // CommonMark resolves HTML blocks before code spans. A standalone
        // comment opener interrupts the paragraph and consumes the appendix.
        let prose = "The token ``\n<!--\n`` opens a comment.\n\n";
        let answer = format!("{prose}{}", appendix(&packet("actual result")));
        assert_eq!(parse_task_outcomes(&answer).unwrap(), None);
    }

    #[test]
    fn one_bad_entry_rejects_the_whole_packet() {
        let mut valid = packet("valid");
        for bad in [
            TaskOutcome {
                request_id: "ask_owned".into(),
                status: ReplyStatus::Done,
                result: "duplicate".into(),
            },
            TaskOutcome {
                request_id: "other".into(),
                status: ReplyStatus::Done,
                result: " \n".into(),
            },
            TaskOutcome {
                request_id: "other".into(),
                status: ReplyStatus::Done,
                result: "x".repeat(MAX_MESSAGE_BYTES + 1),
            },
        ] {
            valid.outcomes.push(bad);
            assert!(parse_task_outcomes(&appendix(&valid)).is_err());
            valid.outcomes.pop();
        }
    }

    #[test]
    fn versions_fields_statuses_and_trailing_json_are_strict() {
        for payload in [
            r#"{"schema_version":2,"outcomes":[]}"#,
            r#"{"schema_version":1,"outcomes":[],"extra":true}"#,
            r#"{"schema_version":1,"outcomes":[{"request_id":"a","status":"completed","result":"x"}]}"#,
            r#"{"schema_version":1,"outcomes":[]}{}"#,
        ] {
            assert!(parse_task_outcomes(&format!(
                "{TASK_OUTCOME_OPEN}\n{payload}\n{TASK_OUTCOME_CLOSE}"
            ))
            .is_err());
        }
    }

    #[test]
    fn reply_limits_count_utf8_bytes_and_allow_an_exact_maximum_body() {
        assert!(parse_task_outcomes(&appendix(&packet(&"x".repeat(MAX_MESSAGE_BYTES)))).is_ok());
        assert!(
            parse_task_outcomes(&appendix(&packet(&"日".repeat(MAX_MESSAGE_BYTES / 3 + 1))))
                .is_err()
        );
    }
}
