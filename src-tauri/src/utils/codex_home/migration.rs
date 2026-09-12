//! Immutable intent + verified readiness receipt; directory topology records progress.
//! Interrupted partial copies remain inspectable and never overwrite their source.
use super::{canonical, no_overlay, storage, tree, Intent, Ready, READY};
use std::path::Path;

pub(super) fn ready(intent: &Intent) -> Result<Option<Ready>, String> {
    let receipt = storage::read_record::<Ready>(&intent.slot().join(READY))?;
    if receipt
        .as_ref()
        .is_some_and(|receipt| receipt.version != 1 || receipt.token != intent.token)
    {
        return Err("Foreign compact-home readiness receipt".into());
    }
    Ok(receipt)
}

pub(super) fn validate_target(intent: &Intent, ready: &Ready, path: &Path) -> Result<(), String> {
    if storage::directory_identity(path)? != ready.target_identity
        || (!ready.copied && ready.target_identity != intent.source_identity)
    {
        return Err("Compact target identity changed; state retained without replay".into());
    }
    Ok(())
}

fn source_matches(intent: &Intent, path: &Path) -> Result<(), String> {
    if storage::directory_identity(path)? != intent.source_identity {
        return Err("Original Codex home identity changed; migration stopped".into());
    }
    unchanged(intent, path)
}

fn unchanged(intent: &Intent, path: &Path) -> Result<(), String> {
    no_overlay(path)?;
    if tree::snapshot(path)? != intent.snapshot {
        return Err(
            "Codex migration state changed or copy is incomplete; retain all paths for inspection"
                .into(),
        );
    }
    Ok(())
}

pub(super) fn resume(intent: &Intent) -> Result<(), String> {
    if storage::is_link(&intent.source)? {
        let receipt = ready(intent)?.ok_or("Mapped home has no verified readiness receipt")?;
        validate_target(intent, &receipt, &intent.target)?;
        if canonical(&intent.source)? != intent.target {
            return Err("Foreign Codex habitat link; migration stopped".into());
        }
        return Ok(()); // Complete mapping may contain newer provider state.
    }
    let receipt = match ready(intent)? {
        Some(receipt) => receipt,
        None => prepare_transfer(intent)?,
    };
    if receipt.copied && storage::exists(&intent.staging())? {
        if storage::exists(&intent.target)? || storage::exists(&intent.backup())? {
            return Err("Ambiguous compact-copy publication; all state retained".into());
        }
        validate_target(intent, &receipt, &intent.staging())?;
        unchanged(intent, &intent.staging())?;
        source_matches(intent, &intent.source)?;
        std::fs::rename(intent.staging(), &intent.target).map_err(storage::error)?;
    }
    validate_target(intent, &receipt, &intent.target)?;
    unchanged(intent, &intent.target)?;
    if receipt.copied {
        if storage::exists(&intent.source)? {
            if storage::exists(&intent.backup())? {
                return Err(
                    "Both original and migration backup exist; refusing ambiguous replay".into(),
                );
            }
            source_matches(intent, &intent.source)?;
            std::fs::rename(&intent.source, intent.backup()).map_err(storage::error)?;
        }
        source_matches(intent, &intent.backup())?;
    } else if storage::exists(&intent.source)? || storage::exists(&intent.backup())? {
        return Err("Unexpected original/backup after compact rename; retained".into());
    }
    // This is the final publication. The verified target and immutable receipt
    // already exist, so a crash immediately after link creation is complete.
    crate::utils::fs::create_directory_link(&intent.target, &intent.source)?;
    if canonical(&intent.source)? != intent.target {
        return Err("Compact Codex home link did not select the recorded target".into());
    }
    Ok(())
}

fn prepare_transfer(intent: &Intent) -> Result<Ready, String> {
    if storage::exists(&intent.staging())? || storage::exists(&intent.backup())? {
        return Err("Interrupted unverified compact copy retained beside its original; inspect before retry".into());
    }
    if storage::exists(&intent.target)? {
        // Exact crash window: same-volume rename completed before receipt write.
        if storage::exists(&intent.source)? {
            return Err(
                "Unverified target and source both exist; refusing migration replay".into(),
            );
        }
        source_matches(intent, &intent.target)?;
        return publish_ready(intent, &intent.target, false);
    }
    source_matches(intent, &intent.source)?;
    match std::fs::rename(&intent.source, &intent.target) {
        Ok(()) => {
            source_matches(intent, &intent.target)?;
            publish_ready(intent, &intent.target, false)
        }
        Err(error) if cross_volume(&error) => copy_ready(intent),
        Err(error) => Err(storage::error(error)),
    }
}

pub(super) fn copy_ready(intent: &Intent) -> Result<Ready, String> {
    source_matches(intent, &intent.source)?;
    tree::copy(&intent.source, &intent.staging())?;
    // Verify both trees AFTER copy; no source is moved aside until this complete
    // snapshot and its target identity have been durably recorded.
    unchanged(intent, &intent.staging())?;
    source_matches(intent, &intent.source)?;
    publish_ready(intent, &intent.staging(), true)
}

fn publish_ready(intent: &Intent, path: &Path, copied: bool) -> Result<Ready, String> {
    let receipt = Ready {
        version: 1,
        token: intent.token.clone(),
        target_identity: storage::directory_identity(path)?,
        copied,
    };
    storage::publish_new(&intent.slot().join(READY), &receipt)?;
    Ok(receipt)
}

fn cross_volume(error: &std::io::Error) -> bool {
    #[cfg(windows)]
    let code = 17; // ERROR_NOT_SAME_DEVICE
    #[cfg(unix)]
    let code = libc::EXDEV;
    error.raw_os_error() == Some(code)
}
