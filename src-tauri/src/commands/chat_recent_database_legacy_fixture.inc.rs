// Legacy projection code retained for compatibility fixtures.
// This file is included only by the test build of its owner module.

const ROWS: usize = 80;

const PAYLOAD_BYTES: usize = 128 * 1024;

const VALUE_BYTES: usize = 16 * 1024;

const VM_QUANTUM: i32 = 100;

const VM_QUANTA: usize = 1000;

const OPENCODE_QUERIES: [&str; 2] = [
    "SELECT p.rowid,p.id,p.time_created,m.rowid,m.id,m.time_created FROM part p JOIN message m ON m.id=p.message_id WHERE p.session_id=?1 AND m.session_id=?1 AND p.time_created IS NOT NULL ORDER BY p.time_created DESC,p.id DESC LIMIT 80",
    "SELECT p.rowid,p.id,p.time_created,m.rowid,m.id,m.time_created FROM part p JOIN message m ON m.id=p.message_id WHERE p.session_id=?1 AND m.session_id=?1 AND p.time_created IS NULL ORDER BY m.time_created DESC,p.id DESC LIMIT 80",
];

impl Source {
    pub(crate) fn is_database(&self) -> bool {
        self.watch_version.is_none()
    }
}

impl Source {
    pub(crate) fn matches_path(&self, path: &Path) -> bool {
        path.canonicalize().is_ok_and(|path| path == self.path)
    }
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
