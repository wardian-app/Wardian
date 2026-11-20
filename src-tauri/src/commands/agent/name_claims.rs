use super::*;

#[derive(Debug)]
pub(super) struct SpawnNameReservation {
    pub(super) session_name: String,
    pub(super) owner_id: String,
}

pub(super) struct LegacyNameClaimGuard {
    reservation: SpawnNameReservation,
    release_on_drop: bool,
}

impl LegacyNameClaimGuard {
    pub(super) fn new(reservation: SpawnNameReservation) -> Self {
        Self {
            reservation,
            release_on_drop: true,
        }
    }

    pub(super) fn retain_for_uncertain_launch(&mut self) {
        self.release_on_drop = false;
    }

    pub(super) fn commit(&mut self) {
        self.release_on_drop = false;
    }
}

impl Drop for LegacyNameClaimGuard {
    fn drop(&mut self) {
        if self.release_on_drop {
            let _ = wardian_core::agent_spawn::release_legacy_name(
                &self.reservation.session_name,
                &self.reservation.owner_id,
            );
        }
    }
}

pub(super) async fn reserve_spawn_session_name(
    state: &AppState,
    requested_session_name: &str,
    agent_class: &str,
    agent_id: &str,
) -> Result<SpawnNameReservation, String> {
    let explicit_name = !requested_session_name.trim().is_empty();
    loop {
        let mut existing_names = persisted_agent_session_names()?;
        existing_names.extend(wardian_core::agent_spawn::claimed_names()?);
        {
            let agents = state.agents.lock().await;
            existing_names.extend(
                agents
                    .values()
                    .map(|agent| agent.config.lock().unwrap().session_name.clone())
                    .collect::<HashSet<_>>(),
            );
        }
        let owner_id = uuid::Uuid::new_v4().to_string();
        let session_name = {
            let mut reservations = state.agent_name_reservations.lock().await;
            existing_names.extend(reservations.iter().cloned());
            let session_name = resolve_requested_spawn_session_name(
                requested_session_name,
                agent_class,
                &existing_names,
            )?;
            reservations.insert(session_name.clone());
            session_name
        };
        match wardian_core::agent_spawn::reserve_legacy_name(
            &owner_id,
            agent_id,
            wardian_core::agent_spawn::LegacyNamePurpose::Spawn,
            None,
            &session_name,
        ) {
            Ok(()) => {
                return Ok(SpawnNameReservation {
                    session_name,
                    owner_id,
                })
            }
            Err(error) if !explicit_name && error.contains("already exists") => {
                state
                    .agent_name_reservations
                    .lock()
                    .await
                    .remove(&session_name);
            }
            Err(error) => {
                state
                    .agent_name_reservations
                    .lock()
                    .await
                    .remove(&session_name);
                return Err(error);
            }
        }
    }
}

pub(super) async fn existing_spawn_session_names(
    state: &AppState,
) -> Result<HashSet<String>, String> {
    let mut existing_names = persisted_agent_session_names()?;
    existing_names.extend(wardian_core::agent_spawn::claimed_names()?);
    let agents = state.agents.lock().await;
    existing_names.extend(
        agents
            .values()
            .map(|agent| agent.config.lock().unwrap().session_name.clone())
            .collect::<HashSet<_>>(),
    );
    let reservations = state.agent_name_reservations.lock().await;
    existing_names.extend(reservations.iter().cloned());
    Ok(existing_names)
}

pub(super) async fn release_spawn_name_reservation(
    state: &AppState,
    reservation: &SpawnNameReservation,
) {
    let mut reservations = state.agent_name_reservations.lock().await;
    reservations.remove(&reservation.session_name);
    drop(reservations);
    let _ = wardian_core::agent_spawn::release_legacy_name(
        &reservation.session_name,
        &reservation.owner_id,
    );
}

/// Reserve a requested rename name using the same lock order as spawn-name
/// reservation. The reservation closes the gap between checking the live
/// roster and committing the rename while another spawn is in flight.
pub(super) async fn reserve_rename_session_name(
    state: &AppState,
    session_id: &str,
    new_name: &str,
) -> Result<Option<SpawnNameReservation>, String> {
    let persisted_names = persisted_agent_session_names()?;
    let agents = state.agents.lock().await;
    let current_name = agents
        .get(session_id)
        .ok_or_else(|| format!("Agent {} not found", session_id))?
        .config
        .lock()
        .map_err(|_| "Agent configuration lock poisoned".to_string())?
        .session_name
        .clone();

    if current_name == new_name {
        return Ok(None);
    }

    if persisted_names.iter().any(|name| name == new_name) {
        return Err(format!(
            "An agent with the name '{}' already exists.",
            new_name
        ));
    }

    if agents.values().any(|agent| {
        agent
            .config
            .lock()
            .map(|config| config.session_name == new_name && config.session_id != session_id)
            .unwrap_or(false)
    }) {
        return Err(format!(
            "An agent with the name '{}' already exists.",
            new_name
        ));
    }

    let mut reservations = state.agent_name_reservations.lock().await;
    if reservations.contains(new_name) {
        return Err(format!(
            "An agent with the name '{}' already exists.",
            new_name
        ));
    }
    let owner_id = uuid::Uuid::new_v4().to_string();
    reservations.insert(new_name.to_string());
    drop(reservations);
    drop(agents);
    let claimed = wardian_core::agent_spawn::reserve_legacy_name(
        &owner_id,
        session_id,
        wardian_core::agent_spawn::LegacyNamePurpose::Rename,
        Some(&current_name),
        new_name,
    );
    match claimed {
        Ok(()) => Ok(Some(SpawnNameReservation {
            session_name: new_name.to_string(),
            owner_id,
        })),
        Err(error) => {
            let _ = wardian_core::agent_spawn::release_legacy_name(new_name, &owner_id);
            state.agent_name_reservations.lock().await.remove(new_name);
            Err(error)
        }
    }
}

pub(super) fn abort_failed_rename(
    reservation: &SpawnNameReservation,
    agent_id: &str,
) -> Result<(), String> {
    let result = crate::spawn_operations::persisted_roster().and_then(|roster| {
        wardian_core::agent_spawn::abort_legacy_rename(&reservation.owner_id, agent_id, &roster)
    });
    match result {
        Ok(true) => Ok(()),
        result => Err(format!(
            "[agent-name-ownership-pending] Agent {agent_id}, attempt {}, has an unresolved rename. {}",
            reservation.owner_id,
            result.err().unwrap_or_else(|| "Durable stores disagree after rename rollback".into()),
        )),
    }
}
