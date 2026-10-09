//! A provider turn ending is evidence, not a task disposition. Only the complete
//! validated appendix can authorize automatic per-request outcomes.

use super::*;
use crate::agent_messaging::{parse_task_outcomes, TaskOutcomePacket};

/// Committed results ready for outbox publication and informational delivery.
/// Neither collection authorizes another provider turn.
pub struct RecordedTaskFinal {
    pub outcomes: Vec<TaskTurnBinding>,
    pub information: Vec<Admitted>,
}

fn bounded_preview(answer: &str) -> &str {
    let mut end = answer.len().min(MAX_MESSAGE_BYTES);
    while !answer.is_char_boundary(end) {
        end -= 1;
    }
    &answer[..end]
}

fn has_reply(conn: &Connection, binding: &TaskTurnBinding) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM structured_replies WHERE request_id=?1)",
        [&binding.request_id],
        |row| row.get(0),
    )?)
}

fn observed_bindings(conn: &Connection, anchor: &TaskTurnBinding) -> Result<Vec<TaskTurnBinding>> {
    let mut statement = conn.prepare(
        "SELECT b.request_id,b.claim_token,b.recipient,b.generation,b.provider,b.provider_session_id,b.provider_turn_id,b.admission_mode
         FROM agent_message_task_turns b JOIN agent_message_delivery d ON d.interaction_id=b.request_id
         AND d.claim_token=b.claim_token AND d.generation=b.generation AND d.recipient=b.recipient
         AND d.operation='followup_task' AND d.owner='provider_accepted'
         WHERE b.recipient=?1 AND b.generation=?2 AND b.provider=?3
         AND b.provider_session_id=?4 AND b.provider_turn_id=?5 AND b.settlement!='uncertain'
         ORDER BY b.request_id",
    )?;
    let rows = statement.query_map(
        params![
            anchor.recipient,
            anchor.generation,
            anchor.provider,
            anchor.provider_session_id,
            anchor.provider_turn_id
        ],
        row_binding,
    )?;
    let bindings = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for binding in &bindings {
        settlement(conn, binding)?;
    }
    Ok(bindings)
}

/// Validate every correlation before recording any positive outcome. Explicit
/// replies retain precedence even when a packet repeats a different disposition.
fn validate_packet(
    conn: &Connection,
    bindings: &[TaskTurnBinding],
    packet: &TaskOutcomePacket,
) -> Result<()> {
    for outcome in &packet.outcomes {
        let binding = bindings
            .iter()
            .find(|binding| binding.request_id == outcome.request_id)
            .ok_or_else(|| {
                Error::new(
                    "unattributed_outcome",
                    "Every result must name an accepted task in this exact observed turn.",
                )
            })?;
        if has_reply(conn, binding)? {
            continue;
        }
        let task = load(conn, &binding.request_id)?;
        if task.status != InteractionStatus::AwaitingReply {
            return Err(Error::new(
                "unattributed_outcome",
                "Task no longer awaits a result.",
            ));
        }
        if settlement(conn, binding)? != "bound" {
            let status = super::super::super::enum_value(&outcome.status)?;
            let same: bool = conn.query_row(
                "SELECT terminal_status=?2 AND terminal_body=?3 FROM agent_message_task_turns WHERE request_id=?1",
                params![binding.request_id, status, outcome.result], |row| row.get(0),
            )?;
            if !same {
                return Err(Error::new(
                    "conflicting_outcome",
                    "A recorded task outcome cannot be replaced.",
                ));
            }
        }
    }
    Ok(())
}

