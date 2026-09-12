//! Share one migrated Codex thread index between agent homes.
//!
//! `codex app-server` migrates legacy rollout files into paginated thread
//! history *before* it opens its control socket, reading them at roughly
//! 125 MB/s. Every agent home projects the same central `sessions/` tree but
//! starts with an empty thread database, so without this each new agent repeats
//! the entire migration and no agent reuses another's work. On a large history
//! that is the whole of a 50-90 second spawn.
//!
//! A snapshot of an already-migrated database is therefore published once and
//! seeded into new homes. Two properties make that safe to share:
//!
//! - Only tables that describe the shared rollout history are retained. The set
//!   is an allow-list, so a table this module has not been taught about stops
//!   publication rather than travelling between agents by default.
//! - `threads.rollout_path` is absolute and, as written by the provider, names
//!   the *publishing* agent's own projected `sessions/` directory. Left alone it
//!   would point every seeded agent at one agent's home, which dangles when that
//!   agent is removed. Publication rewrites those paths to the central tree that
//!   all homes project, so a snapshot names no agent.

use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

const CACHE_DIRECTORY: &[&str] = &["codex", "thread-state"];
const CACHE_LOCK_FILE: &str = ".wardian-thread-state.lock";
const SNAPSHOT_META_FILE: &str = "snapshot.json";
const SNAPSHOT_PREFIX: &str = "snapshot-";
const STAGING_SUFFIX: &str = ".staging";
const SNAPSHOT_VERSION: u32 = 1;

/// Tables whose rows describe the shared rollout history rather than one agent.
///
/// This is deliberately an allow-list. A provider release that adds a per-home
/// table would otherwise be copied between agents silently; instead publication
/// refuses and new agents rebuild until this list is updated.
const SHARED_TABLES: &[&str] = &[
    "_sqlx_migrations",
    "backfill_state",
    "rollout_migration_skipped_rollouts",
    "rollout_migration_state",
    "thread_artifacts",
    "thread_dynamic_tools",
    "thread_sections",
    "thread_spawn_edges",
    "threads",
];

/// Re-snapshot at most this often. A stale snapshot is harmless because the
/// provider migrates only the rollouts it has not already indexed, so a seeded
/// home pays for the delta rather than the whole history.
const REFRESH_INTERVAL_SECONDS: i64 = 6 * 60 * 60;

#[derive(Serialize, Deserialize)]
struct SnapshotMeta {
    version: u32,
    /// The provider's own database filename, which carries a schema generation
    /// (`state_5.sqlite`). The snapshot file is named after it, so content and
    /// name cannot disagree, and a generation change forces republication.
    database: String,
    captured_at: String,
    threads: i64,
}

fn cache_directory(wardian_home: &Path) -> PathBuf {
    CACHE_DIRECTORY
        .iter()
        .fold(wardian_home.to_path_buf(), |path, part| path.join(part))
}

fn snapshot_path(cache: &Path, database: &str) -> PathBuf {
    cache.join(format!("{SNAPSHOT_PREFIX}{database}"))
}

/// The generation number in a provider database filename, if it has one.
fn generation(name: &str) -> Option<u32> {
    name.strip_prefix("state_")
        .and_then(|rest| rest.strip_suffix(".sqlite"))
        .and_then(|digits| digits.parse::<u32>().ok())
}

/// The provider's live thread database inside one Codex home.
///
/// Picks the highest generation present: an upgrade can leave the previous
/// generation behind, and directory order is neither sorted nor stable.
fn state_database_name(codex_home: &Path) -> Option<String> {
    std::fs::read_dir(codex_home)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            generation(&name).map(|number| (number, name))
        })
        .max_by_key(|(number, _)| *number)
        .map(|(_, name)| name)
}

fn read_meta(cache: &Path) -> Option<SnapshotMeta> {
    let bytes = std::fs::read(cache.join(SNAPSHOT_META_FILE)).ok()?;
    let meta: SnapshotMeta = serde_json::from_slice(&bytes).ok()?;
    (meta.version == SNAPSHOT_VERSION).then_some(meta)
}

