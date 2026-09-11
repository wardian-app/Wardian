//! Share one migrated Codex thread index between agent homes.
//!
//! `codex app-server` migrates legacy rollout files into paginated thread
//! history *before* it opens its control socket, reading them at roughly
//! 125 MB/s. Every agent home projects the same central `sessions/` tree but
//! starts with an empty thread database, so without this each new agent repeats
//! the entire migration and no agent reuses another's work. On a large history
//! that is the whole of a 50-90 second spawn.
//!
//! A snapshot of an already-migrated database is therefore cached once and
//! seeded into new homes. The index only describes rollouts every agent already
//! reads through the shared projection, so sharing it exposes nothing new;
//! genuinely per-agent rows are cleared before the snapshot is published.

use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

const CACHE_DIRECTORY: &[&str] = &["codex", "thread-state"];
const CACHE_LOCK_FILE: &str = ".wardian-thread-state.lock";
const SNAPSHOT_FILE: &str = "snapshot.sqlite";
const SNAPSHOT_META_FILE: &str = "snapshot.json";
const SNAPSHOT_VERSION: u32 = 1;

/// Rows that describe one agent rather than the shared rollout history. These
/// are cleared before publication so a snapshot can never carry one agent's
/// projects, workspace roots or remote-control enrolments into another's home.
const PER_AGENT_TABLES: &[&str] = &[
    "projects",
    "project_roots",
    "project_idempotency_keys",
    "remote_control_enrollments",
    "external_agent_config_imports",
];

/// Re-snapshot at most this often. A stale snapshot is harmless because the
/// provider migrates only the rollouts it has not already indexed, so a seeded
/// home pays for the delta rather than the whole history.
const REFRESH_INTERVAL_SECONDS: i64 = 6 * 60 * 60;

#[derive(Serialize, Deserialize)]
struct SnapshotMeta {
    version: u32,
    /// The provider's own database filename, which carries a schema generation
    /// (`state_5.sqlite`). Seeding reuses the captured name so an upgraded
    /// provider that moves to a new generation simply finds no seed and
    /// rebuilds once, after which the cache follows it.
    database: String,
    captured_at: String,
    threads: i64,
}

fn cache_directory(wardian_home: &Path) -> PathBuf {
    CACHE_DIRECTORY
        .iter()
        .fold(wardian_home.to_path_buf(), |path, part| path.join(part))
}

/// The provider's thread database inside one Codex home, if it has one yet.
fn state_database_name(codex_home: &Path) -> Option<String> {
    let entries = std::fs::read_dir(codex_home).ok()?;
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| {
            name.strip_prefix("state_")
                .and_then(|rest| rest.strip_suffix(".sqlite"))
                .is_some_and(|digits| {
                    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
                })
        })
}

