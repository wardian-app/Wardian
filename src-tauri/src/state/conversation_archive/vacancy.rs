//! Explicit operator repair of one proven never-written archive slot.
use super::{agent_lock_for, conversation_dir, lock_agent_archive, ConversationArchiveState};
use std::{fs, io};
use wardian_core::archive_vacancy::{
    inspect_vacancy, read_vacancies, validate_vacancies, SequenceVacancy, MAX_VACANCIES,
    MAX_VACANCY_LINE_BYTES,
};
use wardian_core::control::{ArchiveVacancyRequest, ArchiveVacancyResponse, CONTROL_SCHEMA};

fn invalid(reason: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, reason.into())
}

impl ConversationArchiveState {
    pub(super) fn validate_vacancy_capture(
        &self,
        context: &super::ConversationArchiveContext,
        handle: &super::ActiveConversationHandle,
        directory: &std::path::Path,
    ) -> io::Result<publication::Publication> {
        publication::Publication::load(directory, &handle.conversation_id)
            .map_err(|error| self.record_recovery_failure(context, handle, error))
    }

    /// Revalidate under the same archive lock used by capture. Approval creates
    /// only a vacancy; it grants no provider range or native capture authority.
    pub(crate) fn authorize_vacancy(
        &self,
        agent_id: &str,
        request: ArchiveVacancyRequest,
    ) -> io::Result<ArchiveVacancyResponse> {
        request.validate().map_err(invalid)?;
        let lock = agent_lock_for(&self.agent_locks, agent_id)?;
        let _guard = lock_agent_archive(&lock)?;
        let directory = conversation_dir(agent_id, &request.conversation_id)?;
        let manifest: wardian_core::conversations::ConversationManifest =
            serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)
                .map_err(io::Error::other)?;
        if manifest.agent_id != agent_id || manifest.conversation_id != request.conversation_id {
            return Err(invalid(
                "conversation_identity rule rejected the requested archive",
            ));
        }
        validate_vacancies(&directory, &request.conversation_id)?;
        let existing = read_vacancies(&directory, &request.conversation_id)?;
        let prior = existing.iter().find(|record| {
            record.authorization_id == request.authorization_id
                || record.sequence == request.sequence
        });
        let created = if let Some(prior) = prior {
            if prior.authorization_id != request.authorization_id
                || prior.sequence != request.sequence
                || prior.approval_reference != request.approval_reference
                || prior.approval_text != request.approval_text
            {
                return Err(invalid(
                    "vacancy authorization or sequence already used for another scope",
                ));
            }
            false
        } else {
            if existing.len() >= MAX_VACANCIES {
                return Err(invalid("vacancy_limit rule rejected the requested archive"));
            }
            let report = inspect_vacancy(&directory, &request.conversation_id, request.sequence)?;
            if !report.eligible {
                let rules = report
                    .rules
                    .iter()
                    .filter(|rule| !rule.passed)
                    .map(|rule| rule.rule.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(invalid(format!("vacancy rejected by rules: {rules}")));
            }
            let record = SequenceVacancy {
                schema: 1,
                kind: "sequence_vacancy".into(),
                conversation_id: request.conversation_id.clone(),
                sequence: request.sequence,
                evidence: report
                    .evidence
                    .ok_or_else(|| invalid("eligible vacancy lacks evidence"))?,
                authorization_id: request.authorization_id.clone(),
                approval_reference: request.approval_reference.clone(),
                approval_text: request.approval_text.clone(),
                created_at: super::current_rfc3339_millis(),
            };
            let mut line = serde_json::to_vec(&record).map_err(io::Error::other)?;
            if line.len() + 1 > MAX_VACANCY_LINE_BYTES {
                return Err(invalid("vacancy_line_bytes rule rejected the approval"));
            }
            line.push(b'\n');
            let path = directory.join("vacancies.jsonl");
            let old_bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(error),
            };
            let mut next_bytes = old_bytes.clone();
            next_bytes.extend_from_slice(&line);
            crate::utils::codex_messaging::atomic_replace(&path, &next_bytes)
                .map_err(io::Error::other)?;
            let saved = fs::read(&path)?;
            if !saved.starts_with(&old_bytes) || saved != next_bytes {
                return Err(io::Error::other(
                    "vacancy append prefix verification failed; preserve the ledger for operator review; no automatic repair was attempted",
                ));
            }
            validate_vacancies(&directory, &request.conversation_id)?;
            true
        };
        if !created {
            // A previous attempt may have persisted the file before its parent
            // sync failed. An acknowledged retry must seal that same evidence.
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(directory.join("vacancies.jsonl"))?
                .sync_all()?;
            #[cfg(unix)]
            fs::File::open(&directory)?.sync_all()?;
        }
        // A retired adoption stays retired. Clearing this diagnostic permits
        // the ordinary authorization path to re-grant the exact native range.
        if let Some(stop) = wardian_core::conversations::read_archive_sequence_gap(agent_id)? {
            if stop.conversation_id == request.conversation_id
                && stop.sequence == request.sequence
                && stop.vacancy_rule.is_none()
            {
                fs::remove_file(wardian_core::conversations::archive_capture_status_path(
                    agent_id,
                )?)?;
            }
        }
        self.chat_projection.invalidate(agent_id)?;
        let adoption_regrant_required = !self.has_pending_adoption(agent_id)?;
        Ok(ArchiveVacancyResponse {
            schema: CONTROL_SCHEMA,
            agent_id: agent_id.into(),
            conversation_id: request.conversation_id,
            sequence: request.sequence,
            created,
            adoption_regrant_required,
            reason: if adoption_regrant_required {
                "Vacancy recorded. Re-grant archive adoption through adopt-archive with recorded approval before capture resumes."
            } else {
                "Vacancy recorded. The existing adoption can resume through normal capture."
            }.into(),
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod raw_line_tests;

pub(super) mod publication;