fn is_stale(meta: &SnapshotMeta) -> bool {
    let Ok(captured) = chrono::DateTime::parse_from_rfc3339(&meta.captured_at) else {
        return true;
    };
    (chrono::Utc::now() - captured.with_timezone(&chrono::Utc)).num_seconds()
        >= REFRESH_INTERVAL_SECONDS
}

/// Whether a published snapshot already covers this home's generation.
fn published(cache: &Path, database: &str) -> bool {
    read_meta(cache).is_some_and(|meta| {
        meta.database == database && snapshot_path(cache, database).is_file() && !is_stale(&meta)
    })
}

/// Create the cache under the same private-root rules as the rest of Codex
/// state, and refuse to use a directory that does not meet them.
fn private_cache(wardian_home: &Path) -> Result<PathBuf, String> {
    let cache = cache_directory(wardian_home);
    if let Some(parent) = cache.parent() {
        super::codex_home::create_private_root(parent)?;
    }
    super::codex_home::create_private_root(&cache)?;
    super::codex_home::validate_private_root(&cache)?;
    Ok(cache)
}

fn open_lock(cache: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cache.join(CACHE_LOCK_FILE))
        .map_err(|error| error.to_string())
}

/// Seed a new agent's Codex home with the published thread index.
///
/// Returns whether a seed was written. A home that already has a thread
/// database is left alone: the provider owns it from that point on. Call this
/// only while holding the agent's preparation lock and before its daemon
/// starts, so the copy cannot race the provider creating its own database.
pub(crate) fn seed(wardian_home: &Path, codex_home: &Path) -> Result<bool, String> {
    if state_database_name(codex_home).is_some() {
        return Ok(false);
    }
    let cache = cache_directory(wardian_home);
    if !cache.is_dir() {
        return Ok(false);
    }
    super::codex_home::validate_private_root(&cache)?;
    let Some(meta) = read_meta(&cache) else {
        return Ok(false);
    };
    // The name is read back from disk, so it must stay inside the home.
    if Path::new(&meta.database).components().count() != 1 || generation(&meta.database).is_none() {
        return Err("Published Codex thread index has an unusable database name".into());
    }
    let snapshot = snapshot_path(&cache, &meta.database);
    if !snapshot.is_file() {
        return Ok(false);
    }

    // Hold the lock shared so a concurrent publication cannot replace the file
    // underneath this copy.
    // Seeding is best effort and runs on the spawn path, so never wait: the
    // exclusive holder is a publication doing two full VACUUMs. A skipped seed
    // costs one rebuild, which is the documented fallback.
    let lock = open_lock(&cache)?;
    if lock.try_lock_shared().is_err() {
        return Ok(false);
    }
    let outcome = write_seed(&snapshot, &codex_home.join(&meta.database));
    let _ = FileExt::unlock(&lock);
    outcome
}

