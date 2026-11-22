//! Read-only operator evidence for explicitly authorized narrative sequence vacancies.
//!
//! This module never writes an archive and never returns ledger rows as narrative.
//! Callers own the archive lock, persistence, and integration before repair writes.

mod boundary;
mod scan;

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::Path;

/// Maximum number of authorizations in one conversation ledger.
pub const MAX_VACANCIES: usize = 64;
/// Maximum encoded line size, including its LF terminator.
pub const MAX_VACANCY_LINE_BYTES: usize = 4 * 1024;

/// Direct-neighbour proof. The scan digest is an authorization-time audit value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VacancyEvidence {
    pub committed_head_sequence: u64,
    pub prev_record_hash: String,
    pub next_record_hash: String,
    pub scan_digest: String,
}

/// One append-only operator authorization, separate from archive narrative.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SequenceVacancy {
    pub schema: u8,
    pub kind: String,
    pub conversation_id: String,
    pub sequence: u64,
    pub evidence: VacancyEvidence,
    pub authorization_id: String,
    pub approval_reference: String,
    pub approval_text: String,
    pub created_at: String,
}

/// A stable, content-free validation rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleOutcome {
    pub rule: String,
    pub passed: bool,
}

/// Sanitized inspection result. No paths, bodies, or provider metadata are exposed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VacancyReport {
    pub sequence: u64,
    pub conversation_id: String,
    pub committed_head_sequence: u64,
    pub eligible: bool,
    pub rules: Vec<RuleOutcome>,
    pub evidence: Option<VacancyEvidence>,
}

/// Directly downcastable inner error for terminal capture diagnostics.
///
/// Sequence zero means malformed evidence did not supply a trustworthy sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VacancyConflict {
    pub sequence: u64,
    pub rule: String,
}

impl fmt::Display for VacancyConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "sequence vacancy conflict at {} (rule: {}). Preserve the archive and run archive-vacancy --dry-run for operator review; no automatic repair was attempted",
            self.sequence, self.rule
        )
    }
}

impl std::error::Error for VacancyConflict {}

pub(super) fn conflict(sequence: u64, rule: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        VacancyConflict {
            sequence,
            rule: rule.to_owned(),
        },
    )
}

/// Inspect a candidate without writing or opening provider paths.
///
/// Expected rejections are named-rule reports. Malformed archives are also
/// reported without propagating filesystem paths or JSON parser contents.
pub fn inspect_vacancy(
    directory: &Path,
    conversation_id: &str,
    sequence: u64,
) -> io::Result<VacancyReport> {
    let result: io::Result<VacancyReport> = (|| {
        scan::check_identity(conversation_id, sequence)?;
        let ledger = read_vacancies(directory, conversation_id)?;
        let prior = if ledger.is_empty() {
            None
        } else {
            let prior = existing_snapshot(directory, conversation_id, sequence, &ledger)?;
            for vacancy in &ledger {
                prior.validate(vacancy)?;
            }
            Some(prior)
        };
        let snapshot = scan::Snapshot::load(
            directory,
            conversation_id,
            sequence,
            scan::Purpose::Authorization,
        )?;
        for vacancy in &ledger {
            snapshot.validate(vacancy)?;
        }
        let report = snapshot.inspect(sequence)?;
        snapshot.stable(sequence)?;
        if let Some(prior) = prior {
            prior.stable(sequence)?;
        }
        Ok(report)
    })();
    match result {
        Ok(report) => Ok(report),
        Err(error) => {
            let rule = error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<VacancyConflict>())
                .map_or("archive_readable", |inner| inner.rule.as_str());
            Ok(VacancyReport {
                sequence,
                // Invalid identifiers must not echo arbitrary supplied content.
                conversation_id: if scan::safe_id(conversation_id) {
                    conversation_id.to_owned()
                } else {
                    String::new()
                },
                committed_head_sequence: 0,
                eligible: false,
                rules: vec![RuleOutcome {
                    rule: rule.to_owned(),
                    passed: false,
                }],
                evidence: None,
            })
        }
    }
}

/// Read the bounded, strict LF-terminated authorization ledger.
///
/// Missing ledger returns immediately without scanning narrative. Parsing checks
/// every row and rejects duplicate sequences/authorization IDs, including identical
/// repeated rows. The parent writer implements idempotency before appending.
pub fn read_vacancies(directory: &Path, conversation_id: &str) -> io::Result<Vec<SequenceVacancy>> {
    scan::check_identity(conversation_id, 0)?;
    scan::read_ledger(directory, conversation_id)
}

/// Match a previously validated owned body against a replay using contained,
/// streaming reads and the same SHA-256 binding as the neighbour closure.
/// This creates no vacancy authority. The caller retains the archive lock.
pub fn body_matches_text(directory: &Path, reference: &str, text: &str) -> io::Result<bool> {
    scan::body_matches(directory, reference, text.as_bytes())
}

/// Validate all current authorizations and return only their reserved sequences.
///
/// Missing or empty ledger takes the fast path without a narrative scan. Current
/// direct-neighbour hashes and absence are rechecked; scan_digest is never an
/// equality requirement after future archive appends.
pub fn validate_vacancies(directory: &Path, conversation_id: &str) -> io::Result<BTreeSet<u64>> {
    let ledger = read_vacancies(directory, conversation_id)?;
    if ledger.is_empty() {
        return Ok(BTreeSet::new());
    }
    let first = ledger[0].sequence;
    let snapshot = existing_snapshot(directory, conversation_id, first, &ledger)?;
    for vacancy in &ledger {
        snapshot.validate(vacancy)?;
    }
    snapshot.stable(first)?;
    Ok(ledger.into_iter().map(|row| row.sequence).collect())
}

fn existing_snapshot(
    directory: &Path,
    conversation_id: &str,
    sequence: u64,
    ledger: &[SequenceVacancy],
) -> io::Result<scan::Snapshot> {
    let committed_head = ledger
        .iter()
        .map(|row| row.evidence.committed_head_sequence)
        .max()
        .unwrap_or(0);
    let vacancies = ledger.iter().map(|row| row.sequence).collect();
    scan::Snapshot::load(
        directory,
        conversation_id,
        sequence,
        scan::Purpose::ExistingLedger {
            committed_head,
            vacancies,
        },
    )
}

#[cfg(test)]
mod tests;
