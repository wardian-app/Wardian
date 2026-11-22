//! Read-only vacancy inspection and live-desktop-only authorization.
use crate::{args::ArchiveVacancyArgs, control_error, errors::CliError, live};
use wardian_core::control::ArchiveVacancyRequest;

pub(crate) fn archive_vacancy(args: &ArchiveVacancyArgs) -> Result<String, CliError> {
    if args.dry_run {
        return dry_run(args);
    }
    let request = ArchiveVacancyRequest {
        conversation_id: args.conversation_id.clone(),
        sequence: args.sequence,
        authorization_id: required(&args.authorization_id, "--authorization-id")?,
        approval_reference: required(&args.approval_reference, "--approval-reference")?,
        approval_text: required(&args.approval_text, "--approval-text")?,
    };
    request.validate().map_err(CliError::generic)?;
    let response = live::agent_archive_vacancy(&args.target, request).map_err(control_error)?;
    render(&response)
}

fn dry_run(args: &ArchiveVacancyArgs) -> Result<String, CliError> {
    let agent_id = resolve_agent_id(&args.target)?;
    let directory = wardian_core::paths::agent_conversation_dir(&agent_id, &args.conversation_id)
        .ok_or_else(|| CliError::generic("unsafe agent or conversation path"))?;
    let report = wardian_core::archive_vacancy::inspect_vacancy(
        &directory,
        &args.conversation_id,
        args.sequence,
    )
    .map_err(|error| CliError::generic(error.to_string()))?;
    #[derive(serde::Serialize)]
    struct DryRunResponse<'a> {
        schema: u8,
        agent_id: &'a str,
        dry_run: bool,
        #[serde(flatten)]
        report: wardian_core::archive_vacancy::VacancyReport,
    }
    render(&DryRunResponse {
        schema: wardian_core::control::CONTROL_SCHEMA,
        agent_id: &agent_id,
        dry_run: true,
        report,
    })
}

fn required(value: &Option<String>, flag: &str) -> Result<String, CliError> {
    value
        .clone()
        .ok_or_else(|| CliError::generic(format!("{flag} is required without --dry-run")))
}

fn render(value: &impl serde::Serialize) -> Result<String, CliError> {
    serde_json::to_string(value)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

fn exact_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn resolve_agent_id(target: &str) -> Result<String, CliError> {
    if exact_uuid(target) {
        return Ok(target.to_owned());
    }
    let path = wardian_core::paths::state_db_path()
        .ok_or_else(|| CliError::db_unavailable("could not resolve Wardian state.db path"))?;
    // Immutable mode prevents SQLite from creating WAL/shared-memory files.
    // This resolves persisted identity only; an exact UUID needs no database.
    let path = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file:");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.') {
            uri.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(uri, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    uri.push_str("?mode=ro&immutable=1");
    let conn = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| CliError::db_unavailable(error.to_string()))?;
    wardian_core::identity::resolve_by_name_or_uuid(&conn, target)
        .map(|agent| agent.uuid)
        .map_err(crate::identity_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_uuid_requires_a_complete_path_safe_uuid() {
        assert!(exact_uuid("a5f6ad8a-0ac5-4ec2-99e0-b856663594a8"));
        for value in [
            "a5f6ad8a0ac54ec299e0b856663594a8",
            "../a5f6ad8a-0ac5-4ec2-99e0-b856663594a8",
            "a5f6ad8a-0ac5-4ec2-99e0-b856663594aZ",
            "coder-a1",
        ] {
            assert!(!exact_uuid(value), "{value}");
        }
    }
}
