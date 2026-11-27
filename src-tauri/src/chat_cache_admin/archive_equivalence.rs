//! Read-only evidence about owned rows and body bytes. This is deliberately a
//! narrower claim than reader parity or permission to retire any store.

mod archive;
pub(super) mod copy;
mod graph;

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use archive::Archive;
use copy::{Fault, Reader};
use graph::{object, Published, Row, OBJECT_BYTES};

const ATTEMPTS: usize = 32_768;
const BYTES: usize = 128 * 1024 * 1024;
const HEADS: usize = 512;
const INVENTORY_OBJECT_BYTES: usize = 256 * 1024;

pub(super) fn component(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 255
        || name.ends_with('.')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return false;
    }
    #[cfg(windows)]
    {
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
            || ((stem.starts_with("com") || stem.starts_with("lpt"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return false;
        }
    }
    true
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn digest_ref(reference: &str) -> bool {
    reference.len() == 64
        && reference
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Default, Serialize)]
struct Counts {
    matched: usize,
    missing: usize,
    unverifiable: usize,
}

#[derive(Default, Serialize)]
struct ScanFaults {
    missing_input: usize,
    unsafe_input: usize,
    invalid_input: usize,
    changed_input: usize,
    budget_exhausted: usize,
}

#[derive(Serialize)]
pub(super) struct Report {
    schema: u8,
    dry_run: bool,
    deletion_blocked: bool,
    unverified_surfaces: &'static [&'static str],
    scan_complete: bool,
    measured_equivalent: bool,
    incomplete_scans: usize,
    scan_faults: ScanFaults,
    heads_checked: usize,
    detached_heads: usize,
    partial_heads: usize,
    authenticated_legacy_packs: usize,
    protected_pack_rows: usize,
    rows: Counts,
    bodies: Counts,
    current_archive_unpaired_events: usize,
    incomplete_old_bodies: usize,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            schema: 1,
            dry_run: true,
            deletion_blocked: true,
            unverified_surfaces: &[
                "legacy_token_acceptance_and_revision_deltas",
                "turns_aliases_and_native_admission",
                "native_preview_and_logging_disabled",
                "source_and_working_graphs",
                "turns_jsonl_consumer_cutover",
                "deletion_authority",
            ],
            scan_complete: false,
            measured_equivalent: false,
            incomplete_scans: 0,
            scan_faults: ScanFaults::default(),
            heads_checked: 0,
            detached_heads: 0,
            partial_heads: 0,
            authenticated_legacy_packs: 0,
            protected_pack_rows: 0,
            rows: Counts::default(),
            bodies: Counts::default(),
            current_archive_unpaired_events: 0,
            incomplete_old_bodies: 0,
        }
    }
}

impl Report {
    fn fault(&mut self, fault: Fault) {
        self.incomplete_scans += 1;
        match fault {
            Fault::Missing => self.scan_faults.missing_input += 1,
            Fault::Unsafe => self.scan_faults.unsafe_input += 1,
            Fault::Invalid => self.scan_faults.invalid_input += 1,
            Fault::Changed => self.scan_faults.changed_input += 1,
            Fault::Limit => self.scan_faults.budget_exhausted += 1,
        }
    }

    pub(super) fn measured_equivalent(&self) -> bool {
        self.measured_equivalent
    }
}

#[derive(Deserialize)]
struct ProgressRoot {
    version: u8,
    agent_id: String,
    conversation_id: String,
    head_ref: Option<String>,
}

struct Audit<'a> {
    reader: Reader,
    agent: &'a str,
    agent_dir: PathBuf,
    objects: PathBuf,
    seen: BTreeSet<String>,
    current: Option<String>,
    scopes: BTreeSet<(String, String, Option<String>, Option<String>)>,
    report: Report,
}

pub(super) fn verify(home: PathBuf, agent: &str) -> Report {
    verify_with_limits(home, agent, ATTEMPTS, BYTES)
}

fn verify_with_limits(home: PathBuf, agent: &str, attempts: usize, bytes: usize) -> Report {
    let agent_dir = Path::new("agents").join(agent);
    let mut audit = Audit {
        reader: Reader::new(home, attempts, bytes),
        agent,
        objects: agent_dir.join("chat-read-objects"),
        agent_dir,
        seen: BTreeSet::new(),
        current: None,
        scopes: BTreeSet::new(),
        report: Report::default(),
    };
    if !component(agent) {
        audit.report.fault(Fault::Unsafe);
    } else if let Err(fault) = audit.scan() {
        audit.report.fault(fault);
    }
    if !audit.reader.stable() {
        audit.report.fault(Fault::Changed);
    }
    let report = &mut audit.report;
    report.scan_complete = report.incomplete_scans == 0 && report.heads_checked > 0;
    report.measured_equivalent = report.scan_complete
        && report.rows.matched > 0
        && report.partial_heads == 0
        && report.rows.missing == 0
        && report.rows.unverifiable == 0
        && report.bodies.missing == 0
        && report.bodies.unverifiable == 0
        && report.current_archive_unpaired_events == 0;
    audit.report
}