fn read_meta(cache: &Path) -> Option<SnapshotMeta> {
    let bytes = std::fs::read(cache.join(SNAPSHOT_META_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Hold the cache lock for the duration of a publication. Readers copy a
/// complete file that was renamed into place, so they need no lock.
fn lock_cache(cache: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(cache).map_err(|error| error.to_string())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cache.join(CACHE_LOCK_FILE))
        .map_err(|error| error.to_string())?;
    file.lock_exclusive().map_err(|error| error.to_string())?;
    Ok(file)
}

/// Seed a new agent's Codex home with the cached thread index.
///
/// Returns whether a seed was written. A home that already has a thread
/// database is left alone: the provider owns it from that point on.
pub(crate) fn seed(wardian_home: &Path, codex_home: &Path) -> Result<bool, String> {
    if state_database_name(codex_home).is_some() {
        return Ok(false);
    }
    let cache = cache_directory(wardian_home);
    let Some(meta) = read_meta(&cache) else {
        return Ok(false);
    };
    if meta.version != SNAPSHOT_VERSION {
        return Ok(false);
    }
    let snapshot = cache.join(SNAPSHOT_FILE);
    if !snapshot.is_file() {
        return Ok(false);
    }
    // Never let a captured name escape the home; the provider chooses it, but
    // this value is read back from disk.
    if Path::new(&meta.database).components().count() != 1 {
        return Err("Cached Codex thread database name is not a single component".into());
    }
    let target = codex_home.join(&meta.database);
    // The provider recreates its write-ahead log and shared-memory files, so a
    // single complete database file is a sufficient seed.
    std::fs::copy(&snapshot, &target).map_err(|error| {
        format!(
            "could not seed Codex thread index into {}: {error}",
            codex_home.display()
        )
    })?;
    Ok(true)
}

/// Publish a snapshot of this home's thread index for future agents.
///
/// Best effort: a failure here costs the next agent a rebuild, never this
/// agent's launch. Skips quietly when a recent snapshot already exists.
pub(crate) fn refresh(wardian_home: &Path, codex_home: &Path) -> Result<bool, String> {
    let Some(database) = state_database_name(codex_home) else {
        return Ok(false);
    };
    let cache = cache_directory(wardian_home);
    if let Some(meta) = read_meta(&cache) {
        if meta.version == SNAPSHOT_VERSION
            && cache.join(SNAPSHOT_FILE).is_file()
            && !is_stale(&meta)
        {
            return Ok(false);
        }
    }
    let _lock = lock_cache(&cache)?;
    // Another launch may have published while this one waited for the lock.
    if let Some(meta) = read_meta(&cache) {
        if meta.version == SNAPSHOT_VERSION
            && cache.join(SNAPSHOT_FILE).is_file()
            && !is_stale(&meta)
        {
            return Ok(false);
        }
    }

    let staging = cache.join(format!("{SNAPSHOT_FILE}.{}", std::process::id()));
    let _ = std::fs::remove_file(&staging);
    let threads = capture(&codex_home.join(&database), &staging).inspect_err(|_| {
        let _ = std::fs::remove_file(&staging);
    })?;

    let meta = SnapshotMeta {
        version: SNAPSHOT_VERSION,
        database,
        captured_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        threads,
    };
    let encoded = serde_json::to_vec_pretty(&meta).map_err(|error| error.to_string())?;
    // Publish the database first: a reader that sees fresh metadata must find
    // the file it describes already complete.
    std::fs::rename(&staging, cache.join(SNAPSHOT_FILE)).map_err(|error| {
        let _ = std::fs::remove_file(&staging);
        error.to_string()
    })?;
    std::fs::write(cache.join(SNAPSHOT_META_FILE), encoded).map_err(|error| error.to_string())?;
    Ok(true)
}

fn is_stale(meta: &SnapshotMeta) -> bool {
    let Ok(captured) = chrono::DateTime::parse_from_rfc3339(&meta.captured_at) else {
        return true;
    };
    (chrono::Utc::now() - captured.with_timezone(&chrono::Utc)).num_seconds()
        >= REFRESH_INTERVAL_SECONDS
}

/// Copy a live thread database and strip its per-agent rows.
///
/// `VACUUM INTO` is used rather than a file copy because agent daemons stay
/// resident with the database open and a write-ahead log active; copying those
/// files out from under a live writer yields a torn snapshot.
fn capture(source: &Path, target: &Path) -> Result<i64, String> {
    let reader = rusqlite::Connection::open_with_flags(
        source,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("could not open {}: {error}", source.display()))?;
    reader
        .execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])
        .map_err(|error| format!("could not snapshot Codex thread index: {error}"))?;
    drop(reader);

    let writer = rusqlite::Connection::open(target)
        .map_err(|error| format!("could not open Codex thread snapshot: {error}"))?;
    for table in PER_AGENT_TABLES {
        let present: i64 = writer
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if present > 0 {
            writer
                .execute(&format!("DELETE FROM \"{table}\""), [])
                .map_err(|error| format!("could not clear {table} from snapshot: {error}"))?;
        }
    }
    let threads = writer
        .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
        .unwrap_or(0);
    // Reclaim the pages the deletes freed so the seed stays small.
    writer
        .execute("VACUUM", [])
        .map_err(|error| format!("could not compact Codex thread snapshot: {error}"))?;
    Ok(threads)
}

#[cfg(test)]
#[path = "codex_thread_state/tests.rs"]
mod tests;
