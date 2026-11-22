//! Agent-local Codex MCP registration. Never edits the user's global Codex home
//! or provider approval policy; shared bootstrap homes have no registration seam.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Component, Path};
use toml_edit::{value, Array, DocumentMut, Item, Table};

const SERVER: &str = "wardian";
const RECORD: &str = ".wardian-messaging.json";

/// Preserve every existing local Wardian entry during upstream reconciliation.
/// This is not an ownership grant: user edits invalidate `owns`, but remain
/// authoritative over a conflicting global entry (and stale-runtime pruning).
pub(super) fn local_registration(servers: &Item) -> Option<(&'static str, Item)> {
    servers.get(SERVER).map(|entry| (SERVER, entry.clone()))
}

/// A selected agent's MCP settings before upstream reconciliation can replace them.
pub(crate) struct McpProfile {
    servers: Vec<(String, Item)>,
    messaging_removed: bool,
}

impl McpProfile {
    pub(crate) fn load(wardian_home: &Path, profile_id: &str) -> Result<Option<Self>, String> {
        let Some(home) = super::codex_home::resolve_prepared_profile(wardian_home, profile_id)?
        else {
            return Ok(None);
        };
        let path = home.join("config.toml");
        reject_link(&path)?;
        let config = read_optional(&path)?
            .unwrap_or_default()
            .parse::<DocumentMut>()
            .map_err(|_| "Could not parse selected agent Codex config.toml")?;
        let servers = config
            .get("mcp_servers")
            .map(|item| {
                item.as_table_like()
                    .ok_or("Selected agent Codex mcp_servers is not a table")
                    .map(|table| {
                        table
                            .iter()
                            .map(|(name, server)| (name.to_owned(), server.clone()))
                            .collect::<Vec<_>>()
                    })
            })
            .transpose()?
            .unwrap_or_default();
        let record = home.join(RECORD);
        reject_link(&record)?;
        let messaging_removed = !servers.iter().any(|(name, _)| name == SERVER) && record.is_file();
        Ok((!servers.is_empty() || messaging_removed).then_some(Self {
            servers,
            messaging_removed,
        }))
    }
}

/// Whether the launch keeps the selected agent's identity, uses a fresh run
/// identity, or uses the shared bootstrap path without local messaging.
#[derive(Clone, Copy)]
pub(crate) enum ProfileTarget<'a> {
    Resume,
    Fresh {
        wardian_home: &'a Path,
        run_id: &'a str,
    },
    Bootstrap,
}

#[derive(Clone, Copy)]
pub(crate) struct ProfileProjection<'a> {
    pub profile: &'a McpProfile,
    pub target: ProfileTarget<'a>,
}

impl ProfileProjection<'_> {
    pub(super) fn inherits_messaging(&self) -> bool {
        matches!(self.target, ProfileTarget::Fresh { .. })
            && (self.profile.messaging_removed
                || self.profile.servers.iter().any(|(name, _)| name == SERVER))
    }

    /// Restore complete source entries, including nested tool approval, env and
    /// cwd. The two Wardian identity variables belong to the new run.
    pub(super) fn apply(&self, config: &mut DocumentMut) -> Result<(), String> {
        if config.get("mcp_servers").is_none() {
            config["mcp_servers"] = Item::Table(Table::new());
        }
        let servers = config["mcp_servers"]
            .as_table_like_mut()
            .ok_or("Launch Codex mcp_servers is not a table")?;
        if self.profile.messaging_removed || matches!(self.target, ProfileTarget::Bootstrap) {
            servers.remove(SERVER);
        }
        for (name, server) in &self.profile.servers {
            let mut server = server.clone();
            if name == SERVER {
                match self.target {
                    ProfileTarget::Bootstrap => continue,
                    ProfileTarget::Resume => {}
                    ProfileTarget::Fresh {
                        wardian_home,
                        run_id,
                    } => {
                        if let Some(env) = server.get_mut("env").and_then(Item::as_table_like_mut) {
                            if let Some(home) = env.get_mut("WARDIAN_HOME") {
                                *home = value(path_text(wardian_home)?);
                            }
                            if let Some(session) = env.get_mut("WARDIAN_SESSION_ID") {
                                *session = value(run_id);
                            }
                        }
                    }
                }
            }
            servers.insert(name, server);
        }
        Ok(())
    }

    pub(crate) fn write_bootstrap_config(&self, destination: &Path) -> Result<(), String> {
        let path = destination.join("config.toml");
        reject_link(&path)?;
        let mut config = read_optional(&path)?
            .unwrap_or_default()
            .parse::<DocumentMut>()
            .map_err(|_| "Could not parse bootstrap Codex config.toml")?;
        self.apply(&mut config)?;
        atomic_replace(&path, config.to_string().as_bytes())
    }
}

