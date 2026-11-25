// Fixture-only helpers included by the owning module during tests.

pub(crate) struct SourceReplayRequest {
    pub(crate) previous: ProviderLogCaptureState,
    pub(crate) expected_identity: ProviderLogNativeIdentity,
    pub(crate) target_extent: u64,
    pub(crate) anchor_floor: u64,
}

/// Replay an independently pinned source interval using the same native
/// identity, framing, and normalizer as foreground provider acquisition.
/// This cursor is owned by SourceIndex; it never reads or advances the primary
/// provider-capture cursor.
pub(crate) fn acquire_source_replay_batch(
    source: ProviderLogSource<'_>,
    request: SourceReplayRequest,
) -> io::Result<(ProviderLogBatch, ProviderLogReplayMetrics)> {
    let mut metrics = ProviderLogReplayMetrics::default();
    let batch = acquire_provider_log_batch_bounded(
        source,
        ProviderLogBatchOptions {
            previous: Some(request.previous),
            trust_source_from_start: true,
            expected_identity: Some(&request.expected_identity),
            target_extent: Some(request.target_extent),
            max_records: Some(80),
            anchor_floor: request.anchor_floor,
            recover_legacy_rows: false,
            emit_source_proof: false,
        },
        &mut metrics,
    )?;
    Ok((batch, metrics))
}

pub(crate) fn source_replay_state(
    path: &Path,
    provider_source_key: &str,
    expected_identity: &ProviderLogNativeIdentity,
    cursor: u64,
) -> io::Result<ProviderLogCaptureState> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() < cursor || native_file_identity(&file)? != *expected_identity {
        return Err(io::Error::other("provider log replay identity changed"));
    }
    let canonical_path = std::fs::canonicalize(path)?.to_string_lossy().to_string();
    Ok(ProviderLogCaptureState {
        provider_source_key: provider_source_key.to_owned(),
        path: canonical_path,
        native_identity: expected_identity.clone(),
        committed_offset: cursor,
        continuity_anchor: ProviderLogContinuityAnchor {
            start: cursor,
            len: 0,
            sha256: format!("{:x}", Sha256::digest(b"")),
        },
        normalizer: TranscriptNormalizationState::default(),
        status: "pending".into(),
        reason: None,
        unknown_before_offset: None,
        policy_generation: 0,
        disabled_spans: Vec::new(),
        open_disabled_from: None,
    })
}

// Shared capture engine: keep ownership, range, admission mode, and accounting inputs explicit.
