use crate::remote::crypto::random_url_token;
use crate::remote::models::{
    AuthChallengeRecord, DeviceCredentialKind, PairingOfferRecord, PairingQrPayload,
    PendingPairingDecision, PendingPairingRequestRecord, RateLimitBucket,
    RemoteClientPairingQrPayload, RemoteClientScope, RemoteClientSessionRecord,
    RemoteCredentialAudience, RemoteDeviceStore, RemoteReadGrant, RemoteRuntimeState,
    RemoteSendGrant, RemoteSessionRecord, WebSocketTicketRecord,
};
use std::{collections::HashSet, sync::Arc};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

const PAIRING_OFFER_TTL_MS: i64 = 120_000;
const AUTH_CHALLENGE_TTL_MS: i64 = 60_000;
const ACCESS_SESSION_TTL_MS: i64 = 15 * 60 * 1000;
const CLIENT_ACCESS_TOKEN_TTL_MS: i64 = 15 * 60 * 1000;
const ABSOLUTE_SESSION_TTL_MS: i64 = 12 * 60 * 60 * 1000;
const WEBSOCKET_TICKET_TTL_MS: i64 = 60_000;
const PENDING_PAIRING_TTL_MS: i64 = 120_000;
const RATE_LIMIT_RETAIN_MS: i64 = 10 * 60 * 1000;
const MAX_RATE_LIMIT_BUCKETS: usize = 1024;
const MAX_CLIENT_AUTH_CHALLENGES: usize = 1024;
const MAX_CLIENT_SESSIONS: usize = 1024;
const MAX_ACTIVE_STATUS_STREAMS_PER_SESSION: usize = 3;

pub fn create_pairing_offer(
    runtime: &mut RemoteRuntimeState,
    canonical_origin: &str,
    server_fingerprint: &str,
    now_ms: i64,
) -> Result<PairingQrPayload, String> {
    let offer = create_pairing_offer_for(
        runtime,
        canonical_origin,
        server_fingerprint,
        DeviceCredentialKind::Pwa,
        None,
        Vec::new(),
        now_ms,
    )?;
    Ok(PairingQrPayload {
        gateway_origin: offer.canonical_origin,
        pairing_offer_id: offer.offer_id,
        expires_at: millis_to_rfc3339(offer.expires_at_ms),
        nonce: offer.nonce,
        server_identity_fingerprint: offer.gateway_identity_fingerprint,
    })
}

pub fn create_client_pairing_offer(
    runtime: &mut RemoteRuntimeState,
    canonical_gateway_origin: &str,
    client_origin: &str,
    server_fingerprint: &str,
    now_ms: i64,
) -> Result<RemoteClientPairingQrPayload, String> {
    let offer = create_pairing_offer_for(
        runtime,
        canonical_gateway_origin,
        server_fingerprint,
        DeviceCredentialKind::PairedClient,
        Some(client_origin.to_string()),
        vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
        now_ms,
    )?;
    Ok(RemoteClientPairingQrPayload {
        gateway_origin: offer.canonical_origin,
        credential_kind: offer.credential_kind,
        client_origin: offer
            .client_origin
            .ok_or_else(|| "client_pairing_origin_missing".to_string())?,
        read_grants: offer.read_grants,
        pairing_offer_id: offer.offer_id,
        expires_at: millis_to_rfc3339(offer.expires_at_ms),
        nonce: offer.nonce,
        server_identity_fingerprint: offer.gateway_identity_fingerprint,
    })
}

fn create_pairing_offer_for(
    runtime: &mut RemoteRuntimeState,
    canonical_origin: &str,
    server_fingerprint: &str,
    credential_kind: DeviceCredentialKind,
    client_origin: Option<String>,
    read_grants: Vec<RemoteReadGrant>,
    now_ms: i64,
) -> Result<PairingOfferRecord, String> {
    prune_expired_runtime_state(runtime, now_ms);
    let offer_id = random_url_token(18);
    let nonce = random_url_token(18);
    let audience = RemoteCredentialAudience::for_kind(credential_kind);
    let offer = PairingOfferRecord {
        offer_id: offer_id.clone(),
        nonce: nonce.clone(),
        canonical_origin: canonical_origin.to_string(),
        gateway_identity_fingerprint: server_fingerprint.to_string(),
        credential_kind,
        audience,
        client_origin: client_origin.clone(),
        read_grants: read_grants.clone(),
        expires_at_ms: now_ms + PAIRING_OFFER_TTL_MS,
        used: false,
    };
    runtime.pairing_offers.insert(offer_id, offer.clone());
    Ok(offer)
}

pub fn consume_pairing_offer(
    runtime: &mut RemoteRuntimeState,
    offer_id: &str,
    now_ms: i64,
) -> Result<PairingOfferRecord, String> {
    let offer = runtime
        .pairing_offers
        .get_mut(offer_id)
        .ok_or_else(|| "pairing_offer_not_found".to_string())?;
    if offer.used {
        return Err("pairing_offer_used".to_string());
    }
    if now_ms > offer.expires_at_ms {
        return Err("pairing_offer_expired".to_string());
    }
    if offer.credential_kind != DeviceCredentialKind::Pwa
        || offer.audience != RemoteCredentialAudience::Pwa
        || offer.client_origin.is_some()
    {
        return Err("pairing_offer_invalid".to_string());
    }
    offer.used = true;
    Ok(offer.clone())
}

pub fn consume_client_pairing_offer(
    runtime: &mut RemoteRuntimeState,
    offer_id: &str,
    nonce: &str,
    canonical_gateway_origin: &str,
    gateway_identity_fingerprint: &str,
    client_origin: &str,
    now_ms: i64,
) -> Result<PairingOfferRecord, String> {
    let offer = runtime
        .pairing_offers
        .get_mut(offer_id)
        .ok_or_else(|| "pairing_offer_invalid".to_string())?;
    if offer.used
        || now_ms > offer.expires_at_ms
        || offer.credential_kind != DeviceCredentialKind::PairedClient
        || offer.audience != RemoteCredentialAudience::PairedClientV1
        || offer.canonical_origin != canonical_gateway_origin
        || offer.gateway_identity_fingerprint != gateway_identity_fingerprint
        || offer.client_origin.as_deref() != Some(client_origin)
        || offer.nonce.as_bytes().ct_eq(nonce.as_bytes()).unwrap_u8() != 1
    {
        return Err("pairing_offer_invalid".to_string());
    }
    offer.used = true;
    Ok(offer.clone())
}

#[derive(Clone, Copy)]
pub(crate) struct PairingDeviceIdentity<'a> {
    pub(crate) device_id: &'a str,
    pub(crate) device_label: &'a str,
    pub(crate) public_key_spki_der_base64: &'a str,
    pub(crate) public_key_fingerprint: &'a str,
}

pub fn create_pending_pairing_request(
    runtime: &mut RemoteRuntimeState,
    offer: &PairingOfferRecord,
    device_id: &str,
    device_label: &str,
    public_key_spki_der_base64: &str,
    public_key_fingerprint: &str,
    now_ms: i64,
) -> PendingPairingRequestRecord {
    create_pending_pairing_request_with_grants(
        runtime,
        offer,
        PairingDeviceIdentity {
            device_id,
            device_label,
            public_key_spki_der_base64,
            public_key_fingerprint,
        },
        offer.read_grants.clone(),
        Vec::new(),
        now_ms,
    )
}

pub(crate) fn create_pending_client_pairing_request(
    runtime: &mut RemoteRuntimeState,
    offer: &PairingOfferRecord,
    identity: PairingDeviceIdentity<'_>,
    requested_scopes: Option<&[RemoteClientScope]>,
    now_ms: i64,
) -> Result<PendingPairingRequestRecord, String> {
    let (read_grants, send_grants) = match requested_scopes {
        None => (offer.read_grants.clone(), Vec::new()),
        Some(scopes) => {
            if scopes.is_empty()
                || scopes
                    .iter()
                    .enumerate()
                    .any(|(index, scope)| scopes[..index].contains(scope))
            {
                return Err("invalid_request".to_string());
            }
            let read_grants = [RemoteClientScope::AgentsRead, RemoteClientScope::ChatRead]
                .into_iter()
                .filter_map(|scope| match scope {
                    RemoteClientScope::AgentsRead if scopes.contains(&scope) => {
                        Some(RemoteReadGrant::AgentsRead)
                    }
                    RemoteClientScope::ChatRead if scopes.contains(&scope) => {
                        Some(RemoteReadGrant::ChatRead)
                    }
                    _ => None,
                })
                .collect();
            let send_grants = scopes
                .contains(&RemoteClientScope::ChatSend)
                .then_some(RemoteSendGrant::ChatSend)
                .into_iter()
                .collect();
            (read_grants, send_grants)
        }
    };
    Ok(create_pending_pairing_request_with_grants(
        runtime,
        offer,
        identity,
        read_grants,
        send_grants,
        now_ms,
    ))
}

