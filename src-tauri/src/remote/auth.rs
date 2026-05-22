use crate::remote::crypto::random_url_token;
use crate::remote::models::{
    AuthChallengeRecord, PairingOfferRecord, PairingQrPayload, PendingPairingDecision,
    PendingPairingRequestRecord, RateLimitBucket, RemoteRuntimeState, RemoteSessionRecord,
    WebSocketTicketRecord,
};
use std::collections::HashSet;
use subtle::ConstantTimeEq;

const PAIRING_OFFER_TTL_MS: i64 = 120_000;
const AUTH_CHALLENGE_TTL_MS: i64 = 60_000;
const ACCESS_SESSION_TTL_MS: i64 = 15 * 60 * 1000;
const ABSOLUTE_SESSION_TTL_MS: i64 = 12 * 60 * 60 * 1000;
const WEBSOCKET_TICKET_TTL_MS: i64 = 60_000;
const PENDING_PAIRING_TTL_MS: i64 = 120_000;
const RATE_LIMIT_RETAIN_MS: i64 = 10 * 60 * 1000;
const MAX_RATE_LIMIT_BUCKETS: usize = 1024;
const MAX_ACTIVE_STATUS_STREAMS_PER_SESSION: usize = 3;

pub fn create_pairing_offer(
    runtime: &mut RemoteRuntimeState,
    canonical_origin: &str,
    server_fingerprint: &str,
    now_ms: i64,
) -> Result<PairingQrPayload, String> {
    prune_expired_runtime_state(runtime, now_ms);
    let offer_id = random_url_token(18);
    let nonce = random_url_token(18);
    runtime.pairing_offers.insert(
        offer_id.clone(),
        PairingOfferRecord {
            offer_id: offer_id.clone(),
            nonce: nonce.clone(),
            canonical_origin: canonical_origin.to_string(),
            expires_at_ms: now_ms + PAIRING_OFFER_TTL_MS,
            used: false,
        },
    );
    Ok(PairingQrPayload {
        gateway_origin: canonical_origin.to_string(),
        pairing_offer_id: offer_id,
        expires_at: millis_to_rfc3339(now_ms + PAIRING_OFFER_TTL_MS),
        nonce,
        server_identity_fingerprint: server_fingerprint.to_string(),
    })
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
    offer.used = true;
    Ok(offer.clone())
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
    prune_expired_runtime_state(runtime, now_ms);
    let request = PendingPairingRequestRecord {
        request_id: random_url_token(18),
        pairing_offer_id: offer.offer_id.clone(),
        device_id: device_id.to_string(),
        device_label: device_label.to_string(),
        public_key_spki_der_base64: public_key_spki_der_base64.to_string(),
        public_key_fingerprint: public_key_fingerprint.to_string(),
        canonical_origin: offer.canonical_origin.clone(),
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

pub fn mark_pending_pairing_approved(
    runtime: &mut RemoteRuntimeState,
    request_id: &str,
    paired_at: &str,
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
    request.decision = PendingPairingDecision::Approved;
    request.paired_at = Some(paired_at.to_string());
    Ok(request.clone())
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
    revoked
}

pub fn create_auth_challenge(
    runtime: &mut RemoteRuntimeState,
    device_id: &str,
    canonical_origin: &str,
    now_ms: i64,
) -> AuthChallengeRecord {
    prune_expired_runtime_state(runtime, now_ms);
    let challenge_id = random_url_token(18);
    let challenge = AuthChallengeRecord {
        challenge_id: challenge_id.clone(),
        device_id: device_id.to_string(),
        nonce: random_url_token(18),
        canonical_origin: canonical_origin.to_string(),
        expires_at_ms: now_ms + AUTH_CHALLENGE_TTL_MS,
        used: false,
    };
    runtime
        .auth_challenges
        .insert(challenge_id, challenge.clone());
    challenge
}

pub fn consume_auth_challenge(
    runtime: &mut RemoteRuntimeState,
    challenge_id: &str,
    now_ms: i64,
) -> Result<AuthChallengeRecord, String> {
    let challenge = runtime
        .auth_challenges
        .get_mut(challenge_id)
        .ok_or_else(|| "auth_challenge_not_found".to_string())?;
    if challenge.used {
        return Err("auth_challenge_used".to_string());
    }
    if now_ms > challenge.expires_at_ms {
        return Err("auth_challenge_expired".to_string());
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
        now_ms <= request.expires_at_ms && request.decision != PendingPairingDecision::Rejected
    });
    runtime
        .auth_challenges
        .retain(|_, challenge| !challenge.used && now_ms <= challenge.expires_at_ms);
    runtime
        .sessions
        .retain(|_, session| session_is_active(session, now_ms));
    runtime
        .websocket_tickets
        .retain(|_, ticket| !ticket.used && now_ms <= ticket.expires_at_ms);
    let active_session_ids = runtime.sessions.keys().cloned().collect::<HashSet<_>>();
    runtime
        .active_status_streams
        .retain(|session_id, _| active_session_ids.contains(session_id));
    prune_rate_limit_buckets(runtime, now_ms);
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
    use crate::remote::models::RemoteRuntimeState;

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
            1_000_000,
        );
        assert_eq!(
            consume_auth_challenge(&mut runtime, &expired.challenge_id, 1_061_000)
                .expect_err("expired challenge rejected"),
            "auth_challenge_expired"
        );
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
        let challenge =
            create_auth_challenge(&mut runtime, "dev-1", "https://wardian.tailnet.ts.net", now);
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

        let approved = mark_pending_pairing_approved(
            &mut runtime,
            &request.request_id,
            "2026-05-21T00:00:00.000Z",
            now + 3,
        )
        .expect("approved");
        assert_eq!(approved.decision, PendingPairingDecision::Approved);
        assert_eq!(
            approved.paired_at.as_deref(),
            Some("2026-05-21T00:00:00.000Z")
        );
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
