//! Read-only, bounded native database observations for the recent-header owner.
//! A LIMIT bounds returned rows only; the shared VM interrupt and payload budget
//! also bound attempted work. Unsupported/expensive layouts remain a safe miss.
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use rusqlite::{Connection, OpenFlags, StatementStatus};
use serde::Serialize;
use wardian_core::models::chat::{AgentChatEvent, AgentChatEventKind};

use super::provider_log_acquisition::{native_file_identity, ProviderLogNativeIdentity};

const ROWS: usize = 80;
const PAYLOAD_BYTES: usize = 128 * 1024;
const VALUE_BYTES: usize = 16 * 1024;
const VM_QUANTUM: i32 = 100;
const VM_QUANTA: usize = 1000;
const OPENCODE_QUERIES: [&str; 2] = [
    "SELECT p.rowid,p.id,p.time_created,m.rowid,m.id,m.time_created FROM part p JOIN message m ON m.id=p.message_id WHERE p.session_id=?1 AND m.session_id=?1 AND p.time_created IS NOT NULL ORDER BY p.time_created DESC,p.id DESC LIMIT 80",
    "SELECT p.rowid,p.id,p.time_created,m.rowid,m.id,m.time_created FROM part p JOIN message m ON m.id=p.message_id WHERE p.session_id=?1 AND m.session_id=?1 AND p.time_created IS NULL ORDER BY m.time_created DESC,p.id DESC LIMIT 80",
];

pub(crate) const MESSAGE_CAPTURE_BINDING: &str = "opencode_message_capture";

/// Non-serializable authority produced only by the actual full DB query and
/// normal projector. Provider/Watch metadata cannot construct this receipt.
pub(crate) struct MessageAuthority {
    source: Source,
    _native_guard: File,
    connection: Connection,
    data_version: i64,
    agent: String,
    session: String,
    observations: std::collections::HashMap<String, String>,
}

pub(super) struct FullOpenCodeCapture {
    pub(super) events: Vec<AgentChatEvent>,
    pub(super) authority: MessageAuthority,
}

fn database_version(connection: &Connection) -> io::Result<i64> {
    connection
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .map_err(io::Error::other)
}

fn message_observation(event: &AgentChatEvent) -> io::Result<String> {
    Ok(super::chat_recent_seed::hash(
        &serde_json::to_vec(&(
            &event.id,
            &event.session_id,
            &event.provider,
            &event.kind,
            &event.role,
            &event.turn_id,
            &event.created_at,
            &event.source,
            &event.text,
            (
                &event.metadata["opencode_session_id"],
                &event.metadata["part_id"],
                &event.metadata["chat_recent_native_key"],
                &event.metadata["chat_recent_native_version"],
                &event.metadata["source_path"],
                &event.metadata["input_origin"],
                &event.metadata["input_purpose"],
                &event.metadata["request_root_id"],
            ),
        ))
        .map_err(io::Error::other)?,
    ))
}

/// Full capture retains its existing payload/order contract. The descriptor
/// guard binds SQLite's name-based open; data_version also fences an older
/// acquired snapshot from overwriting a later committed native version.
pub(super) fn capture_opencode(
    path: &Path,
    agent: &str,
    session: &str,
) -> io::Result<FullOpenCodeCapture> {
    let guard = database_guard(path)?;
    let source = Source::open(path)?;
    if native_file_identity(&guard)? != source.identity {
        return Err(io::Error::other("native capture source changed"));
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(io::Error::other)?;
    connection
        .busy_timeout(std::time::Duration::ZERO)
        .map_err(io::Error::other)?;
    let data_version = database_version(&connection)?;
    let events = super::chat::project_opencode_database_connection(
        &connection,
        path,
        agent,
        session,
        Some(&source),
    )
    .map_err(io::Error::other)?;
    if database_version(&connection)? != data_version || !source.valid()? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "native capture changed during acquisition",
        ));
    }
    let observations = events
        .iter()
        .filter(|event| {
            event.kind == AgentChatEventKind::Message
                && event.source.as_deref() == Some("opencode_db")
        })
        .map(|event| Ok((event.id.clone(), message_observation(event)?)))
        .collect::<io::Result<_>>()?;
    Ok(FullOpenCodeCapture {
        events,
        authority: MessageAuthority {
            source,
            _native_guard: guard,
            connection,
            data_version,
            agent: agent.into(),
            session: session.into(),
            observations,
        },
    })
}