fn scope(head: &Published) -> (String, String, Option<String>, Option<String>) {
    let head = &head.head;
    (
        head.conversation_id.clone(),
        head.generation.clone(),
        head.source_key.clone(),
        head.source_epoch.clone(),
    )
}

impl Audit<'_> {
    fn scan(&mut self) -> Result<(), Fault> {
        let reference: Option<String> = serde_json::from_slice(
            &self
                .reader
                .file(&self.agent_dir.join("chat-read-head.json"), 4096)?,
        )
        .map_err(|_| Fault::Invalid)?;
        let reference = reference.ok_or(Fault::Missing)?;
        self.current = Some(reference.clone());
        self.scan_chain(reference, false)?;
        let progress = self.reader.file(
            &self.agent_dir.join("chat-read-progress.json"),
            2 * 1024 * 1024,
        );
        match progress {
            Ok(bytes) => {
                let progress: Option<ProgressRoot> =
                    serde_json::from_slice(&bytes).map_err(|_| Fault::Invalid)?;
                if let Some(progress) = progress {
                    if !matches!(progress.version, 1 | 2)
                        || progress.agent_id != self.agent
                        || !component(&progress.conversation_id)
                    {
                        return Err(Fault::Invalid);
                    }
                    if let Some(reference) = progress.head_ref {
                        let head = self.load_head(&reference)?;
                        if head.head.conversation_id != progress.conversation_id {
                            return Err(Fault::Invalid);
                        }
                        self.scan_chain(reference, false)?;
                    }
                }
            }
            Err(Fault::Missing) => {}
            Err(fault) => return Err(fault),
        }

        let names = self.reader.objects(&self.objects)?;
        for reference in names {
            if self.seen.contains(&reference) {
                continue;
            }
            let bytes = match object(
                &mut self.reader,
                &self.objects,
                &reference,
                INVENTORY_OBJECT_BYTES,
            ) {
                Ok(bytes) => bytes,
                Err(Fault::Limit) => return Err(Fault::Limit),
                Err(fault) => {
                    self.report.fault(fault);
                    continue;
                }
            };
            if bytes.len() > OBJECT_BYTES {
                continue;
            }
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if ![
                "agent_id",
                "conversation_id",
                "generation",
                "progress",
                "row_count",
            ]
            .iter()
            .all(|key| value.get(key).is_some())
            {
                continue;
            }
            match Published::decode(&bytes, self.agent) {
                Ok(head) if self.scopes.contains(&scope(&head)) => {
                    self.scan_chain(reference, true)?
                }
                Ok(_) => {}
                Err(fault) => self.report.fault(fault),
            }
        }
        Ok(())
    }

    fn load_head(&mut self, reference: &str) -> Result<Published, Fault> {
        let bytes = object(&mut self.reader, &self.objects, reference, OBJECT_BYTES)?;
        Published::decode(&bytes, self.agent)
    }

    fn scan_chain(&mut self, reference: String, detached: bool) -> Result<(), Fault> {
        let mut pending = VecDeque::from([(reference, detached)]);
        while let Some((reference, detached)) = pending.pop_front() {
            if self.seen.contains(&reference) {
                continue;
            }
            if self.seen.len() >= HEADS {
                return Err(Fault::Limit);
            }
            self.seen.insert(reference.clone());
            let head = match self.load_head(&reference) {
                Ok(head) => head,
                Err(Fault::Limit) => return Err(Fault::Limit),
                Err(fault) => {
                    self.report.fault(fault);
                    continue;
                }
            };
            if let Some(parent) = &head.links.parent {
                pending.push_back((parent.clone(), false));
            }
            self.scopes.insert(scope(&head));
            self.report.heads_checked += 1;
            self.report.detached_heads += usize::from(detached);
            self.report.partial_heads += usize::from(head.head.progress != "ready");
            match self.scan_head(&head, self.current.as_deref() == Some(reference.as_str())) {
                Ok(()) => {}
                Err(Fault::Limit) => return Err(Fault::Limit),
                Err(fault) => self.report.fault(fault),
            }
        }
        Ok(())
    }

    fn scan_head(&mut self, head: &Published, current: bool) -> Result<(), Fault> {
        let positions = graph::tree(&mut self.reader, &self.objects, head.head.root.as_deref())?;
        if positions.len() > head.links.row_count
            || (head.head.progress == "ready"
                && head.links.bootstrap.is_none()
                && positions.len() != head.links.row_count)
            || positions.keys().any(|key| {
                graph::numeric_key(key)
                    .map_or(true, |position| position >= head.links.row_count as u64)
            })
        {
            return Err(Fault::Invalid);
        }
        let identities = graph::tree(
            &mut self.reader,
            &self.objects,
            head.head.identities.as_deref(),
        )?;
        if identities.keys().any(|key| !digest_ref(key)) {
            return Err(Fault::Invalid);
        }
        let mut protected = match &head.links.recent_page {
            Some(reference) => match graph::legacy_pack(
                &mut self.reader,
                &self.objects,
                reference,
                head,
                &positions,
            ) {
                Ok(pack) => {
                    self.report.authenticated_legacy_packs += 1;
                    pack
                }
                Err(Fault::Limit) => return Err(Fault::Limit),
                Err(fault) => {
                    self.report.fault(fault);
                    Default::default()
                }
            },
            None => Default::default(),
        };
        let archive = match Archive::read(&mut self.reader, &self.agent_dir, &head.head) {
            Ok(archive) => Some(archive),
            Err(Fault::Limit) => return Err(Fault::Limit),
            Err(fault) => {
                self.report.fault(fault);
                None
            }
        };
        let mut prior = None;
        let mut projected_ids = HashSet::new();
        for (key, reference) in positions {
            let row = match object(&mut self.reader, &self.objects, &reference, OBJECT_BYTES)
                .and_then(|bytes| Row::decode(&bytes, &head.head, &key))
            {
                Ok(row) => row,
                Err(Fault::Missing) if protected.contains_key(&key) => {
                    self.report.protected_pack_rows += 1;
                    protected.remove(&key).expect("authenticated pack member")
                }
                Err(Fault::Limit) => return Err(Fault::Limit),
                Err(fault) => {
                    self.report.rows.unverifiable += 1;
                    self.report.fault(fault);
                    continue;
                }
            };
            if identities.get(&digest(row.event.id.as_bytes())) != Some(&reference) {
                self.report.rows.unverifiable += 1;
                self.report.fault(Fault::Invalid);
                continue;
            }
            if current {
                projected_ids.insert(row.event.id.clone());
            }
            self.compare(&row, archive.as_ref(), &mut prior)?;
        }
        if let Some(archive) = archive.filter(|_| current) {
            self.report.current_archive_unpaired_events = archive.unpaired(&projected_ids);
        }
        Ok(())
    }

    fn compare(
        &mut self,
        row: &Row,
        archive: Option<&Archive>,
        prior: &mut Option<usize>,
    ) -> Result<(), Fault> {
        let old = if let Some(body) = &row.body {
            if !body.complete {
                self.report.incomplete_old_bodies += 1;
            }
            match body.payload(&mut self.reader, &self.objects) {
                Ok(payload) if body.complete => Ok(Some(payload)),
                Ok(_) => Err(Fault::Invalid),
                Err(Fault::Limit) => return Err(Fault::Limit),
                Err(fault) => {
                    self.report.fault(fault);
                    Err(Fault::Invalid)
                }
            }
        } else if row.event.metadata["chat_body_unavailable"] == true
            || row.event.metadata["chat_body_pending"] == true
        {
            Err(Fault::Invalid)
        } else {
            Ok(None)
        };
        let Some(archive) = archive else {
            self.report.rows.unverifiable += 1;
            self.report.bodies.unverifiable += usize::from(!matches!(old, Ok(None)));
            return Ok(());
        };
        let (event, record, position) = match archive.event(&row.event.id) {
            Ok(value) => value,
            Err(Fault::Missing) => {
                self.report.rows.missing += 1;
                match old {
                    Ok(Some(_)) => self.report.bodies.missing += 1,
                    Err(_) => self.report.bodies.unverifiable += 1,
                    Ok(None) => {}
                }
                return Ok(());
            }
            Err(_) => {
                self.report.rows.unverifiable += 1;
                self.report.bodies.unverifiable += usize::from(!matches!(old, Ok(None)));
                return Ok(());
            }
        };
        if prior.is_some_and(|prior| position <= prior) || !archive::header_matches(row, event) {
            self.report.rows.unverifiable += 1;
            self.report.bodies.unverifiable += usize::from(!matches!(old, Ok(None)));
            return Ok(());
        }
        *prior = Some(position);
        self.report.rows.matched += 1;
        let expected = archive.payload(&mut self.reader, event, record);
        match (old, expected) {
            (_, Err(Fault::Limit)) => return Err(Fault::Limit),
            (Ok(None), Ok(None)) => {}
            (Ok(Some(old)), Ok(Some(expected))) if old == expected => {
                self.report.bodies.matched += 1
            }
            (Ok(_), Err(Fault::Missing)) => self.report.bodies.missing += 1,
            (_, Err(fault @ (Fault::Unsafe | Fault::Changed))) => {
                self.report.fault(fault);
                self.report.bodies.unverifiable += 1;
            }
            _ => self.report.bodies.unverifiable += 1,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