fn create_pending_pairing_request_with_grants(
    runtime: &mut RemoteRuntimeState,
    offer: &PairingOfferRecord,
    identity: PairingDeviceIdentity<'_>,
    read_grants: Vec<RemoteReadGrant>,
    send_grants: Vec<RemoteSendGrant>,
    now_ms: i64,
) -> PendingPairingRequestRecord {
    prune_expired_runtime_state(runtime, now_ms);
    let request = PendingPairingRequestRecord {
        request_id: random_url_token(18),
        pairing_offer_id: offer.offer_id.clone(),
        device_id: identity.device_id.to_string(),
        device_label: identity.device_label.to_string(),
        public_key_spki_der_base64: identity.public_key_spki_der_base64.to_string(),
        public_key_fingerprint: identity.public_key_fingerprint.to_string(),
        canonical_origin: offer.canonical_origin.clone(),
        credential_kind: offer.credential_kind,
        audience: offer.audience,
        client_origin: offer.client_origin.clone(),
        read_grants,
        send_grants,
        submitted_at_ms: now_ms,
        expires_at_ms: offer.expires_at_ms.min(now_ms + PENDING_PAIRING_TTL_MS),
        decision: PendingPairingDecision::Pending,
        paired_at: None,
    };
    runtime
        .pending_pairing_requests
        .insert(request.request_id.clone(), request.clone());
    request
}

pub fn pending_pairing_request_is_active(
    request: &PendingPairingRequestRecord,
    now_ms: i64,
) -> bool {
    request.decision == PendingPairingDecision::Pending && now_ms <= request.expires_at_ms
}

pub(crate) fn begin_pending_pairing_approval(
    runtime: &mut RemoteRuntimeState,
    request_id: &str,
    now_ms: i64,
) -> Result<PendingPairingRequestRecord, String> {
    prune_expired_runtime_state(runtime, now_ms);
    let request = runtime
        .pending_pairing_requests
        .get_mut(request_id)
        .ok_or_else(|| "pending_pairing_not_found".to_string())?;
    if !pending_pairing_request_is_active(request, now_ms) {
        return Err("pending_pairing_not_active".to_string());
    }
    request.decision = PendingPairingDecision::Approving;
    Ok(request.clone())
}

pub(crate) fn mark_pending_pairing_approved(
    runtime: &mut RemoteRuntimeState,
    request_id: &str,
    paired_at: &str,
) -> Result<PendingPairingRequestRecord, String> {
    let request = runtime
        .pending_pairing_requests
        .get_mut(request_id)
        .ok_or_else(|| "pending_pairing_not_found".to_string())?;
    if request.decision != PendingPairingDecision::Approving {
        return Err("pending_pairing_approval_not_started".to_string());
    }
    request.decision = PendingPairingDecision::Approved;
    request.paired_at = Some(paired_at.to_string());
    Ok(request.clone())
}

pub(crate) fn mark_pending_pairing_approval_failed(
    runtime: &mut RemoteRuntimeState,
    request_id: &str,
    now_ms: i64,
) {
    let should_remove = match runtime.pending_pairing_requests.get_mut(request_id) {
        Some(request) if request.decision == PendingPairingDecision::Approving => {
            if now_ms <= request.expires_at_ms {
                request.decision = PendingPairingDecision::Pending;
                false
            } else {
                true
            }
        }
        _ => false,
    };
    if should_remove {
        runtime.pending_pairing_requests.remove(request_id);
    }
}

pub fn mark_pending_pairing_rejected(
    runtime: &mut RemoteRuntimeState,
    request_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    prune_expired_runtime_state(runtime, now_ms);
    let request = runtime
        .pending_pairing_requests
        .get_mut(request_id)
        .ok_or_else(|| "pending_pairing_not_found".to_string())?;
    if !pending_pairing_request_is_active(request, now_ms) {
        return Err("pending_pairing_not_active".to_string());
    }
    request.decision = PendingPairingDecision::Rejected;
    Ok(())
}

pub fn create_session_record(device_id: &str, now_ms: i64) -> RemoteSessionRecord {
    RemoteSessionRecord {
        session_id: random_url_token(32),
        device_id: device_id.to_string(),
        created_at_ms: now_ms,
        last_seen_at_ms: now_ms,
        expires_at_ms: now_ms + ACCESS_SESSION_TTL_MS,
        absolute_expires_at_ms: now_ms + ABSOLUTE_SESSION_TTL_MS,
        csrf_nonce: random_url_token(24),
        revoked: false,
    }
}

pub fn create_session(
    runtime: &mut RemoteRuntimeState,
    device_id: &str,
    now_ms: i64,
) -> RemoteSessionRecord {
    prune_expired_runtime_state(runtime, now_ms);
    runtime
        .sessions
        .retain(|_, session| session.device_id != device_id);
    let session = create_session_record(device_id, now_ms);
    runtime
        .sessions
        .insert(session.session_id.clone(), session.clone());
    session
}

/// Rechecks persistent authority and creates the session under the shared
/// device-transition gate so revocation cannot finish between those steps.
pub(crate) async fn issue_session_for_active_device(
    device_store_lock: Arc<Mutex<()>>,
    runtime: &Mutex<RemoteRuntimeState>,
    device_id: String,
    last_used_at: String,
    now_ms: i64,
) -> Result<RemoteSessionRecord, crate::remote::storage::DeviceStoreUpdateError> {
    let _device_store_guard = device_store_lock.lock_owned().await;
    crate::remote::storage::update_device_last_used(device_id.clone(), last_used_at).await?;
    let mut runtime = runtime.lock().await;
    Ok(create_session(&mut runtime, &device_id, now_ms))
}

/// Issues a fixed-expiry client token only after rechecking its persisted
/// credential binding under the same gate used by device revocation.
pub(crate) struct ClientAccessTokenIssue {
    pub(crate) device_id: String,
    pub(crate) client_origin: String,
    pub(crate) gateway_origin: String,
    pub(crate) gateway_identity_fingerprint: String,
    pub(crate) challenge_expires_at_ms: i64,
    pub(crate) last_used_at: String,
}

pub(crate) async fn issue_client_access_token_for_active_device(
    device_store_lock: Arc<Mutex<()>>,
    runtime: &Mutex<RemoteRuntimeState>,
    issue: ClientAccessTokenIssue,
) -> Result<(String, RemoteClientSessionRecord), String> {
    let _device_store_guard = device_store_lock.lock_owned().await;
    let expected_device_id = issue.device_id.clone();
    let expected_origin = issue.client_origin.clone();
    let last_used_at = issue.last_used_at.clone();
    let (store, (read_grants, send_grants)) =
        crate::remote::storage::update_device_store(move |store| {
            let device = store
                .devices
                .iter_mut()
                .find(|device| device.device_id == expected_device_id)
                .ok_or_else(|| "remote_device_not_found".to_string())?;
            if device.revoked_at.is_some() {
                return Err("remote_device_revoked".to_string());
            }
            if device.credential_kind != DeviceCredentialKind::PairedClient
                || device.audience != RemoteCredentialAudience::PairedClientV1
                || device.client_origin.as_deref() != Some(expected_origin.as_str())
                || !valid_client_grants(&device.read_grants, &device.send_grants)
            {
                return Err("remote_device_credential_mismatch".to_string());
            }
            crate::remote::storage::advance_device_last_used(device, &last_used_at)?;
            Ok((device.read_grants.clone(), device.send_grants.clone()))
        })
        .await
        .map_err(|error| error.message)?;
    drop(store);

    let mut runtime = runtime.lock().await;
    let issued_at_ms = chrono::Utc::now().timestamp_millis();
    if issued_at_ms >= issue.challenge_expires_at_ms {
        return Err("auth_challenge_expired".to_string());
    }
    prune_expired_runtime_state(&mut runtime, issued_at_ms);
    if runtime.client_sessions.len() >= MAX_CLIENT_SESSIONS {
        return Err("client_session_capacity".to_string());
    }
    let namespace = if send_grants.contains(&RemoteSendGrant::ChatSend) {
        Some(
            crate::remote::client_messages::get_or_create_namespace(
                issue.device_id.clone(),
                issue.client_origin.clone(),
                RemoteCredentialAudience::PairedClientV1,
                issued_at_ms,
            )
            .await?,
        )
    } else {
        None
    };
    create_client_access_token_with_grants_at(
        &mut runtime,
        issue,
        read_grants,
        send_grants,
        namespace,
        issued_at_ms,
    )
}

fn valid_client_grants(read_grants: &[RemoteReadGrant], send_grants: &[RemoteSendGrant]) -> bool {
    !read_grants
        .iter()
        .enumerate()
        .any(|(index, grant)| read_grants[..index].contains(grant))
        && !send_grants
            .iter()
            .enumerate()
            .any(|(index, grant)| send_grants[..index].contains(grant))
}

