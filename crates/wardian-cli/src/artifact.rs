use crate::{
    args::{ArtifactArgs, ArtifactCommand, ArtifactReviewCommand},
    control_error,
    errors::{CliError, ExitCode},
    live,
};
use std::path::{Path, PathBuf};
use wardian_core::{artifacts::ArtifactStore, control::MessageOrigin};

pub fn handle_artifact(args: ArtifactArgs) -> Result<String, CliError> {
    match args.command {
        ArtifactCommand::Present {
            path,
            title,
            description,
            artifact,
            force_new,
            addressed_comment_ids,
        } => {
            let session_id = current_session_id()?;
            let path = absolute_path(&path)?;
            let response = live::artifact_present(
                path.to_string_lossy().as_ref(),
                title.as_deref(),
                description.as_deref(),
                artifact.as_deref(),
                force_new,
                &addressed_comment_ids,
                MessageOrigin::WardianAgent { session_id },
            )
            .map_err(control_error)?;
            render_json(&response)
        }
        ArtifactCommand::Show {
            artifact_id,
            version,
        } => {
            let response = match live::artifact_show(&artifact_id, version.as_deref()) {
                Ok(response) => response,
                Err(_) => disk_artifact_show(&artifact_id, version.as_deref())?,
            };
            render_json(&response)
        }
        ArtifactCommand::Review { command } => match command {
            ArtifactReviewCommand::Show {
                artifact_id,
                review,
                latest,
            } => {
                let response = live::artifact_review_show(&artifact_id, review.as_deref(), latest)
                    .map_err(control_error)?;
                render_json(&response)
            }
        },
    }
}

fn current_session_id() -> Result<String, CliError> {
    std::env::var("WARDIAN_SESSION_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CliError::backend(
                ExitCode::NotInSession,
                "invalid_origin",
                "WARDIAN_SESSION_ID is required to present an artifact",
            )
        })
}

fn absolute_path(path: &str) -> Result<PathBuf, CliError> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| CliError::generic(format!("cannot resolve artifact path: {error}")))
}

fn disk_artifact_show(
    artifact_id: &str,
    version_id: Option<&str>,
) -> Result<serde_json::Value, CliError> {
    let home = wardian_core::paths::wardian_home()
        .ok_or_else(|| CliError::db_unavailable("Wardian home is unavailable"))?;
    let store = ArtifactStore::open(home.join("artifacts"))
        .map_err(|error| CliError::generic(error.to_string()))?;
    let stored = store
        .load_version(artifact_id, version_id)
        .map_err(|error| match error {
            wardian_core::artifacts::ArtifactStoreError::ArtifactNotFound(_)
            | wardian_core::artifacts::ArtifactStoreError::VersionNotFound { .. } => {
                CliError::backend(ExitCode::NotFound, "artifact_not_found", error.to_string())
            }
            _ => CliError::generic(error.to_string()),
        })?;
    Ok(serde_json::json!({
        "schema": 1,
        "manifest": stored.manifest,
        "selected_version": stored.version,
    }))
}

fn render_json(value: &serde_json::Value) -> Result<String, CliError> {
    serde_json::to_string_pretty(value)
        .map(|json| format!("{json}\n"))
        .map_err(|error| CliError::generic(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Cli, Command};
    use clap::Parser;

    #[test]
    fn parser_accepts_present_and_repeated_address_flags() {
        let cli = Cli::try_parse_from([
            "wardian",
            "artifact",
            "present",
            "report.md",
            "--title",
            "Report",
            "--address",
            "comment-1",
            "--address",
            "comment-2",
        ])
        .expect("parse");
        let Command::Artifact(ArtifactArgs {
            command:
                ArtifactCommand::Present {
                    addressed_comment_ids,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected artifact present");
        };
        assert_eq!(addressed_comment_ids, ["comment-1", "comment-2"]);
    }

    #[test]
    fn parser_rejects_new_with_explicit_artifact() {
        let error = Cli::try_parse_from([
            "wardian",
            "artifact",
            "present",
            "report.md",
            "--new",
            "--artifact",
            "artifact-1",
        ])
        .expect_err("conflict");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn present_requires_a_managed_session() {
        let _guard = crate::test_env_lock();
        let previous = std::env::var_os("WARDIAN_SESSION_ID");
        unsafe { std::env::remove_var("WARDIAN_SESSION_ID") };
        let error = current_session_id().expect_err("missing session");
        if let Some(previous) = previous {
            unsafe { std::env::set_var("WARDIAN_SESSION_ID", previous) };
        }
        assert_eq!(error.code, "invalid_origin");
        assert_eq!(error.code_i32(), ExitCode::NotInSession as i32);
    }
}