impl MessageAuthority {
    /// Actual acquired IDs and native-binding fields identify DB claims; an
    /// unbound Watch fallback is not silently promoted to a DB observation.
    pub(crate) fn claims(&self, event: &AgentChatEvent) -> bool {
        self.observations.contains_key(&event.id)
            || (event.kind == AgentChatEventKind::Message
                && event.source.as_deref() == Some("opencode_db")
                && [
                    "opencode_session_id",
                    "part_id",
                    "chat_recent_native_key",
                    "chat_recent_native_version",
                ]
                .iter()
                .any(|field| !event.metadata[*field].is_null()))
    }

    pub(crate) fn require_current(&self) -> io::Result<()> {
        if self.is_current()? {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "native message capture retired before archive publication",
            ))
        }
    }
    pub(crate) fn is_current(&self) -> io::Result<bool> {
        Ok(self.source.valid()? && database_version(&self.connection)? == self.data_version)
    }

    pub(crate) fn proves(
        &self,
        context: &crate::state::conversation_archive::ConversationArchiveContext,
        event: &AgentChatEvent,
    ) -> bool {
        context.agent_id == self.agent
            && context.provider == "opencode"
            && context.provider_source_key.as_deref()
                == Some(format!("opencode:session:{}", self.session).as_str())
            && event.session_id == self.agent
            && event.provider == "opencode"
            && event.kind == AgentChatEventKind::Message
            && event.source.as_deref() == Some("opencode_db")
            && event.metadata["opencode_session_id"].as_str() == Some(self.session.as_str())
            && event
                .turn_id
                .as_deref()
                .is_some_and(|id| !id.is_empty() && id.len() <= 256)
            && event.metadata["part_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty() && id.len() <= 256)
            && self.observations.get(&event.id).is_some_and(|observed| {
                message_observation(event).is_ok_and(|current| &current == observed)
            })
    }

    pub(crate) fn binding(&self, event: &AgentChatEvent) -> serde_json::Value {
        serde_json::json!({ "source": self.source.key("opencode", &self.session, "message-capture"),
            "native_key": event.metadata["chat_recent_native_key"],
            "version": event.metadata["chat_recent_native_version"],
            "session": self.session, "part": event.metadata["part_id"], "message": event.turn_id })
    }

    pub(crate) fn permits_refresh(&self, old: &AgentChatEvent, current: &AgentChatEvent) -> bool {
        let prior = &old.metadata[MESSAGE_CAPTURE_BINDING];
        let next = self.binding(current);
        old.id == current.id
            && old.session_id == current.session_id
            && old.provider == current.provider
            && old.kind == current.kind
            && old.role == current.role
            && old.turn_id == current.turn_id
            && old.created_at == current.created_at
            && old.source == current.source
            && ["source", "native_key", "session", "part", "message"]
                .iter()
                .all(|field| !prior[*field].is_null() && prior[*field] == next[*field])
            && old.metadata["chat_recent_native_key"] == prior["native_key"]
            && old.metadata["chat_recent_native_version"]
                .as_str()
                .is_some_and(|version| {
                    version.len() == 64
                        && version.bytes().all(|byte| byte.is_ascii_hexdigit())
                        && prior["version"].as_str() == Some(version)
                })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Source {
    path: PathBuf,
    root: PathBuf,
    identity: ProviderLogNativeIdentity,
    minimum_bytes: u64,
    watch_version: Option<(std::time::SystemTime, String)>,
}

impl Source {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        plain_path(path)?;
        let path = path.canonicalize()?;
        if path.to_str().is_none_or(|path| path.len() > 4096) {
            return Err(io::Error::other(
                "recent database path exceeds identity budget or is not UTF-8",
            ));
        }
        let root = path
            .parent()
            .ok_or_else(|| io::Error::other("database has no owner root"))?
            .to_owned();
        let file = File::open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::other("recent database is not a file"));
        }
        Ok(Self {
            path,
            root,
            identity: native_file_identity(&file)?,
            minimum_bytes: metadata.len(),
            watch_version: None,
        })
    }

    /// Bind an already normal-parsed Watch observation to this exact file
    /// version. Unlike a growing database, any file change withholds it until
    /// the normal producer parses and binds another observation.
    pub(crate) fn watch(path: &Path) -> io::Result<Self> {
        let mut source = Self::open(path)?;
        let (modified, anchor) = source.watch_stamp()?;
        source.watch_version = Some((modified, anchor));
        Ok(source)
    }

    pub(crate) fn watch_reader(path: &Path) -> io::Result<(Self, File)> {
        // The normal parser reads this very descriptor. Unlike SQLite's
        // separate name-based open, this needs no provider rename exclusion.
        let file = File::open(path)?;
        let source = Self::watch(path)?;
        if native_file_identity(&file)? != source.identity {
            return Err(io::Error::other("recent Watch reader changed"));
        }
        Ok((source, file))
    }

    fn watch_stamp(&self) -> io::Result<(std::time::SystemTime, String)> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = File::open(&self.path)?;
        let before = file.metadata()?;
        if before.len() != self.minimum_bytes || native_file_identity(&file)? != self.identity {
            return Err(io::Error::other("recent Watch source changed"));
        }
        let mut bytes = Vec::with_capacity(8192);
        Read::by_ref(&mut file).take(4096).read_to_end(&mut bytes)?;
        file.seek(SeekFrom::Start(before.len().saturating_sub(4096)))?;
        Read::by_ref(&mut file).take(4096).read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        if after.len() != before.len() || after.modified()? != before.modified()? {
            return Err(io::Error::other(
                "recent Watch source changed during observation",
            ));
        }
        Ok((after.modified()?, super::chat_recent_seed::hash(&bytes)))
    }

    pub(crate) fn is_database(&self) -> bool {
        self.watch_version.is_none()
    }

    pub(crate) fn matches_path(&self, path: &Path) -> bool {
        path.canonicalize().is_ok_and(|path| path == self.path)
    }

    pub(crate) fn valid(&self) -> io::Result<bool> {
        plain_path(&self.path)?;
        if self.path.parent() != Some(self.root.as_path()) || self.path.canonicalize()? != self.path
        {
            return Ok(false);
        }
        let file = File::open(&self.path)?;
        Ok(file.metadata()?.len() >= self.minimum_bytes
            && native_file_identity(&file)? == self.identity
            && self
                .watch_version
                .as_ref()
                .is_none_or(|version| self.watch_stamp().is_ok_and(|current| &current == version)))
    }

    pub(super) fn key(&self, provider: &str, session: &str, native: &str) -> String {
        super::chat_recent_seed::hash(
            &serde_json::to_vec(&(
                provider,
                session,
                &self.path,
                &self.root,
                &self.identity,
                native,
            ))
            .expect("serializable native source identity"),
        )
    }
}