fn create_client_access_token_with_grants_at(
    runtime: &mut RemoteRuntimeState,
    issue: ClientAccessTokenIssue,
    read_grants: Vec<RemoteReadGrant>,
    send_grants: Vec<RemoteSendGrant>,
    namespace: Option<(String, i64)>,
    issued_at_ms: i64,
) -> Result<(String, RemoteClientSessionRecord), String> {
    if issued_at_ms >= issue.challenge_expires_at_ms {
        return Err("auth_challenge_expired".to_string());
    }
    prune_expired_runtime_state(runtime, issued_at_ms);
    if runtime.client_sessions.len() >= MAX_CLIENT_SESSIONS {
        return Err("client_session_capacity".to_string());
    }

    let access_token = random_url_token(32);
    let token_verifier = crate::remote::crypto::sha256_token_verifier(&access_token);
    let session = RemoteClientSessionRecord {
        session_id: random_url_token(18),
        device_id: issue.device_id,
        client_origin: issue.client_origin,
        gateway_origin: issue.gateway_origin,
        audience: RemoteCredentialAudience::PairedClientV1,
        gateway_identity_fingerprint: issue.gateway_identity_fingerprint,
        read_grants,
        send_grants,
        idempotency_namespace: namespace.as_ref().map(|(id, _)| id.clone()),
        idempotency_expires_at_ms: namespace.map(|(_, expires_at_ms)| expires_at_ms),
        expires_at_ms: issued_at_ms + CLIENT_ACCESS_TOKEN_TTL_MS,
        revoked: false,
    };
    runtime
        .client_sessions
        .insert(token_verifier, session.clone());
    Ok((access_token, session))
}

pub(crate) fn invalidate_client_authority_for_config_change(
    runtime: &mut RemoteRuntimeState,
    previous: Option<&crate::remote::models::RemoteGatewayConfig>,
    current: &crate::remote::models::RemoteGatewayConfig,
) {
    let Some(previous) = previous else {
        clear_client_auth_runtime(runtime);
        return;
    };
    let same_gateway = previous.enabled == current.enabled
        && previous.canonical_origin == current.canonical_origin
        && previous.gateway_identity_public_key == current.gateway_identity_public_key
        && previous.gateway_identity_fingerprint == current.gateway_identity_fingerprint;
    if !same_gateway {
        clear_client_auth_runtime(runtime);
        return;
    }

    for removed_origin in previous
        .client_origin_allowlist
        .iter()
        .filter(|origin| !current.client_origin_allowlist.contains(origin))
    {
        invalidate_client_origin_authority(runtime, removed_origin);
    }
}

pub(crate) fn invalidate_client_origin_authority(
    runtime: &mut RemoteRuntimeState,
    client_origin: &str,
) {
    runtime
        .client_sessions
        .retain(|_, session| session.client_origin != client_origin);
    runtime
        .client_auth_challenges
        .retain(|_, challenge| challenge.client_origin.as_deref() != Some(client_origin));
    runtime.pairing_offers.retain(|_, offer| {
        offer.credential_kind != DeviceCredentialKind::PairedClient
            || offer.client_origin.as_deref() != Some(client_origin)
    });
}

pub(crate) fn invalidate_all_client_runtime_authority(runtime: &mut RemoteRuntimeState) {
    runtime.client_sessions.clear();
    runtime.client_auth_challenges.clear();
    runtime
        .pairing_offers
        .retain(|_, offer| offer.credential_kind != DeviceCredentialKind::PairedClient);
}

fn clear_client_auth_runtime(runtime: &mut RemoteRuntimeState) {
    invalidate_all_client_runtime_authority(runtime);
}

/// Publishes revocation before invalidating sessions while holding the same
/// transition gate used by final session issuance.
pub(crate) async fn revoke_device_and_sessions(
    device_store_lock: Arc<Mutex<()>>,
    runtime: &Mutex<RemoteRuntimeState>,
    device_id: String,
    revoked_at: String,
) -> Result<RemoteDeviceStore, crate::remote::storage::DeviceStoreUpdateError> {
    let _device_store_guard = device_store_lock.lock_owned().await;
    let target_device_id = device_id.clone();
    let update = crate::remote::storage::update_device_store(move |store| {
        let device = store
            .devices
            .iter_mut()
            .find(|device| device.device_id == target_device_id)
            .ok_or_else(|| "remote_device_not_found".to_string())?;
        if device.revoked_at.is_none() {
            device.revoked_at = Some(revoked_at);
        }
        Ok(())
    })
    .await;

    match update {
        Ok((store, ())) => {
            let mut runtime = runtime.lock().await;
            revoke_sessions_for_device(&mut runtime, &device_id);
            Ok(store)
        }
        Err(error) => {
            if error.publish_state != crate::remote::storage::DeviceStorePublishState::NotPublished
            {
                let mut runtime = runtime.lock().await;
                revoke_sessions_for_device(&mut runtime, &device_id);
            }
            Err(error)
        }
    }
}

pub fn session_is_active(session: &RemoteSessionRecord, now_ms: i64) -> bool {
    !session.revoked && now_ms <= session.expires_at_ms && now_ms <= session.absolute_expires_at_ms
}

pub fn refresh_session_activity(
    session: &mut RemoteSessionRecord,
    now_ms: i64,
) -> Result<(), String> {
    if !session_is_active(session, now_ms) {
        return Err("session_expired".to_string());
    }
    session.last_seen_at_ms = now_ms;
    session.expires_at_ms = (now_ms + ACCESS_SESSION_TTL_MS).min(session.absolute_expires_at_ms);
    Ok(())
}

pub fn csrf_nonce_matches(session: &RemoteSessionRecord, submitted_nonce: &str) -> bool {
    !session.revoked
        && !submitted_nonce.is_empty()
        && session
            .csrf_nonce
            .as_bytes()
            .ct_eq(submitted_nonce.as_bytes())
            .into()
}

pub fn revoke_sessions_for_device(runtime: &mut RemoteRuntimeState, device_id: &str) -> usize {
    let mut revoked = 0;
    for session in runtime.sessions.values_mut() {
        if session.device_id == device_id && !session.revoked {
            session.revoked = true;
            revoked += 1;
        }
    }
    for session in runtime.client_sessions.values_mut() {
        if session.device_id == device_id && !session.revoked {
            session.revoked = true;
            revoked += 1;
        }
    }
    runtime
        .client_auth_challenges
        .retain(|_, challenge| challenge.device_id != device_id);
    revoked
}

pub fn create_auth_challenge(
    runtime: &mut RemoteRuntimeState,
    device_id: &str,
    canonical_origin: &str,
    gateway_identity_fingerprint: &str,
    now_ms: i64,
) -> AuthChallengeRecord {
    create_auth_challenge_for(
        runtime,
        device_id,
        canonical_origin,
        gateway_identity_fingerprint,
        RemoteCredentialAudience::Pwa,
        None,
        now_ms,
    )
}

pub fn create_client_auth_challenge(
    runtime: &mut RemoteRuntimeState,
    device_id: &str,
    canonical_gateway_origin: &str,
    gateway_identity_fingerprint: &str,
    client_origin: &str,
    now_ms: i64,
) -> Result<AuthChallengeRecord, String> {
    prune_expired_runtime_state(runtime, now_ms);
    if runtime.client_auth_challenges.len() >= MAX_CLIENT_AUTH_CHALLENGES {
        return Err("client_auth_challenge_capacity".to_string());
    }
    Ok(create_auth_challenge_for(
        runtime,
        device_id,
        canonical_gateway_origin,
        gateway_identity_fingerprint,
        RemoteCredentialAudience::PairedClientV1,
        Some(client_origin.to_string()),
        now_ms,
    ))
}

fn create_auth_challenge_for(
    runtime: &mut RemoteRuntimeState,
    device_id: &str,
    canonical_origin: &str,
    gateway_identity_fingerprint: &str,
    audience: RemoteCredentialAudience,
    client_origin: Option<String>,
    now_ms: i64,
) -> AuthChallengeRecord {
    prune_expired_runtime_state(runtime, now_ms);
    let challenge_id = random_url_token(18);
    let challenge = AuthChallengeRecord {
        challenge_id: challenge_id.clone(),
        device_id: device_id.to_string(),
        nonce: random_url_token(18),
        canonical_origin: canonical_origin.to_string(),
        gateway_identity_fingerprint: gateway_identity_fingerprint.to_string(),
        audience,
        client_origin,
        expires_at_ms: now_ms + AUTH_CHALLENGE_TTL_MS,
        used: false,
    };
    let challenges = match audience {
        RemoteCredentialAudience::Pwa => &mut runtime.auth_challenges,
        RemoteCredentialAudience::PairedClientV1 => &mut runtime.client_auth_challenges,
    };
    challenges.insert(challenge_id, challenge.clone());
    challenge
}

pub fn consume_auth_challenge(
    runtime: &mut RemoteRuntimeState,
    challenge_id: &str,
    now_ms: i64,
) -> Result<AuthChallengeRecord, String> {
    consume_auth_challenge_for(runtime, challenge_id, RemoteCredentialAudience::Pwa, now_ms)
}

