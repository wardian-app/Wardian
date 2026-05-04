use crate::errors::CliError;
use serde_json::{Map, Value};
use wardian_core::identity::AgentIdentity;

const DEFAULT_FIELDS: &[&str] = &["name", "uuid", "class", "provider", "workspace", "status"];
const VERBOSE_FIELDS: &[&str] = &["pid", "started_at", "last_status_at"];
const ALL_FIELDS: &[&str] = &[
    "name",
    "uuid",
    "class",
    "provider",
    "workspace",
    "status",
    "pid",
    "started_at",
    "last_status_at",
];

#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    pub fields: Option<Vec<String>>,
    pub field: Option<String>,
    pub verbose: bool,
    pub pretty: bool,
}

pub fn render_show(agent: &AgentIdentity, opts: &RenderOptions) -> Result<String, CliError> {
    if let Some(field) = opts.field.as_deref() {
        return render_single_field(agent, field);
    }

    let projected = project_agent(agent, opts)?;
    if opts.pretty {
        return Ok(render_pretty(&projected));
    }

    let envelope = serde_json::json!({
        "schema": 1,
        "agent": projected,
    });
    Ok(format!(
        "{}\n",
        serde_json::to_string(&envelope).map_err(json_error)?
    ))
}

pub fn render_list(agents: &[AgentIdentity], opts: &RenderOptions) -> Result<String, CliError> {
    if let Some(field) = opts.field.as_deref() {
        let mut out = String::new();
        for agent in agents {
            out.push_str(render_single_field(agent, field)?.trim_end());
            out.push('\n');
        }
        return Ok(out);
    }

    let projected = agents
        .iter()
        .map(|agent| project_agent(agent, opts))
        .collect::<Result<Vec<_>, _>>()?;
    if opts.pretty {
        let mut out = String::new();
        for agent in projected {
            out.push_str(&render_pretty(&agent));
            out.push('\n');
        }
        return Ok(out);
    }

    let envelope = serde_json::json!({
        "schema": 1,
        "agents": projected,
    });
    Ok(format!(
        "{}\n",
        serde_json::to_string(&envelope).map_err(json_error)?
    ))
}

fn project_agent(
    agent: &AgentIdentity,
    opts: &RenderOptions,
) -> Result<Map<String, Value>, CliError> {
    let requested = requested_fields(opts);
    let values = agent_to_map(agent);
    let mut projected = Map::new();
    for field in requested {
        if !ALL_FIELDS.contains(&field.as_str()) {
            return Err(CliError::invalid_field(&field));
        }
        if let Some(value) = values.get(&field) {
            projected.insert(field, value.clone());
        }
    }
    Ok(projected)
}

fn requested_fields(opts: &RenderOptions) -> Vec<String> {
    if let Some(fields) = &opts.fields {
        return fields.clone();
    }

    let mut fields = DEFAULT_FIELDS
        .iter()
        .map(|field| (*field).to_string())
        .collect::<Vec<_>>();
    if opts.verbose {
        fields.extend(VERBOSE_FIELDS.iter().map(|field| (*field).to_string()));
    }
    fields
}

fn render_single_field(agent: &AgentIdentity, field: &str) -> Result<String, CliError> {
    if !ALL_FIELDS.contains(&field) {
        return Err(CliError::invalid_field(field));
    }
    let values = agent_to_map(agent);
    let value = values
        .get(field)
        .map(value_to_bare_string)
        .unwrap_or_default();
    Ok(format!("{value}\n"))
}

fn agent_to_map(agent: &AgentIdentity) -> Map<String, Value> {
    let mut values = Map::new();
    values.insert("name".to_string(), Value::String(agent.name.clone()));
    values.insert("uuid".to_string(), Value::String(agent.uuid.clone()));
    values.insert("class".to_string(), Value::String(agent.class.clone()));
    values.insert(
        "provider".to_string(),
        Value::String(agent.provider.clone()),
    );
    values.insert("status".to_string(), Value::String(agent.status.clone()));
    if let Some(pid) = agent.pid {
        values.insert("pid".to_string(), serde_json::json!(pid));
    }
    if let Some(started_at) = &agent.started_at {
        values.insert("started_at".to_string(), Value::String(started_at.clone()));
    }
    if let Some(workspace) = &agent.workspace {
        values.insert("workspace".to_string(), Value::String(workspace.clone()));
    }
    if let Some(last_status_at) = &agent.last_status_at {
        values.insert(
            "last_status_at".to_string(),
            Value::String(last_status_at.clone()),
        );
    }
    values
}

fn value_to_bare_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn render_pretty(values: &Map<String, Value>) -> String {
    let width = values.keys().map(String::len).max().unwrap_or(0);
    let mut out = String::new();
    for (key, value) in values {
        out.push_str(&format!(
            "{key:width$}  {}\n",
            value_to_bare_string(value),
            width = width
        ));
    }
    out
}

fn json_error(error: serde_json::Error) -> CliError {
    CliError::generic(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardian_core::identity::AgentIdentity;

    fn agent() -> AgentIdentity {
        AgentIdentity {
            name: "coder-a1".to_string(),
            uuid: "uuid-1".to_string(),
            class: "Coder".to_string(),
            provider: "codex".to_string(),
            status: "processing".to_string(),
            pid: Some(111),
            started_at: Some("2026-05-03T20:00:00.000Z".to_string()),
            workspace: Some("D:/Development/Wardian".to_string()),
            last_status_at: Some("2026-05-03T20:01:00.000Z".to_string()),
        }
    }

    #[test]
    fn render_show_outputs_json_envelope() {
        let rendered = render_show(&agent(), &RenderOptions::default()).unwrap();
        assert!(rendered.contains(r#""schema":1"#));
        assert!(rendered.contains(r#""agent""#));
        assert!(rendered.contains(r#""name":"coder-a1""#));
        assert!(!rendered.contains(r#""pid""#));
    }

    #[test]
    fn render_list_outputs_agents_envelope() {
        let rendered = render_list(&[agent()], &RenderOptions::default()).unwrap();
        assert!(rendered.contains(r#""schema":1"#));
        assert!(rendered.contains(r#""agents":["#));
    }

    #[test]
    fn verbose_adds_verbose_fields() {
        let rendered = render_show(
            &agent(),
            &RenderOptions {
                verbose: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(rendered.contains(r#""pid":111"#));
        assert!(rendered.contains(r#""workspace":"D:/Development/Wardian""#));
    }

    #[test]
    fn fields_selects_only_requested_fields() {
        let rendered = render_show(
            &agent(),
            &RenderOptions {
                fields: Some(vec!["name".to_string(), "status".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(rendered.contains(r#""name":"coder-a1""#));
        assert!(rendered.contains(r#""status":"processing""#));
        assert!(!rendered.contains(r#""uuid""#));
    }

    #[test]
    fn field_returns_bare_value() {
        let rendered = render_show(
            &agent(),
            &RenderOptions {
                field: Some("status".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "processing\n");
    }

    #[test]
    fn unknown_field_errors() {
        let error = render_show(
            &agent(),
            &RenderOptions {
                field: Some("env".to_string()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "invalid_field");
    }

    #[test]
    fn pretty_outputs_human_block() {
        let rendered = render_show(
            &agent(),
            &RenderOptions {
                pretty: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(rendered.contains("name"));
        assert!(rendered.contains("coder-a1"));
    }
}
