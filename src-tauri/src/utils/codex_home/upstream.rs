use std::path::PathBuf;

/// The application's upstream home is distinct from each provider child's
/// Wardian-managed CODEX_HOME. Explicit selection must never fall back silently.
pub(crate) struct UpstreamHome {
    pub(crate) path: PathBuf,
    pub(crate) explicit: bool,
}

/// Resolve the standard application CODEX_HOME before projection or publication.
/// An explicit directory is canonicalized; unset and empty values retain the
/// native profile default, which may be created during ordinary preparation.
pub(crate) fn resolve_upstream_home() -> Result<UpstreamHome, String> {
    if let Some(value) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        let path = std::fs::canonicalize(PathBuf::from(value))
            .map_err(|error| format!("Could not resolve explicit CODEX_HOME: {error}"))?;
        if !path.is_dir() {
            return Err("Explicit CODEX_HOME must be an existing directory".into());
        }
        return Ok(UpstreamHome {
            path,
            explicit: true,
        });
    }

    let path = dirs::home_dir()
        .ok_or("Could not find user home directory")?
        .join(".codex");
    #[cfg(test)]
    let path = super::super::codex_messaging::TEST_NATIVE_HOME
        .with(|home| home.borrow().clone())
        .unwrap_or(path);
    Ok(UpstreamHome {
        path,
        explicit: false,
    })
}