pub fn consume_bound_client_auth_challenge(
    runtime: &mut RemoteRuntimeState,
    challenge_id: &str,
    device_id: &str,
    canonical_gateway_origin: &str,
    gateway_identity_fingerprint: &str,
    client_origin: &str,
    now_ms: i64,
) -> Result<AuthChallengeRecord, String> {
    let challenge = runtime
        .client_auth_challenges
        .get_mut(challenge_id)
        .ok_or_else(|| "auth_challenge_not_found".to_string())?;
    if challenge.used {
        return Err("auth_challenge_used".to_string());
    }
    if now_ms >= challenge.expires_at_ms {
        return Err("auth_challenge_expired".to_string());
    }
    if challenge.audience != RemoteCredentialAudience::PairedClientV1
        || challenge.device_id != device_id
        || challenge.canonical_origin != canonical_gateway_origin
        || challenge.gateway_identity_fingerprint != gateway_identity_fingerprint
        || challenge.client_origin.as_deref() != Some(client_origin)
    {
        return Err("auth_challenge_mismatch".to_string());
    }
    challenge.used = true;
    Ok(challenge.clone())
}

fn consume_auth_challenge_for(
    runtime: &mut RemoteRuntimeState,
    challenge_id: &str,
    audience: RemoteCredentialAudience,
    now_ms: i64,
) -> Result<AuthChallengeRecord, String> {
    let challenges = match audience {
        RemoteCredentialAudience::Pwa => &mut runtime.auth_challenges,
        RemoteCredentialAudience::PairedClientV1 => &mut runtime.client_auth_challenges,
    };
    let challenge = challenges
        .get_mut(challenge_id)
        .ok_or_else(|| "auth_challenge_not_found".to_string())?;
    if challenge.used {
        return Err("auth_challenge_used".to_string());
    }
    if now_ms >= challenge.expires_at_ms && audience == RemoteCredentialAudience::PairedClientV1 {
        return Err("auth_challenge_expired".to_string());
    }
    if now_ms > challenge.expires_at_ms {
        return Err("auth_challenge_expired".to_string());
    }
    if challenge.audience != audience {
        return Err("auth_challenge_mismatch".to_string());
    }
    challenge.used = true;
    Ok(challenge.clone())
}

pub fn check_rate_limit(
    runtime: &mut RemoteRuntimeState,
    key: &str,
    now_ms: i64,
    max_attempts: usize,
    window_ms: i64,
) -> Result<(), String> {
    prune_rate_limit_buckets(runtime, now_ms);
    if !runtime.rate_limits.contains_key(key) && runtime.rate_limits.len() >= MAX_RATE_LIMIT_BUCKETS
    {
        return Err("rate_limit_capacity".to_string());
    }
    let bucket = runtime.rate_limits.entry(key.to_string()).or_default();
    check_rate_limit_bucket(bucket, now_ms, max_attempts, window_ms)
}

pub fn check_rate_limit_available(
    runtime: &mut RemoteRuntimeState,
    key: &str,
    now_ms: i64,
    max_attempts: usize,
    window_ms: i64,
) -> Result<(), String> {
    prune_rate_limit_buckets(runtime, now_ms);
    let Some(bucket) = runtime.rate_limits.get_mut(key) else {
        return Ok(());
    };
    if bucket
        .locked_until_ms
        .is_some_and(|locked_until_ms| now_ms < locked_until_ms)
    {
        return Err("rate_limited".to_string());
    }
    if bucket
        .locked_until_ms
        .is_some_and(|locked_until_ms| now_ms >= locked_until_ms)
    {
        bucket.locked_until_ms = None;
        bucket.attempts.clear();
    }
    prune_bucket_window(bucket, now_ms, window_ms);
    if bucket.attempts.len() >= max_attempts.max(1) {
        bucket.locked_until_ms = Some(now_ms + window_ms);
        return Err("rate_limited".to_string());
    }
    Ok(())
}

pub fn prune_expired_runtime_state(runtime: &mut RemoteRuntimeState, now_ms: i64) {
    runtime
        .pairing_offers
        .retain(|_, offer| !offer.used && now_ms <= offer.expires_at_ms);
    runtime.pending_pairing_requests.retain(|_, request| {
        request.decision == PendingPairingDecision::Approving
            || (now_ms <= request.expires_at_ms
                && request.decision != PendingPairingDecision::Rejected)
    });
    runtime
        .auth_challenges
        .retain(|_, challenge| !challenge.used && now_ms <= challenge.expires_at_ms);
    runtime
        .client_auth_challenges
        .retain(|_, challenge| !challenge.used && now_ms < challenge.expires_at_ms);
    runtime
        .sessions
        .retain(|_, session| session_is_active(session, now_ms));
    runtime.client_sessions.retain(|_, session| {
        session.audience == RemoteCredentialAudience::PairedClientV1
            && now_ms < session.expires_at_ms
    });
    runtime
        .websocket_tickets
        .retain(|_, ticket| !ticket.used && now_ms <= ticket.expires_at_ms);
    let active_session_ids = runtime.sessions.keys().cloned().collect::<HashSet<_>>();
    runtime
        .active_status_streams
        .retain(|session_id, _| active_session_ids.contains(session_id));
    prune_rate_limit_buckets(runtime, now_ms);
}

pub fn client_session_is_active(session: &RemoteClientSessionRecord, now_ms: i64) -> bool {
    !session.revoked
        && session.audience == RemoteCredentialAudience::PairedClientV1
        && now_ms < session.expires_at_ms
}

fn prune_rate_limit_buckets(runtime: &mut RemoteRuntimeState, now_ms: i64) {
    for bucket in runtime.rate_limits.values_mut() {
        while bucket
            .attempts
            .front()
            .is_some_and(|attempt_ms| now_ms.saturating_sub(*attempt_ms) >= RATE_LIMIT_RETAIN_MS)
        {
            bucket.attempts.pop_front();
        }
        if bucket
            .locked_until_ms
            .is_some_and(|locked_until_ms| now_ms >= locked_until_ms)
        {
            bucket.locked_until_ms = None;
        }
    }
    runtime.rate_limits.retain(|_, bucket| {
        !bucket.attempts.is_empty()
            || bucket
                .locked_until_ms
                .is_some_and(|locked_until_ms| now_ms < locked_until_ms)
    });
}

fn check_rate_limit_bucket(
    bucket: &mut RateLimitBucket,
    now_ms: i64,
    max_attempts: usize,
    window_ms: i64,
) -> Result<(), String> {
    if bucket
        .locked_until_ms
        .is_some_and(|locked_until_ms| now_ms < locked_until_ms)
    {
        return Err("rate_limited".to_string());
    }

    if bucket
        .locked_until_ms
        .is_some_and(|locked_until_ms| now_ms >= locked_until_ms)
    {
        bucket.locked_until_ms = None;
        bucket.attempts.clear();
    }

    prune_bucket_window(bucket, now_ms, window_ms);

    if bucket.attempts.len() >= max_attempts.max(1) {
        bucket.locked_until_ms = Some(now_ms + window_ms);
        return Err("rate_limited".to_string());
    }

    bucket.attempts.push_back(now_ms);
    Ok(())
}

fn prune_bucket_window(bucket: &mut RateLimitBucket, now_ms: i64, window_ms: i64) {
    while bucket
        .attempts
        .front()
        .is_some_and(|attempt_ms| now_ms.saturating_sub(*attempt_ms) >= window_ms)
    {
        bucket.attempts.pop_front();
    }
}

pub fn create_websocket_ticket(
    runtime: &mut RemoteRuntimeState,
    session: &RemoteSessionRecord,
    stream: &str,
    canonical_origin: &str,
    now_ms: i64,
) -> WebSocketTicketRecord {
    prune_expired_runtime_state(runtime, now_ms);
    let ticket = random_url_token(32);
    let record = WebSocketTicketRecord {
        ticket: ticket.clone(),
        session_id: session.session_id.clone(),
        device_id: session.device_id.clone(),
        stream: stream.to_string(),
        canonical_origin: canonical_origin.to_string(),
        expires_at_ms: now_ms + WEBSOCKET_TICKET_TTL_MS,
        used: false,
    };
    runtime.websocket_tickets.insert(ticket, record.clone());
    record
}

pub fn consume_websocket_ticket(
    runtime: &mut RemoteRuntimeState,
    ticket: &str,
    now_ms: i64,
) -> Result<WebSocketTicketRecord, String> {
    let record = runtime
        .websocket_tickets
        .get_mut(ticket)
        .ok_or_else(|| "websocket_ticket_not_found".to_string())?;
    if record.used {
        return Err("websocket_ticket_used".to_string());
    }
    if now_ms > record.expires_at_ms {
        return Err("websocket_ticket_expired".to_string());
    }
    record.used = true;
    Ok(record.clone())
}

pub fn try_open_status_stream(
    runtime: &mut RemoteRuntimeState,
    session_id: &str,
) -> Result<(), String> {
    let active = runtime
        .active_status_streams
        .entry(session_id.to_string())
        .or_insert(0);
    if *active >= MAX_ACTIVE_STATUS_STREAMS_PER_SESSION {
        return Err("websocket_connection_limit".to_string());
    }
    *active += 1;
    Ok(())
}

pub fn close_status_stream(runtime: &mut RemoteRuntimeState, session_id: &str) {
    let Some(active) = runtime.active_status_streams.get_mut(session_id) else {
        return;
    };
    *active = active.saturating_sub(1);
    if *active == 0 {
        runtime.active_status_streams.remove(session_id);
    }
}