/// Copy the snapshot in without ever replacing an existing database.
fn write_seed(snapshot: &Path, target: &Path) -> Result<bool, String> {
    let mut source = std::fs::File::open(snapshot).map_err(|error| error.to_string())?;
    let mut destination = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
    {
        Ok(file) => file,
        // The provider created its own database first; it owns this home now.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    // The provider recreates its write-ahead log and shared-memory files, so a
    // single complete database file is a sufficient seed.
    match std::io::copy(&mut source, &mut destination) {
        Ok(_) => Ok(true),
        Err(error) => {
            drop(destination);
            let _ = std::fs::remove_file(target);
            Err(error.to_string())
        }
    }
}

/// Publish a snapshot of this home's thread index for future agents.
///
/// Best effort: a failure here costs the next agent a rebuild, never this
/// agent's launch. Skips quietly when the current generation is already
/// published and recent.
pub(crate) fn refresh(
    wardian_home: &Path,
    codex_home: &Path,
    real_codex_home: &Path,
) -> Result<bool, String> {
    let Some(database) = state_database_name(codex_home) else {
        return Ok(false);
    };
    let cache = cache_directory(wardian_home);
    if cache.is_dir() && published(&cache, &database) {
        return Ok(false);
    }
    let cache = private_cache(wardian_home)?;
    let lock = open_lock(&cache)?;
    lock.lock_exclusive().map_err(|error| error.to_string())?;
    let outcome = publish(&cache, codex_home, &database, real_codex_home);
    let _ = FileExt::unlock(&lock);
    outcome
}

fn publish(
    cache: &Path,
    codex_home: &Path,
    database: &str,
    real_codex_home: &Path,
) -> Result<bool, String> {
    // Another launch may have published while this one waited for the lock.
    if published(cache, database) {
        return Ok(false);
    }
    // An interrupted publication leaves a full-size staging file behind and
    // nothing else reclaims it. The lock guarantees none is in flight now.
    sweep_staging(cache);

    let staging = snapshot_path(cache, database).with_extension(format!(
        "{}{STAGING_SUFFIX}",
        snapshot_path(cache, database)
            .extension()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let threads = match capture(&codex_home.join(database), &staging, real_codex_home) {
        Ok(threads) => threads,
        Err(error) => {
            let _ = std::fs::remove_file(&staging);
            return Err(error);
        }
    };

    let meta = SnapshotMeta {
        version: SNAPSHOT_VERSION,
        database: database.to_owned(),
        captured_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        threads,
    };
    let encoded = serde_json::to_vec_pretty(&meta).map_err(|error| error.to_string())?;
    let meta_staging = cache.join(format!("{SNAPSHOT_META_FILE}{STAGING_SUFFIX}"));
    std::fs::write(&meta_staging, encoded).map_err(|error| error.to_string())?;

    // Publish the database first: a reader that sees this metadata must find
    // the file it names already complete. The snapshot carries its generation
    // in its own filename, so the two can never describe different schemas.
    if let Err(error) = std::fs::rename(&staging, snapshot_path(cache, database)) {
        let _ = std::fs::remove_file(&staging);
        let _ = std::fs::remove_file(&meta_staging);
        return Err(error.to_string());
    }
    std::fs::rename(&meta_staging, cache.join(SNAPSHOT_META_FILE))
        .map_err(|error| error.to_string())?;
    // A generation bump would otherwise orphan a full-size index forever.
    sweep_superseded(cache, database);
    Ok(true)
}

/// Remove published snapshots other than the one just written.
fn sweep_superseded(cache: &Path, database: &str) {
    let keep = snapshot_path(cache, database);
    let Ok(entries) = std::fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(SNAPSHOT_PREFIX) && path != keep {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn sweep_staging(cache: &Path) {
    let Ok(entries) = std::fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(STAGING_SUFFIX) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Copy a live thread database, keep only shared rows, and make its rollout
/// references name the central tree rather than the publishing agent's home.
///
/// `VACUUM INTO` is used rather than a file copy because agent daemons stay
/// resident with the database open and a write-ahead log active; copying those
/// files out from under a live writer yields a torn snapshot.
fn capture(source: &Path, target: &Path, real_codex_home: &Path) -> Result<i64, String> {
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
    refuse_unknown_tables(&writer)?;
    let rewritten = canonicalize_rollout_paths(&writer, real_codex_home)?;
    // The invariant is that no published row names an agent home. Rows whose
    // path could not be split are dropped rather than shipped: the seeded agent
    // re-migrates those few rollouts, and one odd path cannot disable the cache.
    let root = canonical_root(real_codex_home);
    let stranded = writer
        .execute(
            "DELETE FROM threads WHERE rollout_path IS NULL OR substr(rollout_path, 1, ?2) <> ?1",
            rusqlite::params![root.as_str(), root.chars().count() as i64],
        )
        .map_err(|error| format!("could not drop unrewritable rollout paths: {error}"))?;
    let threads: i64 = writer
        .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
        .unwrap_or(0);
    let _ = (rewritten, stranded);
    // Reclaim pages the rewrite freed so the seed stays small.
    writer
        .execute("VACUUM", [])
        .map_err(|error| format!("could not compact Codex thread snapshot: {error}"))?;
    Ok(threads)
}

/// Refuse to publish a schema this module has not been taught about.
fn refuse_unknown_tables(connection: &rusqlite::Connection) -> Result<(), String> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .map_err(|error| error.to_string())?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for name in names {
        // SQLite's own bookkeeping (sqlite_sequence, sqlite_stat1 after an
        // ANALYZE) cannot carry agent data and must not stop publication.
        if name.starts_with("sqlite_") || SHARED_TABLES.contains(&name.as_str()) {
            continue;
        }
        let rows: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM \"{name}\""), [], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        if rows > 0 {
            return Err(format!(
                "Codex thread index has unshared table {name} with {rows} row(s); \
not publishing until it is classified"
            ));
        }
        connection
            .execute(&format!("DELETE FROM \"{name}\""), [])
            .map_err(|error| format!("could not clear {name} from snapshot: {error}"))?;
    }
    Ok(())
}

/// Rewrite `<any home>/sessions/<rest>` to `<real codex home>/sessions/<rest>`.
///
/// Every home projects the same central tree, so the tail after `sessions` is
/// the shared identity of a rollout; only the prefix is agent-specific.
fn canonicalize_rollout_paths(
    connection: &rusqlite::Connection,
    real_codex_home: &Path,
) -> Result<i64, String> {
    let canonical_root = real_codex_home.join("sessions");
    let mut statement = connection
        .prepare("SELECT id, rollout_path FROM threads")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;

    let mut rewritten = 0;
    for (id, rollout_path) in rows {
        let Some(tail) = sessions_tail(&rollout_path) else {
            continue;
        };
        let canonical = tail
            .iter()
            .fold(canonical_root.clone(), |path, part| path.join(part))
            .to_string_lossy()
            .into_owned();
        if canonical == rollout_path {
            continue;
        }
        connection
            .execute(
                "UPDATE threads SET rollout_path = ?1 WHERE id = ?2",
                [canonical.as_str(), id.as_str()],
            )
            .map_err(|error| format!("could not canonicalize a rollout path: {error}"))?;
        rewritten += 1;
    }
    Ok(rewritten)
}

/// The central sessions directory every home is expected to project, as the
/// string form used for both rewriting and the published-row invariant.
fn canonical_root(real_codex_home: &Path) -> String {
    real_codex_home
        .join("sessions")
        .to_string_lossy()
        .into_owned()
}

/// Whether this home's `sessions` entry really resolves to the central tree.
///
/// The projection falls back to a private local directory when a link cannot be
/// created. A home on that fallback must not keep a seed whose rows all name the
/// central tree, because its migration-state rows would stop it rebuilding.
pub(crate) fn projects_central_sessions(codex_home: &Path, real_codex_home: &Path) -> bool {
    let resolve = |path: &Path| std::fs::canonicalize(path).ok();
    match (
        resolve(&codex_home.join("sessions")),
        resolve(&real_codex_home.join("sessions")),
    ) {
        (Some(projected), Some(central)) => projected == central,
        // Nothing to contradict: leave the decision to the caller's default.
        _ => true,
    }
}

/// Remove a seed this call wrote after discovering the home cannot use it.
pub(crate) fn discard_seed(wardian_home: &Path, codex_home: &Path) {
    let cache = cache_directory(wardian_home);
    let Some(meta) = read_meta(&cache) else {
        return;
    };
    let _ = std::fs::remove_file(codex_home.join(&meta.database));
}

/// The components of a rollout path below its `sessions` directory.
///
/// The provider writes whichever separator its platform uses, so the tail is
/// returned as components and rejoined through `PathBuf`; producing a string
/// with one platform's separator would yield a single filename on the other.
fn sessions_tail(rollout_path: &str) -> Option<Vec<String>> {
    let lowered = rollout_path.to_ascii_lowercase();
    let mut search_from = lowered.len();
    while let Some(marker) = lowered[..search_from].rfind("sessions") {
        search_from = marker;
        let after = marker + "sessions".len();
        // The component must be exactly `sessions`, not a name ending in it.
        let separator = |byte: Option<&u8>| matches!(byte, Some(b'\\') | Some(b'/'));
        let starts_component = marker == 0 || separator(rollout_path.as_bytes().get(marker - 1));
        if !starts_component || !separator(rollout_path.as_bytes().get(after)) {
            continue;
        }
        let components: Vec<String> = rollout_path[after..]
            .split(['\\', '/'])
            .filter(|part| !part.is_empty() && *part != ".")
            .map(str::to_owned)
            .collect();
        // A traversal component would let a rewritten path escape the tree.
        if components.is_empty() || components.iter().any(|part| part == "..") {
            return None;
        }
        return Some(components);
    }
    None
}

#[cfg(test)]
#[path = "codex_thread_state/tests.rs"]
mod tests;