fn verify_observation(
    conn: &Connection,
    binding: &TaskTurnBinding,
    provider_status: &str,
    hash: &str,
    bytes: i64,
) -> Result<bool> {
    let previous: Option<(String, String, i64)> = conn.query_row(
        "SELECT provider_status,answer_sha256,answer_bytes FROM agent_message_task_turn_observations WHERE request_id=?1",
        [&binding.request_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional()?;
    if let Some(previous) = previous {
        if previous != (provider_status.into(), hash.into(), bytes) {
            return Err(Error::new(
                "conflicting_observation",
                "This exact task binding already has different terminal evidence.",
            ));
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Atomically capture an actual terminal turn, validate its entire attribution
/// packet and record the per-request outbox. Omitted or invalid outcomes leave
/// canonical tasks awaiting reply, with durable evidence and one requester INFO.
/// Callers must fence the live runtime generation before entering this transaction.
pub fn record_task_turn_final(
    conn: &Connection,
    anchor: &TaskTurnBinding,
    provider_status: &str,
    answer: &str,
) -> Result<RecordedTaskFinal> {
    if !matches!(provider_status, "completed" | "interrupted" | "failed") {
        return Err(Error::new(
            "invalid_observation",
            "A known provider terminal status is required.",
        ));
    }
    let (packet, mut diagnostic) = if provider_status == "completed" {
        match parse_task_outcomes(answer) {
            Ok(Some(parsed)) => (
                Some(parsed.packet),
                "No attributed result was supplied for this task.",
            ),
            Ok(None) => (None, "The final answer has no task-outcome appendix."),
            Err(_) => (
                None,
                "The task-outcome appendix is malformed or exceeds its limits.",
            ),
        }
    } else {
        (
            None,
            "The provider ended this turn without an attributed task result.",
        )
    };
    let hash = format!("{:x}", Sha256::digest(answer.as_bytes()));
    let bytes = i64::try_from(answer.len()).map_err(|_| {
        Error::new(
            "invalid_observation",
            "Answer byte count is not representable.",
        )
    })?;
    let tx = conn.unchecked_transaction()?;
    if settlement(&tx, anchor)? == "uncertain" {
        return Err(Error::new(
            "uncertain_binding",
            "Lost continuity cannot authorize terminal evidence.",
        ));
    }
    let bindings = observed_bindings(&tx, anchor)?;
    // Verify evidence for the whole observed set before any positive write.
    let previous = bindings
        .iter()
        .map(|binding| verify_observation(&tx, binding, provider_status, &hash, bytes))
        .collect::<Result<Vec<_>>>()?;
    let packet = match packet {
        Some(packet) => match validate_packet(&tx, &bindings, &packet) {
            Ok(()) => Some(packet),
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "unattributed_outcome" | "conflicting_outcome"
                ) =>
            {
                diagnostic = "The whole appendix was rejected because at least one result lacks exact eligible task attribution.";
                None
            }
            Err(error) => return Err(error),
        },
        None => None,
    };
    let mut recorded = RecordedTaskFinal {
        outcomes: Vec::new(),
        information: Vec::new(),
    };
    for (binding, previous) in bindings.iter().zip(previous) {
        let outcome = packet.as_ref().and_then(|packet| {
            packet
                .outcomes
                .iter()
                .find(|outcome| outcome.request_id == binding.request_id)
        });
        let already_replied = has_reply(&tx, binding)?;
        if let Some(outcome) = outcome.filter(|_| !already_replied) {
            record_outcome_in_transaction(&tx, binding, outcome.status.clone(), &outcome.result)?;
            recorded.outcomes.push(binding.clone());
        }
        if previous {
            continue;
        }
        let task = load(&tx, &binding.request_id)?;
        let information = if outcome.is_none()
            && !already_replied
            && task.status == InteractionStatus::AwaitingReply
            && settlement(&tx, binding)? == "bound"
        {
            if let Some(requester) = task.sender_session_id.as_deref() {
                let body = format!("Wardian: task {} has a finished Codex turn ({provider_status}) without a published task-specific result. The task remains awaiting reply. {diagnostic}", binding.request_id);
                let key = format!("task_turn_observation:{}", binding.request_id);
                let mut admitted = admit_in_transaction(
                    &tx,
                    Admission {
                        sender: &binding.recipient,
                        recipient: requester,
                        message: &body,
                        idempotency_key: Some(&key),
                        task: false,
                        generation: 0,
                    },
                    None,
                )?;
                admitted.record.parent_interaction_id = Some(binding.request_id.clone());
                super::super::super::upsert_interaction_record_with_conn(&tx, &admitted.record)?;
                Some(admitted)
            } else {
                None
            }
        } else {
            None
        };
        let observation_diagnostic = if already_replied {
            "An explicit reply already completed this task."
        } else if outcome.is_some() {
            "A task-specific outcome was captured."
        } else {
            diagnostic
        };
        tx.execute(
            "INSERT INTO agent_message_task_turn_observations(request_id,provider_status,answer_sha256,answer_bytes,answer_preview,diagnostic,information_id,observed_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![binding.request_id, provider_status, hash, bytes, bounded_preview(answer), observation_diagnostic,
                information.as_ref().map(|value| value.record.id.as_str()), now()],
        )?;
        if let Some(information) = information {
            recorded.information.push(information);
        }
    }
    tx.commit()?;
    Ok(recorded)
}

#[cfg(test)]
mod tests;