fn millis_to_rfc3339(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::models::{DeviceRecord, RemoteRuntimeState};
    use std::{ffi::OsString, future::Future, path::Path, sync::Arc, task::Poll};
    use tokio::sync::Mutex;

    struct WardianHomeEnvGuard {
        _lock: tokio::sync::MutexGuard<'static, ()>,
        previous_home: Option<OsString>,
    }

    impl WardianHomeEnvGuard {
        async fn set(home: &Path) -> Self {
            let lock = crate::utils::wardian_test_env_lock_async().await;
            let previous_home = std::env::var_os("WARDIAN_HOME");
            std::env::set_var("WARDIAN_HOME", home);
            Self {
                _lock: lock,
                previous_home,
            }
        }
    }

    impl Drop for WardianHomeEnvGuard {
        fn drop(&mut self) {
            match self.previous_home.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    #[test]
    fn pairing_offer_is_single_use_and_expires() {
        let mut runtime = RemoteRuntimeState::default();
        let now = 1_000_000;
        let offer = create_pairing_offer(
            &mut runtime,
            "https://wardian.tailnet.ts.net",
            "server-fp",
            now,
        )
        .expect("offer");
        let payload = serde_json::to_value(&offer).expect("serialize PWA payload");
        assert!(payload.get("credential_kind").is_none());
        assert!(payload.get("client_origin").is_none());
        assert!(payload.get("read_grants").is_none());

        assert!(consume_pairing_offer(&mut runtime, &offer.pairing_offer_id, now + 1).is_ok());
        assert!(consume_pairing_offer(&mut runtime, &offer.pairing_offer_id, now + 2).is_err());

        let expired = create_pairing_offer(
            &mut runtime,
            "https://wardian.tailnet.ts.net",
            "server-fp",
            now,
        )
        .expect("offer");
        assert!(
            consume_pairing_offer(&mut runtime, &expired.pairing_offer_id, now + 121_000).is_err()
        );
    }

    #[test]
    fn client_pairing_offer_binds_exact_origin_gateway_identity_and_fixed_grants() {
        let mut runtime = RemoteRuntimeState::default();
        let offer = create_client_pairing_offer(
            &mut runtime,
            "https://wardian.tailnet.ts.net",
            "https://client.example",
            "server-fp",
            1_000_000,
        )
        .expect("client offer");
        assert_eq!(offer.credential_kind, DeviceCredentialKind::PairedClient);
        assert_eq!(offer.client_origin, "https://client.example");
        assert_eq!(
            offer.read_grants,
            [RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead]
        );
        let payload = serde_json::to_value(&offer).expect("serialize client payload");
        assert_eq!(payload["credential_kind"], "client");
        assert_eq!(
            payload["read_grants"],
            serde_json::json!(["agents:read", "chat:read"])
        );

        assert_eq!(
            consume_client_pairing_offer(
                &mut runtime,
                &offer.pairing_offer_id,
                &offer.nonce,
                "https://other-gateway.example",
                "server-fp",
                "https://client.example",
                1_000_001,
            )
            .expect_err("gateway binding mismatch rejected"),
            "pairing_offer_invalid"
        );
        assert!(consume_client_pairing_offer(
            &mut runtime,
            &offer.pairing_offer_id,
            &offer.nonce,
            "https://wardian.tailnet.ts.net",
            "server-fp",
            "https://client.example",
            1_000_002,
        )
        .is_ok());
        assert!(consume_client_pairing_offer(
            &mut runtime,
            &offer.pairing_offer_id,
            &offer.nonce,
            "https://wardian.tailnet.ts.net",
            "server-fp",
            "https://client.example",
            1_000_003,
        )
        .is_err());
    }

    #[test]
    fn client_pairing_scopes_default_to_read_and_send_requires_an_explicit_request() {
        let mut runtime = RemoteRuntimeState::default();
        let now = 1_000_000;
        let offer = create_client_pairing_offer(
            &mut runtime,
            "https://wardian.tailnet.ts.net",
            "https://client.example",
            "server-fp",
            now,
        )
        .expect("client pairing offer");
        let offer_record = runtime
            .pairing_offers
            .get(&offer.pairing_offer_id)
            .expect("stored offer")
            .clone();

        let read_only = create_pending_client_pairing_request(
            &mut runtime,
            &offer_record,
            PairingDeviceIdentity {
                device_id: "device-read-only",
                device_label: "Read-only client",
                public_key_spki_der_base64: "public-key",
                public_key_fingerprint: "read-only-fingerprint",
            },
            None,
            now + 1,
        )
        .expect("default read-only request");
        assert_eq!(
            read_only.read_grants,
            [RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead]
        );
        assert!(read_only.send_grants.is_empty());

        let requested = [RemoteClientScope::AgentsRead, RemoteClientScope::ChatSend];
        let send_enabled = create_pending_client_pairing_request(
            &mut runtime,
            &offer_record,
            PairingDeviceIdentity {
                device_id: "device-send-enabled",
                device_label: "Send-enabled client",
                public_key_spki_der_base64: "public-key",
                public_key_fingerprint: "send-fingerprint",
            },
            Some(&requested),
            now + 2,
        )
        .expect("explicit send-grant request");
        assert_eq!(send_enabled.read_grants, [RemoteReadGrant::AgentsRead]);
        assert_eq!(send_enabled.send_grants, [RemoteSendGrant::ChatSend]);

        assert_eq!(
            create_pending_client_pairing_request(
                &mut runtime,
                &offer_record,
                PairingDeviceIdentity {
                    device_id: "device-empty-scopes",
                    device_label: "Invalid client",
                    public_key_spki_der_base64: "public-key",
                    public_key_fingerprint: "empty-fingerprint",
                },
                Some(&[]),
                now + 3,
            )
            .expect_err("explicit empty scope request is invalid"),
            "invalid_request"
        );
        let duplicate = [RemoteClientScope::ChatSend, RemoteClientScope::ChatSend];
        assert_eq!(
            create_pending_client_pairing_request(
                &mut runtime,
                &offer_record,
                PairingDeviceIdentity {
                    device_id: "device-duplicate-scopes",
                    device_label: "Invalid client",
                    public_key_spki_der_base64: "public-key",
                    public_key_fingerprint: "duplicate-fingerprint",
                },
                Some(&duplicate),
                now + 4,
            )
            .expect_err("duplicate scope request is invalid"),
            "invalid_request"
        );
    }

    #[test]
    fn session_expiry_checks_idle_and_absolute_lifetime() {
        let session = create_session_record("dev-1", 1_000_000);
        assert!(session_is_active(&session, 1_000_000 + 60_000));
        assert!(!session_is_active(
            &session,
            1_000_000 + (13 * 60 * 60 * 1000)
        ));
    }

    #[test]
    fn session_runtime_helpers_store_check_csrf_and_revoke() {
        let mut runtime = RemoteRuntimeState::default();
        let session = create_session(&mut runtime, "dev-1", 1_000_000);

        assert!(runtime.sessions.contains_key(&session.session_id));
        assert!(csrf_nonce_matches(&session, &session.csrf_nonce));
        assert!(!csrf_nonce_matches(&session, "wrong-nonce"));

        assert_eq!(revoke_sessions_for_device(&mut runtime, "dev-1"), 1);
        let stored = runtime.sessions.get(&session.session_id).expect("session");
        assert!(!session_is_active(stored, 1_001_000));
        assert!(!csrf_nonce_matches(stored, &session.csrf_nonce));
    }

    #[tokio::test]
    async fn device_revocation_serializes_with_session_issuance() {
        let temp = tempfile::tempdir().expect("temp dir");
        let _home = WardianHomeEnvGuard::set(temp.path()).await;
        crate::remote::storage::update_device_store_at(temp.path(), |store| {
            store.devices.push(DeviceRecord {
                device_id: "dev-1".to_string(),
                label: "Phone".to_string(),
                public_key_spki_der_base64: "key".to_string(),
                public_key_fingerprint: "fp".to_string(),
                credential_kind: DeviceCredentialKind::Pwa,
                audience: RemoteCredentialAudience::Pwa,
                client_origin: None,
                read_grants: Vec::new(),
                send_grants: Vec::new(),
                created_at: "2026-05-21T00:00:00Z".to_string(),
                last_used_at: None,
                revoked_at: None,
            });
            Ok(())
        })
        .expect("seed active device");

        let device_store_lock = Arc::new(Mutex::new(()));
        let runtime = Mutex::new(RemoteRuntimeState::default());
        let held = device_store_lock.clone().lock_owned().await;

        let mut issue = Box::pin(issue_session_for_active_device(
            device_store_lock.clone(),
            &runtime,
            "dev-1".to_string(),
            "2026-05-21T00:01:00Z".to_string(),
            1_000_000,
        ));
        assert!(
            std::future::poll_fn(|context| {
                Poll::Ready(issue.as_mut().poll(context).is_pending())
            })
            .await,
            "session issuance waits behind the held device transition gate"
        );

        let mut revoke = Box::pin(revoke_device_and_sessions(
            device_store_lock.clone(),
            &runtime,
            "dev-1".to_string(),
            "2026-05-21T00:02:00Z".to_string(),
        ));
        assert!(
            std::future::poll_fn(|context| {
                Poll::Ready(revoke.as_mut().poll(context).is_pending())
            })
            .await,
            "revocation queues behind session issuance"
        );

        drop(held);
        let session = issue.await.expect("session issued before revocation");
        let revoked_store = revoke.await.expect("device revoked after issuance");

        assert_eq!(
            revoked_store.devices[0].revoked_at.as_deref(),
            Some("2026-05-21T00:02:00Z")
        );
        let runtime_guard = runtime.lock().await;
        assert!(
            runtime_guard
                .sessions
                .get(&session.session_id)
                .expect("issued session retained for observation")
                .revoked
        );
        drop(runtime_guard);

        let error = issue_session_for_active_device(
            device_store_lock,
            &runtime,
            "dev-1".to_string(),
            "2026-05-21T00:03:00Z".to_string(),
            1_001_000,
        )
        .await
        .expect_err("a revoked credential cannot issue a later session");
        assert_eq!(error.message, "remote_device_revoked");
        assert_eq!(runtime.lock().await.sessions.len(), 1);
    }

    #[test]
    fn create_session_replaces_existing_session_for_device() {
        let mut runtime = RemoteRuntimeState::default();
        let first = create_session(&mut runtime, "dev-1", 1_000_000);
        let second = create_session(&mut runtime, "dev-1", 1_001_000);

        assert_ne!(first.session_id, second.session_id);
        assert!(!runtime.sessions.contains_key(&first.session_id));
        assert!(runtime.sessions.contains_key(&second.session_id));
    }

    #[test]
    fn status_stream_count_is_limited_per_session_and_closes() {
        let mut runtime = RemoteRuntimeState::default();

        assert!(try_open_status_stream(&mut runtime, "sess-1").is_ok());
        assert!(try_open_status_stream(&mut runtime, "sess-1").is_ok());
        assert!(try_open_status_stream(&mut runtime, "sess-1").is_ok());
        assert_eq!(
            try_open_status_stream(&mut runtime, "sess-1").expect_err("fourth stream rejected"),
            "websocket_connection_limit"
        );

        close_status_stream(&mut runtime, "sess-1");

        assert!(try_open_status_stream(&mut runtime, "sess-1").is_ok());
    }

    #[test]
    fn websocket_ticket_is_single_use_and_expires() {
        let mut runtime = RemoteRuntimeState::default();
        let session = create_session_record("dev-1", 1_000_000);
        let ticket = create_websocket_ticket(
            &mut runtime,
            &session,
            "agent_status",
            "https://wardian.tailnet.ts.net",
            1_000_000,
        );

        assert!(consume_websocket_ticket(&mut runtime, &ticket.ticket, 1_001_000).is_ok());
        assert!(consume_websocket_ticket(&mut runtime, &ticket.ticket, 1_002_000).is_err());

        let expired = create_websocket_ticket(
            &mut runtime,
            &session,
            "agent_status",
            "https://wardian.tailnet.ts.net",
            1_000_000,
        );
        assert!(consume_websocket_ticket(&mut runtime, &expired.ticket, 1_061_000).is_err());
    }

    #[test]
    fn auth_challenge_is_single_use_and_expires() {
        let mut runtime = RemoteRuntimeState::default();
        let challenge = create_auth_challenge(
            &mut runtime,
            "dev-1",
            "https://wardian.tailnet.ts.net",
            "server-fp",
            1_000_000,
        );

        assert!(consume_auth_challenge(&mut runtime, &challenge.challenge_id, 1_001_000).is_ok());
        assert_eq!(
            consume_auth_challenge(&mut runtime, &challenge.challenge_id, 1_002_000)
                .expect_err("used challenge rejected"),
            "auth_challenge_used"
        );

        let expired = create_auth_challenge(
            &mut runtime,
            "dev-1",
            "https://wardian.tailnet.ts.net",
            "server-fp",
            1_000_000,
        );
        assert_eq!(
            consume_auth_challenge(&mut runtime, &expired.challenge_id, 1_061_000)
                .expect_err("expired challenge rejected"),
            "auth_challenge_expired"
        );
    }

    #[test]
    fn client_auth_challenges_are_separate_and_binding_mismatch_does_not_consume() {
        let mut runtime = RemoteRuntimeState::default();
        let challenge = create_client_auth_challenge(
            &mut runtime,
            "dev-client",
            "https://wardian.tailnet.ts.net",
            "server-fp",
            "https://client.example",
            1_000_000,
        )
        .expect("client challenge");
        assert!(runtime.auth_challenges.is_empty());
        assert_eq!(runtime.client_auth_challenges.len(), 1);
        assert_eq!(
            consume_auth_challenge(&mut runtime, &challenge.challenge_id, 1_000_001)
                .expect_err("PWA namespace cannot consume client challenge"),
            "auth_challenge_not_found"
        );
        assert_eq!(
            consume_bound_client_auth_challenge(
                &mut runtime,
                &challenge.challenge_id,
                "dev-client",
                "https://wardian.tailnet.ts.net",
                "server-fp",
                "https://other.example",
                1_000_001,
            )
            .expect_err("origin mismatch rejected without consuming"),
            "auth_challenge_mismatch"
        );
        assert!(!runtime.client_auth_challenges[&challenge.challenge_id].used);
        assert!(consume_bound_client_auth_challenge(
            &mut runtime,
            &challenge.challenge_id,
            "dev-client",
            "https://wardian.tailnet.ts.net",
            "server-fp",
            "https://client.example",
            1_000_002,
        )
        .is_ok());
        assert_eq!(
            consume_bound_client_auth_challenge(
                &mut runtime,
                &challenge.challenge_id,
                "dev-client",
                "https://wardian.tailnet.ts.net",
                "server-fp",
                "https://client.example",
                1_000_003,
            )
            .expect_err("used challenge rejected"),
            "auth_challenge_used"
        );
    }

    #[tokio::test]
    async fn concurrent_client_challenge_consumption_issues_one_winner() {
        let mut runtime = RemoteRuntimeState::default();
        let challenge = create_client_auth_challenge(
            &mut runtime,
            "dev-client",
            "https://wardian.tailnet.ts.net",
            "server-fp",
            "https://client.example",
            1_000_000,
        )
        .expect("client challenge");
        let runtime = Arc::new(Mutex::new(runtime));
        let consume = |runtime: Arc<Mutex<RemoteRuntimeState>>| {
            let challenge_id = challenge.challenge_id.clone();
            async move {
                let mut runtime = runtime.lock().await;
                consume_bound_client_auth_challenge(
                    &mut runtime,
                    &challenge_id,
                    "dev-client",
                    "https://wardian.tailnet.ts.net",
                    "server-fp",
                    "https://client.example",
                    1_000_001,
                )
            }
        };
        let (first, second) = tokio::join!(consume(runtime.clone()), consume(runtime.clone()));
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert_eq!(
            usize::from(first.is_err()) + usize::from(second.is_err()),
            1
        );
    }

    #[tokio::test]
    async fn client_token_issuance_serializes_with_device_revocation() {
        use crate::remote::models::{
            DeviceCredentialKind, RemoteCredentialAudience, RemoteReadGrant,
        };

        let temp = tempfile::tempdir().expect("temp home");
        let _home = WardianHomeEnvGuard::set(temp.path()).await;
        crate::remote::storage::update_device_store_at(temp.path(), |store| {
            store.devices.push(DeviceRecord {
                device_id: "client-device".to_string(),
                label: "Companion".to_string(),
                public_key_spki_der_base64: "cHVi".to_string(),
                public_key_fingerprint: "fp-client".to_string(),
                credential_kind: DeviceCredentialKind::PairedClient,
                audience: RemoteCredentialAudience::PairedClientV1,
                client_origin: Some("https://client.example".to_string()),
                read_grants: vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
                send_grants: Vec::new(),
                created_at: "2026-10-30T00:00:00Z".to_string(),
                last_used_at: None,
                revoked_at: None,
            });
            Ok(())
        })
        .expect("seed paired client");

        let device_store_lock = Arc::new(Mutex::new(()));
        let runtime = Arc::new(Mutex::new(RemoteRuntimeState::default()));
        let runtime_guard = runtime.lock().await;
        let issue = ClientAccessTokenIssue {
            device_id: "client-device".to_string(),
            client_origin: "https://client.example".to_string(),
            gateway_origin: "https://gateway.example".to_string(),
            gateway_identity_fingerprint: "server-fp".to_string(),
            challenge_expires_at_ms: chrono::Utc::now().timestamp_millis() + 60_000,
            last_used_at: "2026-10-30T12:00:00Z".to_string(),
        };
        let issue_lock = device_store_lock.clone();
        let issue_runtime = runtime.clone();
        let issuance = tokio::spawn(async move {
            issue_client_access_token_for_active_device(issue_lock, &issue_runtime, issue).await
        });

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let store = crate::remote::storage::load_device_store_at(temp.path())
                    .expect("load device store while issuance is paused");
                if store.devices[0].last_used_at.as_deref() == Some("2026-10-30T12:00:00Z") {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("issuance reaches runtime insertion while holding transition gate");

        let (revoke_started, revoke_started_rx) = tokio::sync::oneshot::channel();
        let revoke_lock = device_store_lock.clone();
        let revoke_runtime = runtime.clone();
        let revocation = tokio::spawn(async move {
            let _ = revoke_started.send(());
            revoke_device_and_sessions(
                revoke_lock,
                &revoke_runtime,
                "client-device".to_string(),
                "2026-10-30T12:01:00Z".to_string(),
            )
            .await
        });
        revoke_started_rx.await.expect("revocation task started");
        drop(runtime_guard);

        let (access_token, issued_session) = issuance
            .await
            .expect("issuance task")
            .expect("issuance completes before queued revocation");
        revocation
            .await
            .expect("revocation task")
            .expect("revocation publishes");

        let store = crate::remote::storage::load_device_store_at(temp.path())
            .expect("load device store after revocation");
        assert_eq!(
            store.devices[0].revoked_at.as_deref(),
            Some("2026-10-30T12:01:00Z")
        );
        assert!(store.devices[0].last_used_at.is_some());
        let runtime_state = runtime.lock().await;
        let revoked_session = runtime_state
            .client_sessions
            .get(&crate::remote::crypto::sha256_token_verifier(&access_token))
            .expect("issued verifier retained for revocation");
        assert_eq!(revoked_session.session_id, issued_session.session_id);
        assert!(revoked_session.revoked);
        assert!(!client_session_is_active(
            revoked_session,
            chrono::Utc::now().timestamp_millis()
        ));
        drop(runtime_state);

        let reissue_error = issue_client_access_token_for_active_device(
            device_store_lock,
            &runtime,
            ClientAccessTokenIssue {
                device_id: "client-device".to_string(),
                client_origin: "https://client.example".to_string(),
                gateway_origin: "https://gateway.example".to_string(),
                gateway_identity_fingerprint: "server-fp".to_string(),
                challenge_expires_at_ms: chrono::Utc::now().timestamp_millis() + 60_000,
                last_used_at: "2026-10-30T12:02:00Z".to_string(),
            },
        )
        .await
        .expect_err("completed revocation blocks new client token issuance");
        assert_eq!(reissue_error, "remote_device_revoked");
    }

    #[test]
    fn client_token_expiry_is_fixed_and_exclusive_at_expiration() {
        let session = RemoteClientSessionRecord {
            session_id: "audit-id".to_string(),
            device_id: "dev-client".to_string(),
            client_origin: "https://client.example".to_string(),
            gateway_origin: "https://gateway.example".to_string(),
            audience: RemoteCredentialAudience::PairedClientV1,
            gateway_identity_fingerprint: "server-fp".to_string(),
            read_grants: vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            send_grants: Vec::new(),
            idempotency_namespace: None,
            idempotency_expires_at_ms: None,
            expires_at_ms: 900_000,
            revoked: false,
        };
        assert!(client_session_is_active(&session, 899_999));
        assert!(!client_session_is_active(&session, 900_000));
    }

    fn create_client_access_token_at(
        runtime: &mut RemoteRuntimeState,
        issue: ClientAccessTokenIssue,
        read_grants: Vec<RemoteReadGrant>,
        issued_at_ms: i64,
    ) -> Result<(String, RemoteClientSessionRecord), String> {
        create_client_access_token_with_grants_at(
            runtime,
            issue,
            read_grants,
            Vec::new(),
            None,
            issued_at_ms,
        )
    }

    #[test]
    fn client_token_issuance_rechecks_challenge_expiry_at_creation_time() {
        let issue = |challenge_expires_at_ms| ClientAccessTokenIssue {
            device_id: "dev-client".to_string(),
            client_origin: "https://client.example".to_string(),
            gateway_origin: "https://gateway.example".to_string(),
            gateway_identity_fingerprint: "server-fp".to_string(),
            challenge_expires_at_ms,
            last_used_at: "2026-05-21T00:00:00Z".to_string(),
        };
        let grants = vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead];
        let mut runtime = RemoteRuntimeState::default();

        assert_eq!(
            create_client_access_token_at(&mut runtime, issue(1_000), grants.clone(), 1_000)
                .expect_err("challenge expires at the issuance instant"),
            "auth_challenge_expired"
        );
        assert!(runtime.client_sessions.is_empty());

        let (token, session) =
            create_client_access_token_at(&mut runtime, issue(1_000), grants, 999)
                .expect("challenge remains valid immediately before expiry");
        assert!(!token.is_empty());
        assert_eq!(session.expires_at_ms, 999 + CLIENT_ACCESS_TOKEN_TTL_MS);
        assert_eq!(runtime.client_sessions.len(), 1);
    }

    #[test]
    fn revoked_client_verifiers_remain_for_device_revoked_response_until_expiry() {
        let mut runtime = RemoteRuntimeState::default();
        let token_verifier = "revoked-verifier".to_string();
        let session = RemoteClientSessionRecord {
            session_id: "revoked-audit-id".to_string(),
            device_id: "dev-client".to_string(),
            client_origin: "https://client.example".to_string(),
            gateway_origin: "https://gateway.example".to_string(),
            audience: RemoteCredentialAudience::PairedClientV1,
            gateway_identity_fingerprint: "server-fp".to_string(),
            read_grants: vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            send_grants: Vec::new(),
            idempotency_namespace: None,
            idempotency_expires_at_ms: None,
            expires_at_ms: 900_000,
            revoked: false,
        };
        runtime.client_sessions.insert(token_verifier, session);

        assert_eq!(revoke_sessions_for_device(&mut runtime, "dev-client"), 1);
        assert!(runtime.client_sessions["revoked-verifier"].revoked);
        assert!(!client_session_is_active(
            &runtime.client_sessions["revoked-verifier"],
            899_999,
        ));

        prune_expired_runtime_state(&mut runtime, 899_999);
        assert_eq!(runtime.client_sessions.len(), 1);
        prune_expired_runtime_state(&mut runtime, 900_000);
        assert!(runtime.client_sessions.is_empty());
    }

    #[test]
    fn client_runtime_authority_is_invalidated_by_gateway_or_allowlist_changes() {
        let mut runtime = RemoteRuntimeState::default();
        create_client_auth_challenge(
            &mut runtime,
            "dev-client",
            "https://gateway.example",
            "server-fp",
            "https://client.example",
            500_000,
        )
        .expect("client challenge");
        let token_verifier = "verifier-only".to_string();
        let session = RemoteClientSessionRecord {
            session_id: "audit-id".to_string(),
            device_id: "dev-client".to_string(),
            client_origin: "https://client.example".to_string(),
            gateway_origin: "https://gateway.example".to_string(),
            audience: RemoteCredentialAudience::PairedClientV1,
            gateway_identity_fingerprint: "server-fp".to_string(),
            read_grants: vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            send_grants: Vec::new(),
            idempotency_namespace: None,
            idempotency_expires_at_ms: None,
            expires_at_ms: 900_000,
            revoked: false,
        };
        runtime.client_sessions.insert(token_verifier, session);
        let previous = crate::remote::models::RemoteGatewayConfig {
            schema_version: crate::remote::models::REMOTE_SETTINGS_SCHEMA_VERSION,
            enabled: true,
            canonical_origin: "https://gateway.example".to_string(),
            client_origin_allowlist: vec!["https://client.example".to_string()],
            loopback_host: "127.0.0.1".to_string(),
            loopback_port: 41241,
            gateway_identity_public_key: "server-key".to_string(),
            gateway_identity_fingerprint: "server-fp".to_string(),
        };
        let mut added_origin = previous.clone();
        added_origin
            .client_origin_allowlist
            .push("https://other.example".to_string());
        invalidate_client_authority_for_config_change(&mut runtime, Some(&previous), &added_origin);
        assert_eq!(runtime.client_sessions.len(), 1);

        let removed_origin = previous.client_origin_allowlist[0].clone();
        let mut removed_config = previous.clone();
        removed_config.client_origin_allowlist.clear();
        invalidate_client_authority_for_config_change(
            &mut runtime,
            Some(&previous),
            &removed_config,
        );
        assert!(runtime.client_sessions.is_empty());
        assert!(runtime.client_auth_challenges.is_empty());

        invalidate_client_authority_for_config_change(
            &mut runtime,
            Some(&removed_config),
            &previous,
        );
        assert!(runtime.client_auth_challenges.is_empty());

        let token_verifier = "verifier-only-2".to_string();
        let session = RemoteClientSessionRecord {
            session_id: "audit-id-2".to_string(),
            device_id: "dev-client".to_string(),
            client_origin: removed_origin,
            gateway_origin: "https://gateway.example".to_string(),
            audience: RemoteCredentialAudience::PairedClientV1,
            gateway_identity_fingerprint: "server-fp".to_string(),
            read_grants: vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            send_grants: Vec::new(),
            idempotency_namespace: None,
            idempotency_expires_at_ms: None,
            expires_at_ms: 900_000,
            revoked: false,
        };
        runtime.client_sessions.insert(token_verifier, session);
        let mut changed_identity = removed_config.clone();
        changed_identity.gateway_identity_fingerprint = "replacement-fp".to_string();
        invalidate_client_authority_for_config_change(
            &mut runtime,
            Some(&removed_config),
            &changed_identity,
        );
        assert!(runtime.client_sessions.is_empty());

        let token_verifier = "verifier-only-3".to_string();
        let session = RemoteClientSessionRecord {
            session_id: "audit-id-3".to_string(),
            device_id: "dev-client".to_string(),
            client_origin: "https://client.example".to_string(),
            gateway_origin: "https://gateway.example".to_string(),
            audience: RemoteCredentialAudience::PairedClientV1,
            gateway_identity_fingerprint: "server-fp".to_string(),
            read_grants: vec![RemoteReadGrant::AgentsRead, RemoteReadGrant::ChatRead],
            send_grants: Vec::new(),
            idempotency_namespace: None,
            idempotency_expires_at_ms: None,
            expires_at_ms: 900_000,
            revoked: false,
        };
        runtime.client_sessions.insert(token_verifier, session);
        let mut changed_origin = removed_config.clone();
        changed_origin.canonical_origin = "https://replacement-gateway.example".to_string();
        invalidate_client_authority_for_config_change(
            &mut runtime,
            Some(&removed_config),
            &changed_origin,
        );
        assert!(runtime.client_sessions.is_empty());

        invalidate_client_authority_for_config_change(&mut runtime, None, &previous);
        assert!(runtime.client_sessions.is_empty());
    }

    #[test]
    fn session_activity_refreshes_idle_expiry_until_absolute_lifetime() {
        let mut session = create_session_record("dev-1", 1_000_000);
        let original_expires_at = session.expires_at_ms;
        let absolute_expires_at = session.absolute_expires_at_ms;

        refresh_session_activity(&mut session, 1_600_000).expect("refresh before idle expiry");

        assert_eq!(session.last_seen_at_ms, 1_600_000);
        assert!(session.expires_at_ms > original_expires_at);
        assert!(session.expires_at_ms <= absolute_expires_at);
    }

    #[test]
    fn rate_limit_locks_after_max_attempts_in_window() {
        let mut runtime = RemoteRuntimeState::default();

        assert!(check_rate_limit(&mut runtime, "auth:dev-1", 1_000, 2, 60_000).is_ok());
        assert!(check_rate_limit(&mut runtime, "auth:dev-1", 2_000, 2, 60_000).is_ok());
        assert_eq!(
            check_rate_limit(&mut runtime, "auth:dev-1", 3_000, 2, 60_000)
                .expect_err("third attempt locked"),
            "rate_limited"
        );
        assert_eq!(
            check_rate_limit(&mut runtime, "auth:dev-1", 4_000, 2, 60_000)
                .expect_err("locked attempt rejected"),
            "rate_limited"
        );

        assert!(check_rate_limit(&mut runtime, "auth:dev-1", 65_000, 2, 60_000).is_ok());
    }

    #[test]
    fn rate_limit_availability_does_not_record_successful_attempts() {
        let mut runtime = RemoteRuntimeState::default();

        assert!(check_rate_limit_available(&mut runtime, "auth:dev-1", 1_000, 2, 60_000).is_ok());
        assert!(check_rate_limit_available(&mut runtime, "auth:dev-1", 2_000, 2, 60_000).is_ok());
        assert!(check_rate_limit(&mut runtime, "auth:dev-1", 3_000, 2, 60_000).is_ok());
        assert!(check_rate_limit(&mut runtime, "auth:dev-1", 4_000, 2, 60_000).is_ok());
        assert_eq!(
            check_rate_limit_available(&mut runtime, "auth:dev-1", 5_000, 2, 60_000)
                .expect_err("filled bucket rejected before recording another attempt"),
            "rate_limited"
        );
    }

    #[test]
    fn prune_expired_runtime_state_removes_used_expired_and_revoked_records() {
        let mut runtime = RemoteRuntimeState::default();
        let now = 1_000_000;
        let offer = create_pairing_offer(
            &mut runtime,
            "https://wardian.tailnet.ts.net",
            "server-fp",
            now,
        )
        .expect("offer");
        let challenge = create_auth_challenge(
            &mut runtime,
            "dev-1",
            "https://wardian.tailnet.ts.net",
            "server-fp",
            now,
        );
        let mut session = create_session(&mut runtime, "dev-1", now);
        let ticket = create_websocket_ticket(
            &mut runtime,
            &session,
            "agent_status",
            "https://wardian.tailnet.ts.net",
            now,
        );

        consume_pairing_offer(&mut runtime, &offer.pairing_offer_id, now + 1).expect("offer used");
        consume_auth_challenge(&mut runtime, &challenge.challenge_id, now + 1)
            .expect("challenge used");
        consume_websocket_ticket(&mut runtime, &ticket.ticket, now + 1).expect("ticket used");
        session.revoked = true;
        runtime.sessions.insert(session.session_id.clone(), session);

        prune_expired_runtime_state(&mut runtime, now + 1);

        assert!(runtime.pairing_offers.is_empty());
        assert!(runtime.pending_pairing_requests.is_empty());
        assert!(runtime.auth_challenges.is_empty());
        assert!(runtime.websocket_tickets.is_empty());
        assert!(runtime.sessions.is_empty());
    }

    #[test]
    fn pending_pairing_requires_explicit_approval_or_rejection() {
        let mut runtime = RemoteRuntimeState::default();
        let now = 1_000_000;
        let offer = create_pairing_offer(
            &mut runtime,
            "https://wardian.tailnet.ts.net",
            "server-fp",
            now,
        )
        .expect("offer");
        let offer_record =
            consume_pairing_offer(&mut runtime, &offer.pairing_offer_id, now + 1).expect("offer");
        let request = create_pending_pairing_request(
            &mut runtime,
            &offer_record,
            "dev-1",
            "Pixel phone",
            "spki",
            "fp:phone",
            now + 2,
        );

        assert!(pending_pairing_request_is_active(&request, now + 2));
        assert_eq!(
            runtime
                .pending_pairing_requests
                .get(&request.request_id)
                .expect("pending request")
                .decision,
            PendingPairingDecision::Pending
        );

        let approving = begin_pending_pairing_approval(&mut runtime, &request.request_id, now + 3)
            .expect("approval started");
        assert_eq!(approving.decision, PendingPairingDecision::Approving);
        prune_expired_runtime_state(&mut runtime, now + 121_000);
        assert!(runtime
            .pending_pairing_requests
            .contains_key(&request.request_id));

        let approved = mark_pending_pairing_approved(
            &mut runtime,
            &request.request_id,
            "2026-05-21T00:00:00.000Z",
        )
        .expect("approved");
        assert_eq!(approved.decision, PendingPairingDecision::Approved);
        assert_eq!(
            approved.paired_at.as_deref(),
            Some("2026-05-21T00:00:00.000Z")
        );
    }

    #[test]
    fn failed_pairing_approval_releases_or_expires_its_reservation() {
        let mut runtime = RemoteRuntimeState::default();
        let now = 1_000_000;
        let offer = create_pairing_offer(
            &mut runtime,
            "https://wardian.tailnet.ts.net",
            "server-fp",
            now,
        )
        .expect("offer");
        let offer_record =
            consume_pairing_offer(&mut runtime, &offer.pairing_offer_id, now + 1).expect("offer");
        let request = create_pending_pairing_request(
            &mut runtime,
            &offer_record,
            "dev-1",
            "Phone",
            "spki",
            "fp:phone",
            now + 2,
        );

        begin_pending_pairing_approval(&mut runtime, &request.request_id, now + 3)
            .expect("approval started");
        mark_pending_pairing_approval_failed(&mut runtime, &request.request_id, now + 4);
        assert!(pending_pairing_request_is_active(
            runtime
                .pending_pairing_requests
                .get(&request.request_id)
                .expect("approval returned to pending"),
            now + 4
        ));

        begin_pending_pairing_approval(&mut runtime, &request.request_id, now + 5)
            .expect("approval restarted");
        mark_pending_pairing_approval_failed(
            &mut runtime,
            &request.request_id,
            request.expires_at_ms + 1,
        );
        assert!(!runtime
            .pending_pairing_requests
            .contains_key(&request.request_id));
    }

    #[test]
    fn rate_limit_rejects_new_keys_when_bucket_cap_is_reached() {
        let mut runtime = RemoteRuntimeState::default();

        for idx in 0..MAX_RATE_LIMIT_BUCKETS {
            assert!(
                check_rate_limit(&mut runtime, &format!("auth:{idx}"), 1_000, 1, 60_000).is_ok()
            );
        }

        assert_eq!(
            check_rate_limit(&mut runtime, "auth:overflow", 2_000, 1, 60_000)
                .expect_err("new bucket rejected"),
            "rate_limit_capacity"
        );

        assert_eq!(
            check_rate_limit(&mut runtime, "auth:0", 2_000, 1, 60_000)
                .expect_err("existing bucket keeps normal limit"),
            "rate_limited"
        );
    }
}
