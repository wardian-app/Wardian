use serde_json::Value;

use super::owner::{OpenCodeHttpError, OpenCodeHttpErrorCode};

const MAX_EVENT_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone)]
pub struct OpenCodeEvent {
    pub event: Option<String>,
    pub data: Value,
    pub raw_data: String,
}

impl OpenCodeEvent {
    pub fn event_type(&self) -> Option<&str> {
        self.event
            .as_deref()
            .or_else(|| self.data.get("type").and_then(Value::as_str))
    }

    pub fn session_id(&self) -> Option<&str> {
        self.data
            .get("properties")
            .and_then(|properties| properties.get("sessionID"))
            .and_then(Value::as_str)
            .or_else(|| self.data.get("sessionID").and_then(Value::as_str))
    }

    pub fn message_id(&self) -> Option<&str> {
        self.data
            .get("properties")
            .and_then(|properties| properties.get("messageID"))
            .and_then(Value::as_str)
            .or_else(|| self.data.get("messageID").and_then(Value::as_str))
    }
}

pub struct OpenCodeEventStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
    event: Option<String>,
    data_lines: Vec<String>,
    frame_bytes: usize,
    finished: bool,
}

impl OpenCodeEventStream {
    pub(crate) fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            buffer: Vec::new(),
            event: None,
            data_lines: Vec::new(),
            frame_bytes: 0,
            finished: false,
        }
    }

    pub async fn next_event(&mut self) -> Result<Option<OpenCodeEvent>, OpenCodeHttpError> {
        loop {
            if let Some(event) = self.take_line()? {
                if let Some(event) = event {
                    return Ok(Some(event));
                }
                continue;
            }

            if self.finished {
                return self.finish_frame();
            }

            match self.response.chunk().await {
                Ok(Some(chunk)) => {
                    if self.buffer.len().saturating_add(chunk.len()) > MAX_EVENT_BYTES {
                        return Err(OpenCodeHttpError::new(
                            OpenCodeHttpErrorCode::MalformedEvent,
                            false,
                            None,
                            "OpenCode event exceeded the size limit",
                        ));
                    }
                    self.buffer.extend_from_slice(&chunk);
                }
                Ok(None) => self.finished = true,
                Err(_) => {
                    return Err(OpenCodeHttpError::new(
                        OpenCodeHttpErrorCode::TransportUnavailable,
                        false,
                        None,
                        "OpenCode event stream read failed",
                    ));
                }
            }
        }
    }

    fn take_line(&mut self) -> Result<Option<Option<OpenCodeEvent>>, OpenCodeHttpError> {
        let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') else {
            return Ok(None);
        };
        let line = self.buffer.drain(..=index).collect::<Vec<_>>();
        let line = std::str::from_utf8(&line[..line.len().saturating_sub(1)]).map_err(|_| {
            OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedEvent,
                false,
                None,
                "OpenCode event stream was not UTF-8",
            )
        })?;
        let line = line.strip_suffix('\r').unwrap_or(line);
        self.frame_bytes = self.frame_bytes.saturating_add(line.len());
        if self.frame_bytes > MAX_EVENT_BYTES {
            return Err(OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedEvent,
                false,
                None,
                "OpenCode event frame exceeded the size limit",
            ));
        }
        if line.is_empty() {
            return Ok(Some(self.take_frame()?));
        }
        if line.starts_with(':') {
            return Ok(Some(None));
        }
        if let Some(value) = line.strip_prefix("event:") {
            self.event = Some(value.trim_start().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            self.data_lines
                .push(value.strip_prefix(' ').unwrap_or(value).to_string());
        }
        Ok(Some(None))
    }

    fn take_frame(&mut self) -> Result<Option<OpenCodeEvent>, OpenCodeHttpError> {
        if self.data_lines.is_empty() {
            self.event = None;
            self.frame_bytes = 0;
            return Ok(None);
        }
        let raw_data = self.data_lines.join("\n");
        let data = serde_json::from_str(&raw_data).map_err(|_| {
            OpenCodeHttpError::new(
                OpenCodeHttpErrorCode::MalformedEvent,
                false,
                None,
                "OpenCode event data was not valid JSON",
            )
        })?;
        let event = OpenCodeEvent {
            event: self.event.take(),
            data,
            raw_data,
        };
        self.data_lines.clear();
        self.frame_bytes = 0;
        Ok(Some(event))
    }

    fn finish_frame(&mut self) -> Result<Option<OpenCodeEvent>, OpenCodeHttpError> {
        if !self.buffer.is_empty() {
            self.buffer.push(b'\n');
            return Ok(None);
        }
        self.take_frame()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn event_helpers_use_opencode_global_event_properties() {
        let event = super::OpenCodeEvent {
            event: None,
            data: serde_json::json!({
                "type": "message.updated",
                "properties": {"sessionID": "ses_1", "messageID": "msg_1"}
            }),
            raw_data: "{}".to_string(),
        };
        assert_eq!(event.event_type(), Some("message.updated"));
        assert_eq!(event.session_id(), Some("ses_1"));
        assert_eq!(event.message_id(), Some("msg_1"));
    }
}