fn plain_path(path: &Path) -> io::Result<()> {
    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other("linked recent database path"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(io::Error::other("reparse recent database path"));
            }
        }
    }
    Ok(())
}

#[derive(Default, Debug)]
pub(super) struct Metrics {
    pub(super) vm_steps: usize,
    pub(super) progress_quanta: usize,
    pub(super) fullscan_steps: usize,
    pub(super) payload_bytes: usize,
    pub(super) decoded_rows: usize,
}

pub(super) struct OpenCodeRow {
    pub(super) part: super::chat::OpencodeDbPart,
    pub(super) key: String,
    pub(super) version: String,
}

pub(super) struct OpenCodeBatch {
    pub(super) source: Source,
    pub(super) rows: Vec<OpenCodeRow>,
    pub(super) metrics: Metrics,
}

pub(super) fn opencode_part_binding(
    source: &Source,
    session: &str,
    part: &super::chat::OpencodeDbPart,
) -> io::Result<(String, String)> {
    let key = source.key(
        "opencode",
        session,
        &serde_json::to_string(&(&part.message_id, &part.part_id)).map_err(io::Error::other)?,
    );
    let version = super::chat_recent_seed::hash(
        &serde_json::to_vec(&(
            &part.part_id,
            &part.message_id,
            &part.part_data,
            &part.message_data,
            part.part_time_created,
            part.message_time_created,
        ))
        .map_err(io::Error::other)?,
    );
    Ok((key, version))
}

fn bounded_connection(path: &Path) -> io::Result<(Connection, Arc<AtomicUsize>)> {
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(io::Error::other)?;
    connection
        .busy_timeout(std::time::Duration::ZERO)
        .map_err(io::Error::other)?;
    connection
        .set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 32 * 1024)
        .map_err(io::Error::other)?;
    let quanta = Arc::new(AtomicUsize::new(0));
    let observed = quanta.clone();
    connection
        .progress_handler(
            VM_QUANTUM,
            Some(move || observed.fetch_add(1, Ordering::Relaxed) >= VM_QUANTA - 1),
        )
        .map_err(io::Error::other)?;
    connection
        .execute_batch("BEGIN DEFERRED")
        .map_err(io::Error::other)?;
    Ok((connection, quanta))
}