#[cfg(test)]
thread_local! {
    // No production environment override: isolate the real preparation path
    // without ever projecting an actual user's global Codex home in tests.
    pub(crate) static TEST_NATIVE_HOME: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Registration {
    Updated,
    Unchanged,
    Unavailable(&'static str),
}

pub(super) fn messaging_server_file_name() -> &'static str {
    if cfg!(windows) {
        super::cli_install::bundled_mcp_file_name()
    } else {
        super::cli_install::bundled_cli_file_name()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Ownership {
    version: u32,
    agent_id: String,
    command: String,
    wardian_home: String,
}

/// Register the bundled native CLI in one already-prepared agent home.
///
/// Context comes from habitat preparation, not inherited provider environment.
/// A separate ownership record authorizes updates only while the command, args
/// and identity fields still match our last write. User collisions, edits and
/// deletion are preserved. Additional settings (including disabled state and
/// per-tool approval) are never managed. No tool approval is granted here.
///
/// Missing CLI is recoverable on a later preparation. Invalid TOML, linked
/// targets and I/O failures are errors, never reasons to replace user config.
pub(crate) fn ensure_managed_messaging(
    wardian_home: &Path,
    agent_id: &str,
) -> Result<Registration, String> {
    if !wardian_home.is_absolute()
        || agent_id.trim() != agent_id
        || !matches!(
            Path::new(agent_id)
                .components()
                .collect::<Vec<_>>()
                .as_slice(),
            [Component::Normal(_)]
        )
        || agent_id.contains(['/', '\\'])
    {
        return Err(
            "Managed messaging requires an absolute home and one agent ID component".into(),
        );
    }
    let home = wardian_home.canonicalize().map_err(|e| e.to_string())?;
    // A compact home is writable only through Wardian's recorded mapping.
    // The resolver still rejects arbitrary habitat links and pending migration.
    let codex_home = super::codex_home::resolve_managed_home(wardian_home, agent_id)?;
    if !codex_home.is_dir() {
        return Err("Managed Codex habitat must be prepared before MCP registration".into());
    }
    let cli = home.join("bin").join(messaging_server_file_name());
    if !cli.is_file() {
        return Ok(Registration::Unavailable("bundled native CLI is missing"));
    }
    let desired = Ownership {
        version: 1,
        agent_id: agent_id.to_owned(),
        command: path_text(&cli.canonicalize().map_err(|e| e.to_string())?)?,
        // Control endpoint identity hashes the runtime's home spelling. Keep
        // that exact value for the child environment; canonical paths above
        // are only for filesystem ownership and link checks.
        wardian_home: path_text(wardian_home)?,
    };
    let config_path = codex_home.join("config.toml");
    let record_path = codex_home.join(RECORD);
    reject_link(&config_path)?;
    reject_link(&record_path)?;
    let original = read_optional(&config_path)?.unwrap_or_default();
    let mut config = original.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    let record = read_optional(&record_path)?;
    let previous: Option<Ownership> = record
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| e.to_string())?;
    let existing = config
        .get("mcp_servers")
        .and_then(|servers| servers.get(SERVER));
    match (existing, previous.as_ref()) {
        (Some(entry), Some(owner))
            if owner.version == 1 && owner.agent_id == agent_id && owns(entry, owner) => {}
        (None, None) => {}
        _ => {
            return Ok(Registration::Unavailable(
                "user MCP entry, edit or removal preserved",
            ))
        }
    }
    if let Some(servers) = config.get("mcp_servers") {
        if !servers.is_table() {
            return Ok(Registration::Unavailable(
                "non-table MCP configuration preserved",
            ));
        }
    }
    if existing.is_some_and(|entry| owns(entry, &desired)) {
        return Ok(Registration::Unchanged);
    }
    if config.get("mcp_servers").is_none() {
        config["mcp_servers"] = Item::Table(Table::new());
    }
    let servers = config["mcp_servers"]
        .as_table_mut()
        .expect("checked MCP table");
    if previous.is_none() {
        let mut entry = Table::new();
        entry["command"] = value(&desired.command);
        let mut args = Array::new();
        args.push("mcp");
        args.push("serve");
        entry["args"] = value(args);
        // Allow the v2 receive transport's 61-second maximum plus overhead.
        // Existing timeout values remain user settings, not owned fields.
        entry["tool_timeout_sec"] = value(70);
        let mut env = Table::new();
        env["WARDIAN_HOME"] = value(&desired.wardian_home);
        env["WARDIAN_SESSION_ID"] = value(&desired.agent_id);
        entry["env"] = Item::Table(env);
        servers[SERVER] = Item::Table(entry);
    } else {
        // Replace only changed string values, retaining their TOML decoration.
        let entry = servers[SERVER]
            .as_table_like_mut()
            .expect("owned server table");
        replace_string(
            entry.get_mut("command").expect("owned command"),
            &desired.command,
        );
        let env = entry
            .get_mut("env")
            .and_then(Item::as_table_like_mut)
            .expect("owned env");
        replace_string(
            env.get_mut("WARDIAN_HOME").expect("owned home"),
            &desired.wardian_home,
        );
        replace_string(
            env.get_mut("WARDIAN_SESSION_ID").expect("owned agent"),
            &desired.agent_id,
        );
    }
    // Avoid overwriting an edit observed during preparation. External editors
    // do not share a lock; this is a bounded compare-before-publish check.
    if read_optional(&config_path)?.unwrap_or_default() != original
        || read_optional(&record_path)? != record
    {
        return Err("Codex messaging config changed during preparation".into());
    }
    let rendered = config.to_string();
    let rendered = if original.contains("\r\n") {
        rendered.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        rendered
    };
    atomic_replace(&config_path, rendered.as_bytes())?;
    // Config first: interruption before recording ownership leaves a preserved
    // collision on retry, never permission to adopt an unowned user entry.
    atomic_replace(
        &record_path,
        &serde_json::to_vec(&desired).map_err(|e| e.to_string())?,
    )?;
    Ok(Registration::Updated)
}

fn owns(entry: &Item, owner: &Ownership) -> bool {
    entry.get("command").and_then(Item::as_str) == Some(owner.command.as_str())
        && entry
            .get("args")
            .and_then(Item::as_array)
            .is_some_and(|args| {
                args.len() == 2
                    && args.get(0).and_then(toml_edit::Value::as_str) == Some("mcp")
                    && args.get(1).and_then(toml_edit::Value::as_str) == Some("serve")
            })
        && entry.get("env").is_some_and(|env| {
            env.get("WARDIAN_HOME").and_then(Item::as_str) == Some(owner.wardian_home.as_str())
                && env.get("WARDIAN_SESSION_ID").and_then(Item::as_str)
                    == Some(owner.agent_id.as_str())
        })
}

fn replace_string(item: &mut Item, replacement: &str) {
    if item.as_str() == Some(replacement) {
        return;
    }
    let decoration = item.as_value().expect("owned string").decor().clone();
    *item = value(replacement);
    *item.as_value_mut().expect("replacement string").decor_mut() = decoration;
}

fn path_text(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "MCP path is not UTF-8".into())
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn reject_link(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if super::fs::is_directory_link(&metadata) => {
            Err("Linked managed Codex messaging path is not writable".into())
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
thread_local! {
    static FAIL_BEFORE_REPLACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Sync a complete replacement before publishing it, including on long Windows paths.
pub(crate) fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().expect("managed file parent");
    // tempfile persists through Win32 APIs. Keep the existing parent's verbatim
    // spelling for both paths, including in executables without longPathAware.
    // Resolve only the parent: the destination may not exist yet.
    #[cfg(windows)]
    let parent = parent.canonicalize().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let destination = parent.join(path.file_name().expect("managed file name"));
    #[cfg(not(windows))]
    let destination = path;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temporary.write_all(bytes).map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    #[cfg(test)]
    if FAIL_BEFORE_REPLACE.with(|fault| fault.replace(false)) {
        return Err("injected failure before atomic replace".into());
    }
    temporary.persist(destination).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests;
