//! A forced retraction stays legacy for its admission. A different authorized
//! admission may qualify through the ordinary reader's proof boundary.
use super::*;

const TAG: &str = ":retired-view-v1:";

/// Owned arguments for the ordinary reader after a forced view retraction.
#[derive(Clone)]
pub(crate) struct Read {
    pub(crate) cursor: Option<String>,
    pub(crate) revision: Option<String>,
    pub(crate) detail_ref: Option<String>,
    pub(crate) revalidate_ids: Option<Vec<String>>,
    pub(crate) limit: Option<usize>,
}

/// This marker chooses a representation only. It grants no read authority and
/// names only the retired admission, never the cache's global revision owner.
pub(super) fn split(revision: &str) -> Option<(Option<String>, String)> {
    if let Some(admission) = revision
        .strip_prefix("recent:")
        .and_then(|tail| tail.strip_suffix(":retired"))
    {
        uuid::Uuid::parse_str(admission).ok()?;
        return Some((None, admission.to_owned()));
    }
    let (head, rest) = revision.split_once('.')?;
    let (seed, source) = rest.rsplit_once('~')?;
    let (seed, admission) = seed.rsplit_once(TAG)?;
    uuid::Uuid::parse_str(admission).ok()?;
    Some((
        Some(format!("{head}.{seed}~{source}")),
        admission.to_owned(),
    ))
}

fn stamp(revision: &str, admission: &str) -> String {
    let Some((head, rest)) = revision.split_once('.') else {
        return format!("recent:{admission}:retired");
    };
    let Some((seed, source)) = rest.rsplit_once('~') else {
        return format!("recent:{admission}:retired");
    };
    format!("{head}.{seed}{TAG}{admission}~{source}")
}

fn blocked(cache: &RecentSlots, agent: &str) -> bool {
    cache.global_boundaries > 0 || cache.boundaries.contains_key(agent)
}

/// Select only the caller's existing published runtime/Watch slot. The marker
/// is compared with that slot; it is never a key for looking up another A.
/// This is a candidate only: the ordinary reader rechecks scope, source,
/// privacy, proof, aliases and publication fences before returning a binding.
pub(super) fn replacement(
    cache: &Arc<Mutex<RecentSlots>>,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    runtime: &Arc<Mutex<String>>,
    retired_admission: &str,
) -> io::Result<Option<String>> {
    let current = cache
        .lock()
        .map_err(|_| io::Error::other("recent owner poisoned"))?;
    if blocked(&current, &snapshot.session_id) {
        return Ok(None);
    }
    Ok(current
        .slots
        .iter()
        .find(|slot| {
            slot.agent_id == snapshot.session_id
                && slot.published
                && slot.runtime.ptr_eq(&Arc::downgrade(runtime))
                && slot.watch.ptr_eq(&Arc::downgrade(&snapshot.watch_state))
        })
        .filter(|slot| slot.admission != retired_admission)
        .map(|slot| slot.admission.clone()))
}

fn empty(
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    retired_admission: &str,
) -> AgentChatPage {
    AgentChatPage {
        session_id: snapshot.session_id.clone(),
        conversation_id: None,
        generation: None,
        source_epoch: None,
        recent_admission: None,
        archive_read_ready: false,
        revision: format!("recent:{retired_admission}:retired"),
        presentation: None,
        events: Vec::new(),
        next_before: None,
        unchanged: false,
        reset: true,
        older_page: false,
        progress: "source_changed".into(),
        aliases: Vec::new(),
        removed_ids: Vec::new(),
        detail: None,
        revalidated_ids: None,
        bytes_read: 0,
        records_decoded: 0,
    }
}

pub(super) fn read(
    cache: &Arc<Mutex<RecentSlots>>,
    snapshot: &crate::commands::chat::AgentArchiveCaptureSnapshot,
    request: Read,
    retired_admission: &str,
) -> io::Result<AgentChatPage> {
    {
        let current = cache
            .lock()
            .map_err(|_| io::Error::other("recent owner poisoned"))?;
        if blocked(&current, &snapshot.session_id) {
            return Ok(empty(snapshot, retired_admission));
        }
    }
    let before = recent_scope(snapshot);
    let context = crate::commands::chat::conversation_archive_context_from_snapshot(snapshot);
    let mut page = read_with_revalidation_and_limit(
        &context,
        snapshot,
        request.cursor.as_deref(),
        request.revision.as_deref(),
        request.detail_ref.as_deref(),
        request.revalidate_ids.as_deref(),
        request.limit,
    )?;
    let after = recent_scope(snapshot);
    let same = before
        .as_ref()
        .ok()
        .zip(after.as_ref().ok())
        .is_some_and(|((left, _), (right, _))| left == right);
    let archive = page.archive_read_ready && archive_page_is_current(snapshot, &page);
    let current = cache
        .lock()
        .map_err(|_| io::Error::other("recent owner poisoned"))?;
    if blocked(&current, &snapshot.session_id) || (!same && !archive) {
        return Ok(empty(snapshot, retired_admission));
    }
    // Use the existing ordinary source reader and capabilities. The retired
    // admission cannot consume a binding for itself again.
    page.presentation = None;
    page.revision = stamp(&page.revision, retired_admission);
    if request.cursor.is_none() && request.detail_ref.is_none() && request.revalidate_ids.is_none()
    {
        page.reset = true;
    }
    page.bytes_read += before.as_ref().map_or(0, |(_, bytes)| *bytes)
        + after.as_ref().map_or(0, |(_, bytes)| *bytes);
    finish(page)
}