fn database_guard(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // SQLite opens by name. Keep this exact native file non-replaceable
        // across its connection/transaction, including an A-B-A rename race.
        // Read/write sharing retains ordinary provider SQLite writers.
        File::options().read(true).share_mode(3).open(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        // An open POSIX descriptor alone does not prevent rename. Withhold
        // this optional fast path until the connection's native file can be
        // bound there; the normal producer remains unchanged.
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "recent SQLite native connection binding unavailable",
        ))
    }
}

fn payload_bytes(
    connection: &Connection,
    table: &str,
    column: &str,
    rowid: i64,
    metrics: &mut Metrics,
) -> io::Result<Vec<u8>> {
    let blob = connection
        .blob_open("main", table, column, rowid, true)
        .map_err(io::Error::other)?;
    let length = blob.len();
    if length > VALUE_BYTES || length > PAYLOAD_BYTES.saturating_sub(metrics.payload_bytes) {
        return Err(io::Error::other("recent database payload budget exhausted"));
    }
    let mut bytes = vec![0; length];
    blob.read_at_exact(&mut bytes, 0)
        .map_err(io::Error::other)?;
    metrics.payload_bytes += length;
    Ok(bytes)
}

fn payload(
    connection: &Connection,
    table: &str,
    column: &str,
    rowid: i64,
    metrics: &mut Metrics,
) -> io::Result<String> {
    String::from_utf8(payload_bytes(connection, table, column, rowid, metrics)?)
        .map_err(io::Error::other)
}

/// Both native timestamp branches reproduce the normal COALESCE ordering
/// without inventing a global sequence. The same interrupt bounds both plans.
pub(super) fn opencode(path: &Path, session: &str) -> io::Result<OpenCodeBatch> {
    if session.is_empty() || session.len() > 256 {
        return Err(io::Error::other("invalid native session"));
    }
    let _native_guard = database_guard(path)?;
    let source = Source::open(path)?;
    let (connection, quanta) = bounded_connection(path)?;
    let mut candidates = Vec::new();
    let mut metrics = Metrics::default();
    for sql in OPENCODE_QUERIES {
        let mut statement = connection.prepare(sql).map_err(io::Error::other)?;
        {
            let mut rows = statement.query([session]).map_err(io::Error::other)?;
            while let Some(row) = rows.next().map_err(io::Error::other)? {
                let part_id: String = row.get(1).map_err(io::Error::other)?;
                let message_id: String = row.get(4).map_err(io::Error::other)?;
                if part_id.is_empty()
                    || part_id.len() > 256
                    || message_id.is_empty()
                    || message_id.len() > 256
                {
                    return Err(io::Error::other("invalid bounded native part identity"));
                }
                candidates.push((
                    row.get::<_, i64>(0).map_err(io::Error::other)?,
                    part_id,
                    row.get::<_, Option<i64>>(2).map_err(io::Error::other)?,
                    row.get::<_, i64>(3).map_err(io::Error::other)?,
                    message_id,
                    row.get::<_, Option<i64>>(5).map_err(io::Error::other)?,
                ));
            }
        }
        metrics.vm_steps += statement.get_status(StatementStatus::VmStep).max(0) as usize;
        metrics.fullscan_steps +=
            statement.get_status(StatementStatus::FullscanStep).max(0) as usize;
    }
    candidates.sort_by(|a, b| b.2.or(b.5).cmp(&a.2.or(a.5)).then_with(|| b.1.cmp(&a.1)));
    candidates.truncate(ROWS);
    candidates.reverse();
    let mut rows = Vec::new();
    for (part_rowid, part_id, part_time_created, message_rowid, message_id, message_time_created) in
        candidates
    {
        let part_data = payload(&connection, "part", "data", part_rowid, &mut metrics)?;
        let message_data = payload(&connection, "message", "data", message_rowid, &mut metrics)?;
        let part = super::chat::OpencodeDbPart {
            part_id,
            part_data,
            part_time_created,
            message_id,
            message_data,
            message_time_created,
        };
        let (key, version) = opencode_part_binding(&source, session, &part)?;
        rows.push(OpenCodeRow { key, version, part });
        metrics.decoded_rows += 1;
    }
    // Statement counters describe SQL VM work. Blob bytes are logical payload
    // reads; neither is a claim about OS disk IO or a preemptible syscall.
    metrics.progress_quanta = quanta.load(Ordering::Relaxed);
    if !source.valid()? {
        return Err(io::Error::other(
            "recent database source replaced or truncated",
        ));
    }
    Ok(OpenCodeBatch {
        source,
        rows,
        metrics,
    })
}

pub(super) struct AntigravityRow {
    pub(super) message: crate::providers::antigravity::AntigravityConversationMessage,
    pub(super) key: String,
    pub(super) version: String,
}

pub(super) struct AntigravityBatch {
    pub(super) source: Source,
    pub(super) rows: Vec<AntigravityRow>,
    pub(super) metrics: Metrics,
}

pub(super) fn antigravity(path: &Path, session: &str) -> io::Result<AntigravityBatch> {
    use crate::providers::antigravity::AntigravityProvider;
    if session.is_empty()
        || session.len() > 256
        || path.file_stem().and_then(|stem| stem.to_str()) != Some(session)
    {
        return Err(io::Error::other(
            "recent Antigravity conversation does not match its database",
        ));
    }
    let _native_guard = database_guard(path)?;
    let source = Source::open(path)?;
    let (connection, quanta) = bounded_connection(path)?;
    let mut statement = connection.prepare("SELECT rowid,idx,step_type FROM steps WHERE step_type IN (14,15) ORDER BY idx DESC LIMIT 80").map_err(io::Error::other)?;
    let mut selected = Vec::new();
    {
        let mut rows = statement.query([]).map_err(io::Error::other)?;
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let index: i64 = row.get(1).map_err(io::Error::other)?;
            if index < 0 {
                return Err(io::Error::other("negative Antigravity native step"));
            }
            selected.push((
                row.get::<_, i64>(0).map_err(io::Error::other)?,
                index,
                row.get::<_, i64>(2).map_err(io::Error::other)?,
            ));
        }
    }
    let mut metrics = Metrics {
        vm_steps: statement.get_status(StatementStatus::VmStep).max(0) as usize,
        fullscan_steps: statement.get_status(StatementStatus::FullscanStep).max(0) as usize,
        ..Default::default()
    };
    let mut rows = Vec::new();
    for (rowid, index, kind) in selected.into_iter().rev() {
        let bytes = payload_bytes(&connection, "steps", "step_payload", rowid, &mut metrics)?;
        if let Some(message) = AntigravityProvider::message_from_database_step(index, kind, &bytes)
        {
            rows.push(AntigravityRow {
                message,
                key: source.key("antigravity", session, &index.to_string()),
                version: AntigravityProvider::database_step_version(index, kind, &bytes),
            });
        }
        metrics.decoded_rows += 1;
    }
    metrics.progress_quanta = quanta.load(Ordering::Relaxed);
    if !source.valid()? {
        return Err(io::Error::other(
            "recent Antigravity source replaced or truncated",
        ));
    }
    Ok(AntigravityBatch {
        source,
        rows,
        metrics,
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn antigravity_fixture() -> Vec<(i64, i64, Vec<u8>)> {
        let rows: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../providers/antigravity/fixtures/chat-tools-1.1.27.json"
        ))
        .unwrap();
        rows.iter()
            .map(|row| {
                let bytes = row["step_payload_hex"]
                    .as_str()
                    .unwrap()
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect();
                (
                    row["step_index"].as_i64().unwrap(),
                    row["step_type"].as_i64().unwrap(),
                    bytes,
                )
            })
            .collect()
    }

    fn database(indexed: bool, count: usize) -> (tempfile::TempDir, PathBuf, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("opencode.db");
        let mut connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT); CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,data TEXT);").unwrap();
        if indexed {
            connection
                .execute_batch("CREATE INDEX part_recent ON part(session_id,time_created,id)")
                .unwrap();
        }
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO message VALUES ('message','owned',1,'{\"role\":\"assistant\"}')",
                [],
            )
            .unwrap();
        for index in 0..count {
            transaction
                .execute(
                    "INSERT INTO part VALUES (?1,'message','owned',?2,?3)",
                    rusqlite::params![
                        format!("part-{index:06}"),
                        index as i64,
                        format!("{{\"type\":\"text\",\"text\":\"actual-{index}\"}}")
                    ],
                )
                .unwrap();
        }
        transaction.commit().unwrap();
        (temp, path, connection)
    }

    #[test]
    fn recent_database_antigravity_uses_observed_decoder_order_and_versioned_native_steps() {
        use crate::providers::antigravity::AntigravityProvider;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("observed.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE steps(idx INTEGER PRIMARY KEY,step_type INTEGER,step_payload BLOB)",
            )
            .unwrap();
        for (index, kind, payload) in antigravity_fixture() {
            connection
                .execute(
                    "INSERT INTO steps VALUES (?1,?2,?3)",
                    rusqlite::params![index, kind, payload],
                )
                .unwrap();
        }
        let expected =
            AntigravityProvider::conversation_messages_from_database_since(&path, None).unwrap();
        let before = antigravity(&path, "observed").unwrap();
        assert_eq!(
            before
                .rows
                .iter()
                .map(|row| &row.message)
                .collect::<Vec<_>>(),
            expected.iter().collect::<Vec<_>>()
        );
        assert!(before
            .rows
            .iter()
            .all(|row| row.key.len() == 64 && row.version.len() == 64));
        assert!(antigravity(&path, "foreign").is_err());
        let row = &before.rows[0];
        let mut bytes: Vec<u8> = connection
            .query_row(
                "SELECT step_payload FROM steps WHERE idx=?1",
                [row.message.step_index],
                |row| row.get(0),
            )
            .unwrap();
        let text = row.message.text.as_bytes();
        let start = bytes
            .windows(text.len())
            .position(|slice| slice == text)
            .unwrap();
        let at = text.iter().position(u8::is_ascii_alphabetic).unwrap();
        bytes[start + at] = if bytes[start + at] == b'X' {
            b'Y'
        } else {
            b'X'
        };
        connection
            .execute(
                "UPDATE steps SET step_payload=?1 WHERE idx=?2",
                rusqlite::params![bytes, row.message.step_index],
            )
            .unwrap();
        let after = antigravity(&path, "observed").unwrap();
        assert_eq!(before.rows[0].key, after.rows[0].key);
        assert_ne!(before.rows[0].version, after.rows[0].version);
        assert_ne!(before.rows[0].message.text, after.rows[0].message.text);
    }

    #[test]
    fn recent_database_antigravity_large_history_keeps_native_steps_without_text_deduplication() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.db");
        let mut connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE steps(idx INTEGER PRIMARY KEY,step_type INTEGER,step_payload BLOB)",
            )
            .unwrap();
        let (_, kind, payload) = antigravity_fixture()
            .into_iter()
            .filter(|(_, kind, _)| matches!(kind, 14 | 15))
            .min_by_key(|(_, _, bytes)| bytes.len())
            .unwrap();
        let transaction = connection.transaction().unwrap();
        for index in 0..20_000 {
            transaction
                .execute(
                    "INSERT INTO steps VALUES (?1,?2,?3)",
                    rusqlite::params![index, kind, &payload],
                )
                .unwrap();
        }
        transaction.commit().unwrap();
        let batch = antigravity(&path, "large").unwrap();
        assert_eq!(batch.rows.len(), 80);
        assert_eq!(batch.rows[0].message.step_index, 19_920);
        assert_eq!(batch.rows[79].message.step_index, 19_999);
        assert_eq!(
            batch
                .rows
                .iter()
                .map(|row| &row.key)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            80
        );
        assert!(batch.metrics.vm_steps < VM_QUANTA * VM_QUANTUM as usize);
        assert!(batch.metrics.payload_bytes <= PAYLOAD_BYTES);
        eprintln!("ANTIGRAVITY_RECENT_BOUNDED {:?}", batch.metrics);
    }

    #[test]
    fn recent_database_indexed_large_history_is_bounded_and_uses_actual_native_order() {
        let (_temp, path, connection) = database(true, 20_000);
        for sql in OPENCODE_QUERIES {
            let mut statement = connection
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            let plan = statement
                .query_map(["owned"], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(
                plan.iter().any(|line| line.contains("part_recent")),
                "{plan:?}"
            );
            eprintln!("RECENT_DATABASE_PLAN {plan:?}");
        }
        let batch = opencode(&path, "owned").unwrap();
        assert_eq!(batch.rows.len(), 80);
        assert_eq!(batch.rows.first().unwrap().part.part_id, "part-019920");
        assert_eq!(batch.rows.last().unwrap().part.part_id, "part-019999");
        assert!(batch.source.valid().unwrap());
        assert!(batch.metrics.vm_steps <= VM_QUANTA * VM_QUANTUM as usize);
        assert!(batch.metrics.progress_quanta <= VM_QUANTA);
        assert!(batch.metrics.fullscan_steps < 200);
        assert!(batch.metrics.payload_bytes <= PAYLOAD_BYTES);
        assert_eq!(batch.metrics.decoded_rows, 80);
        eprintln!(
            "RECENT_DATABASE_BOUNDED {:?}; logical payload/VM only, not physical disk IO",
            batch.metrics
        );
    }

    #[test]
    fn recent_database_aggregate_payload_budget_is_independent_of_row_limit() {
        let (_temp, path, connection) = database(true, 80);
        let value = format!("{{\"type\":\"text\",\"text\":\"{}\"}}", "x".repeat(2048));
        connection
            .execute("UPDATE part SET data=?1", [value])
            .unwrap();
        let error = match opencode(&path, "owned") {
            Ok(_) => panic!("80 individually small payloads must not bypass total bytes"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "recent database payload budget exhausted"
        );
    }

    #[test]
    fn recent_database_unindexed_history_interrupts_instead_of_claiming_limit_bounds_work() {
        let (_temp, path, _connection) = database(false, 20_000);
        let error = match opencode(&path, "owned") {
            Ok(_) => panic!("unindexed sort must exceed this VM budget"),
            Err(error) => error,
        };
        assert_eq!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<rusqlite::Error>())
                .and_then(rusqlite::Error::sqlite_error_code),
            Some(rusqlite::ErrorCode::OperationInterrupted)
        );
    }

    #[test]
    fn recent_database_native_guard_prevents_path_aba_during_connection() {
        let (_temp, path, connection) = database(true, 1);
        drop(connection);
        let guard = database_guard(&path).unwrap();
        let source = Source::open(&path).unwrap();
        assert!(std::fs::rename(&path, path.with_extension("moved.db")).is_err());
        assert!(source.valid().unwrap());
        drop(guard);
        std::fs::rename(&path, path.with_extension("moved.db")).unwrap();
    }

    #[test]
    fn recent_database_part_edits_keep_native_key_but_change_version() {
        let (_temp, path, connection) = database(true, 1);
        let before = opencode(&path, "owned").unwrap();
        connection.execute("UPDATE part SET data='{\"type\":\"text\",\"text\":\"edited actual part\"}' WHERE id='part-000000'", []).unwrap();
        let after = opencode(&path, "owned").unwrap();
        assert_eq!(before.rows[0].key, after.rows[0].key);
        assert_ne!(before.rows[0].version, after.rows[0].version);
    }

    #[test]
    fn recent_database_foreign_message_session_is_excluded_and_native_time_absence_is_preserved() {
        let (_temp, path, connection) = database(true, 1);
        connection
            .execute(
                "INSERT INTO message VALUES ('foreign','other',10,'{\"role\":\"assistant\"}')",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO part VALUES ('foreign-part','foreign','owned',20,'{\"type\":\"text\",\"text\":\"foreign\"}')", []).unwrap();
        connection
            .execute(
                "UPDATE part SET time_created=NULL WHERE id='part-000000'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE message SET time_created=NULL WHERE id='message'",
                [],
            )
            .unwrap();
        let batch = opencode(&path, "owned").unwrap();
        assert_eq!(batch.rows.len(), 1);
        assert_eq!(batch.rows[0].part.part_id, "part-000000");
        assert!(
            batch.rows[0].part.part_time_created.is_none()
                && batch.rows[0].part.message_time_created.is_none()
        );
    }

    #[test]
    fn recent_database_rejects_oversized_payload_and_same_path_replacement_or_truncation() {
        let (_temp, path, connection) = database(true, 1);
        let source = Source::open(&path).unwrap();
        connection
            .execute("UPDATE part SET data=?1", ["x".repeat(100_000)])
            .unwrap();
        assert!(opencode(&path, "owned").is_err());
        drop(connection);
        let original = path.with_extension("before.db");
        std::fs::rename(&path, &original).unwrap();
        std::fs::copy(&original, &path).unwrap();
        assert!(
            !source.valid().unwrap(),
            "equal content at the same path does not preserve native identity"
        );
        let replacement = Source::open(&path).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(0)
            .unwrap();
        assert!(!replacement.valid().unwrap());
    }
}
