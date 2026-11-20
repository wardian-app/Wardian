//! Capture-owned correspondence between one Mock stdout observation and its
//! verified physical log record. These links never create source coordinates.
use super::*;
use crate::commands::provider_log_acquisition::ProviderLogCaptureState;
use serde_json::{json, Value};

const WATCH: &str = "_wardian_mock_watch";
const NATIVE: &str = "_wardian_mock_native";
const RETIRED: &str = "_wardian_mock_retired_sequences";

#[cfg(test)]
#[path = "mock_mirror_capture_tests.rs"]
mod capture_tests;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Identity {
    agent: String,
    source_key: String,
    path: String,
    session: String,
    incarnation: String,
    observation: String,
    payload: String,
    kind: AgentChatEventKind,
    role: Option<AgentChatRole>,
    source: Option<String>,
    turn: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Native {
    identity: Identity,
    coordinate: String,
    epoch: String,
    start: u64,
    end: u64,
    policy: u64,
}

fn string<'a>(event: &'a AgentChatEvent, key: &str) -> Option<&'a str> {
    event.metadata[key]
        .as_str()
        .filter(|value| !value.is_empty())
}

fn identity(context: &ConversationArchiveContext, event: &AgentChatEvent) -> Option<Identity> {
    let session = string(event, "mock_provider_session_id")?;
    let incarnation = string(event, "mock_provider_incarnation")?;
    let observation = string(event, "mock_observation_id")?;
    let payload = string(event, "mock_observation_digest")?;
    if context.provider != "mock"
        || event.provider != "mock"
        || event.session_id != context.agent_id
        || event.kind != AgentChatEventKind::Message
        || event.role != Some(AgentChatRole::Assistant)
        || !matches!(event.source.as_deref(), Some("model" | "message" | "info"))
        || event.metadata["mock_mirror_logged"] != true
        || !context
            .provider_session_ids
            .iter()
            .any(|known| known == session)
        || uuid::Uuid::parse_str(incarnation).is_err()
        || observation.len() > 128
        || !observation.starts_with(&format!("{incarnation}:"))
        || !chat_read_store::valid_ref(payload)
    {
        return None;
    }
    Some(Identity {
        agent: context.agent_id.clone(),
        source_key: context.provider_source_key.clone()?,
        path: string(event, "log_path")?.to_owned(),
        session: session.to_owned(),
        incarnation: incarnation.to_owned(),
        observation: observation.to_owned(),
        payload: payload.to_owned(),
        kind: event.kind.clone(),
        role: event.role.clone(),
        source: event.source.clone(),
        turn: event.turn_id.clone(),
    })
}

fn admitted(event: &AgentChatEvent, source: &ProviderLogCaptureState) -> bool {
    let epoch =
        chat_read_store::digest(&serde_json::to_vec(&source.native_identity).unwrap_or_default());
    let Some((start, end)) = event.metadata["chat_source_start"]
        .as_u64()
        .zip(event.metadata["chat_source_end"].as_u64())
    else {
        return false;
    };
    let prefix = format!("source:{}:{epoch}:{start}:", event.session_id);
    let coordinate = string(event, "chat_source_ref").and_then(|value| value.strip_prefix(&prefix));
    let valid_coordinate = coordinate
        .and_then(|value| value.split_once(':'))
        .is_some_and(|(digest, ordinal)| chat_read_store::valid_ref(digest) && ordinal == "0");
    valid_coordinate
        && event.metadata["chat_source_epoch"].as_str() == Some(epoch.as_str())
        && start < end
        && end <= source.committed_offset
        && start >= source.unknown_before_offset.unwrap_or(0)
        && source.open_disabled_from.is_none_or(|cutoff| end <= cutoff)
        && !source
            .disabled_spans
            .iter()
            .any(|span| start < span.end && end > span.start)
}

/// Strip caller-supplied authority and attach only this pass's typed proofs.
pub(super) fn bind(
    context: &ConversationArchiveContext,
    event: &mut AgentChatEvent,
    capture: Option<&CapturePreparation<'_>>,
    watch: Option<&crate::commands::chat::MockWatchAuthority>,
) -> io::Result<()> {
    if let Some(metadata) = event.metadata.as_object_mut() {
        for key in [WATCH, NATIVE, RETIRED, "mock_watch_alias"] {
            metadata.remove(key);
        }
    }
    let Some(mut identity) = identity(context, event) else {
        return Ok(());
    };
    if let Some(capture) = capture {
        if let Some(proof) = capture.source_proof {
            if proof.matches_capture(capture.previous, capture.next)?
                && proof.proves(event)
                && proof.mock_provider_session() == Some(identity.session.as_str())
                && capture.next.provider_source_key == identity.source_key
                && std::fs::canonicalize(&identity.path)
                    .ok()
                    .is_some_and(|path| path.to_string_lossy() == capture.next.path)
                && admitted(event, capture.next)
            {
                identity.path.clone_from(&capture.next.path);
                let binding = Native {
                    identity,
                    coordinate: string(event, "chat_source_ref").unwrap().to_owned(),
                    epoch: string(event, "chat_source_epoch").unwrap().to_owned(),
                    start: event.metadata["chat_source_start"].as_u64().unwrap(),
                    end: event.metadata["chat_source_end"].as_u64().unwrap(),
                    policy: capture.next.policy_generation,
                };
                event.metadata[NATIVE] = serde_json::to_value(binding).map_err(io::Error::other)?;
                return Ok(());
            }
        }
    }
    if event.metadata["provider_log"] != true
        && watch.is_some_and(|proof| proof.proves(context, event))
    {
        event.metadata[WATCH] = serde_json::to_value(identity).map_err(io::Error::other)?;
    }
    Ok(())
}

fn native(event: &AgentChatEvent) -> Option<Native> {
    let binding: Native = serde_json::from_value(event.metadata.get(NATIVE)?.clone()).ok()?;
    (describes(&binding.identity, event)
        && binding.start < binding.end
        && super::chat_source_index::valid_source_id(&binding.identity.agent, &binding.coordinate)
        && event.id == binding.coordinate
        && event.metadata["provider_log"] == true
        && string(event, "chat_source_ref") == Some(binding.coordinate.as_str())
        && string(event, "chat_source_epoch") == Some(binding.epoch.as_str())
        && event.metadata["chat_source_start"].as_u64() == Some(binding.start)
        && event.metadata["chat_source_end"].as_u64() == Some(binding.end))
    .then_some(binding)
}

fn watch(event: &AgentChatEvent) -> Option<Identity> {
    let binding: Identity = serde_json::from_value(event.metadata.get(WATCH)?.clone()).ok()?;
    let prefix = format!("{}:", binding.agent);
    let mapper = event
        .id
        .strip_prefix(&prefix)
        .and_then(|value| value.split_once(':'))
        .is_some_and(|(ordinal, source)| {
            ordinal.len() == 16
                && ordinal.bytes().all(|byte| byte.is_ascii_hexdigit())
                && binding.source.as_deref() == Some(source)
        });
    (describes(&binding, event)
        && mapper
        && event.metadata["provider_log"] != true
        && [
            "chat_source_ref",
            "chat_source_start",
            "chat_source_end",
            "chat_source_epoch",
            "chat_source_admission",
            "chat_display_physical_id",
            "chat_display_member_ids",
            "chat_recent_native_key",
            "chat_recent_native_version",
        ]
        .iter()
        .all(|key| event.metadata.get(*key).is_none_or(Value::is_null)))
    .then_some(binding)
}

fn describes(binding: &Identity, event: &AgentChatEvent) -> bool {
    let observation = binding
        .observation
        .strip_prefix(&format!("{}:", binding.incarnation))
        .is_some_and(|sequence| {
            sequence
                .parse::<u64>()
                .is_ok_and(|value| value > 0 && value.to_string() == sequence)
        });
    event.provider == "mock"
        && binding.kind == AgentChatEventKind::Message
        && binding.role == Some(AgentChatRole::Assistant)
        && matches!(
            binding.source.as_deref(),
            Some("model" | "message" | "info")
        )
        && observation
        && binding.observation.len() <= 128
        && chat_read_store::valid_ref(&binding.payload)
        && uuid::Uuid::parse_str(&binding.incarnation)
            .is_ok_and(|uuid| !uuid.is_nil() && uuid.to_string() == binding.incarnation)
        && event.session_id == binding.agent
        && event.kind == binding.kind
        && event.role == binding.role
        && event.source == binding.source
        && event.turn_id == binding.turn
        && event.metadata["mock_mirror_logged"] == true
        && string(event, "mock_observation_id") == Some(binding.observation.as_str())
        && string(event, "mock_provider_incarnation") == Some(binding.incarnation.as_str())
        && string(event, "mock_provider_session_id") == Some(binding.session.as_str())
        && string(event, "mock_observation_digest") == Some(binding.payload.as_str())
}

fn source_admits(
    binding: &Native,
    event: &AgentChatEvent,
    sources: &[ProviderLogCaptureState],
) -> bool {
    sources.iter().any(|source| {
        source.provider_source_key == binding.identity.source_key
            && source.path == binding.identity.path
            && source.policy_generation == binding.policy
            && admitted(event, source)
    })
}

struct Observation<'a> {
    id: &'a str,
    native: Option<Native>,
    watch: Option<Identity>,
    admitted: bool,
}

impl Observation<'_> {
    fn pairs_with(&self, other: &Self) -> bool {
        if let Some(binding) = &self.native {
            return other.watch.as_ref() == Some(&binding.identity) && self.admitted;
        }
        if let Some(binding) = &other.native {
            return self.watch.as_ref() == Some(&binding.identity) && other.admitted;
        }
        false
    }
}

/// Decode private bindings once for this serialized archive pass. This borrows
/// only the already admitted inputs; no proof or file contents survive the pass.
pub(super) struct MatchingSet<'a> {
    observations: Vec<Observation<'a>>,
    existing: usize,
}

impl<'a> MatchingSet<'a> {
    pub(super) fn new(
        existing: &'a [AgentChatEvent],
        batch: &'a [AgentChatEvent],
        sources: &[ProviderLogCaptureState],
    ) -> io::Result<Self> {
        let observations = existing
            .iter()
            .chain(batch)
            .map(|event| {
                let native = native(event);
                let admitted = native
                    .as_ref()
                    .is_some_and(|binding| source_admits(binding, event, sources));
                Observation {
                    id: &event.id,
                    native,
                    watch: watch(event),
                    admitted,
                }
            })
            .collect::<Vec<_>>();
        let mut owners = HashMap::new();
        for observation in &observations {
            let Some(binding) = &observation.native else {
                continue;
            };
            let key = (
                &binding.identity.agent,
                &binding.identity.source_key,
                &binding.identity.session,
                &binding.identity.incarnation,
                &binding.identity.observation,
            );
            if owners
                .insert(key, &binding.coordinate)
                .is_some_and(|old| old != &binding.coordinate)
            {
                return Err(io::Error::other(
                    "ambiguous Mock mirror receipt at distinct source coordinates",
                ));
            }
        }
        Ok(Self {
            observations,
            existing: existing.len(),
        })
    }

    /// A mirror selects a narrative only through one proved native observation.
    /// The native event keeps its own physical ID and source/body ownership.
    pub(super) fn matching_record(
        &self,
        records: &[ConversationNarrativeRecord],
        batch_index: usize,
    ) -> Option<usize> {
        let event = &self.observations[self.existing + batch_index];
        let matches = self.observations[..self.existing]
            .iter()
            .filter(|old| old.pairs_with(event))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return None;
        }
        let old = matches[0];
        let reverse = self
            .observations
            .iter()
            .filter(|candidate| candidate.id != old.id && old.pairs_with(candidate))
            .map(|candidate| candidate.id)
            .collect::<HashSet<_>>();
        if reverse.len() != 1 {
            return None;
        }
        let owners = records
            .iter()
            .enumerate()
            .filter(|(_, record)| record.event_refs.iter().any(|id| id == old.id))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        (owners.len() == 1).then(|| owners[0])
    }
}

fn pairs(
    events: &[AgentChatEvent],
    sources: &[ProviderLogCaptureState],
) -> io::Result<Vec<(usize, usize)>> {
    let prepared = MatchingSet::new(events, &[], sources)?;
    let observations = &prepared.observations;
    let mut result = Vec::new();
    for (watch_index, watch_event) in observations
        .iter()
        .enumerate()
        .filter(|(_, event)| event.watch.is_some())
    {
        let matching = observations
            .iter()
            .enumerate()
            .filter(|(_, event)| watch_event.pairs_with(event))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            continue;
        }
        let native_index = matching[0];
        if observations
            .iter()
            .filter(|candidate| candidate.pairs_with(&observations[native_index]))
            .count()
            == 1
        {
            result.push((watch_index, native_index));
        }
    }
    Ok(result)
}

/// Hide the proved stdout mirror only after the source cursor/policy commit.
/// The auxiliary Watch lookup authorizes recent handoff, never source renewal.
pub(super) fn project(
    events: &mut Vec<AgentChatEvent>,
    sources: &[ProviderLogCaptureState],
) -> io::Result<()> {
    let pairs = pairs(events, sources)?;
    let mut removed = HashSet::new();
    for (watch_index, native_index) in pairs {
        let watch = &events[watch_index];
        let alias = watch.id.clone();
        let recent =
            ["chat_recent_key", "chat_recent_version"].map(|key| watch.metadata.get(key).cloned());
        let native = &mut events[native_index];
        native.metadata["mock_watch_alias"] = json!(alias);
        for (key, value) in ["chat_recent_key", "chat_recent_version"]
            .into_iter()
            .zip(recent)
        {
            if let Some(value) = value {
                native.metadata[key] = value;
            }
        }
        removed.insert(watch_index);
    }
    let mut index = 0;
    events.retain(|_| {
        let keep = !removed.contains(&index);
        index += 1;
        keep
    });
    Ok(())
}

/// Reconcile audit narratives that were admitted before their native mirror.
pub(super) fn reconcile(
    records: &mut Vec<ConversationNarrativeRecord>,
    events: &mut [AgentChatEvent],
    sources: &[ProviderLogCaptureState],
) -> io::Result<bool> {
    let pairs = pairs(events, sources)?;
    let mut removed = HashSet::new();
    for (watch_index, native_index) in pairs {
        let owners = |id: &str| {
            records
                .iter()
                .enumerate()
                .filter(|(_, record)| record.event_refs.iter().any(|value| value == id))
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        };
        let watch_owners = owners(&events[watch_index].id);
        let native_owners = owners(&events[native_index].id);
        if watch_owners.len() != 1 || native_owners.len() != 1 || watch_owners == native_owners {
            continue;
        }
        let from = watch_owners[0];
        let to = native_owners[0];
        let duplicate = records[from].clone();
        events[watch_index].metadata[RETIRED] = json!([duplicate.seq]);
        let canonical = &mut records[to];
        for (target, values) in [
            (&mut canonical.event_refs, duplicate.event_refs),
            (&mut canonical.source_refs, duplicate.source_refs),
            (&mut canonical.artifact_refs, duplicate.artifact_refs),
        ] {
            for value in values {
                if !target.contains(&value) {
                    target.push(value);
                }
            }
        }
        removed.insert(from);
    }
    let changed = !removed.is_empty();
    let mut index = 0;
    records.retain(|_| {
        let keep = !removed.contains(&index);
        index += 1;
        keep
    });
    Ok(changed)
}

/// Finish an already durable retirement intent before ordinary recovery checks
/// its sequence markers. This repairs a past committed correspondence; current
/// source/privacy admission still independently controls canonical projection.
pub(super) fn recover_pending(
    context: &ConversationArchiveContext,
    directory: &std::path::Path,
    records: &mut Vec<ConversationNarrativeRecord>,
    events: &[AgentChatEvent],
) -> io::Result<bool> {
    let mut merges = Vec::new();
    for event in events {
        let Some(markers) = event.metadata.get(RETIRED) else {
            continue;
        };
        let invalid = || io::Error::other("invalid pending Mock narrative reconciliation");
        let identity = watch(event).ok_or_else(invalid)?;
        if context.provider != "mock"
            || identity.agent != context.agent_id
            || context.provider_source_key.as_deref() != Some(identity.source_key.as_str())
            || !context.provider_session_ids.contains(&identity.session)
        {
            return Err(invalid());
        }
        let markers = markers
            .as_array()
            .filter(|markers| markers.len() == 1)
            .ok_or_else(invalid)?;
        let retired = markers[0]
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(invalid)?;
        let native_owners = events
            .iter()
            .filter(|candidate| {
                native(candidate).is_some_and(|binding| binding.identity == identity)
            })
            .collect::<Vec<_>>();
        if native_owners.len() != 1
            || events
                .iter()
                .filter(|candidate| watch(candidate).as_ref() == Some(&identity))
                .count()
                != 1
        {
            return Err(invalid());
        }
        let owners = |id: &str| {
            records
                .iter()
                .enumerate()
                .filter(|(_, record)| record.event_refs.iter().any(|value| value == id))
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        };
        let from = owners(&event.id);
        let to = owners(&native_owners[0].id);
        if from.len() != 1 || to.len() != 1 {
            return Err(invalid());
        }
        if from == to {
            continue;
        }
        if records[from[0]].seq != retired {
            return Err(invalid());
        }
        merges.push((from[0], to[0]));
    }
    if merges.is_empty() {
        return Ok(false);
    }
    let mut removed = HashSet::new();
    for (from, to) in merges {
        if !removed.insert(from) {
            return Err(io::Error::other("ambiguous pending Mock narrative"));
        }
        let duplicate = records[from].clone();
        let canonical = &mut records[to];
        for (target, values) in [
            (&mut canonical.event_refs, duplicate.event_refs),
            (&mut canonical.source_refs, duplicate.source_refs),
            (&mut canonical.artifact_refs, duplicate.artifact_refs),
        ] {
            for value in values {
                if !target.contains(&value) {
                    target.push(value);
                }
            }
        }
    }
    let mut index = 0;
    records.retain(|_| {
        let keep = !removed.contains(&index);
        index += 1;
        keep
    });
    // Validate the entire result before its only durable mutation. A failed
    // replacement leaves the same provable intent available to the next owner.
    retired_sequences(records, events)?;
    write_jsonl_atomic(&directory.join("conversation.jsonl"), records)?;
    Ok(true)
}

/// Validate retired sequence markers against the actual persisted mirror pair.
pub(super) fn retired_sequences(
    records: &[ConversationNarrativeRecord],
    events: &[AgentChatEvent],
) -> io::Result<HashSet<u64>> {
    let mut retired = HashSet::new();
    for event in events {
        let Some(markers) = event.metadata.get(RETIRED) else {
            continue;
        };
        let proof =
            watch(event).ok_or_else(|| io::Error::other("unproved Mock retired sequence"))?;
        let matches = events
            .iter()
            .filter(|other| native(other).is_some_and(|native| native.identity == proof))
            .collect::<Vec<_>>();
        if matches.len() != 1
            || records
                .iter()
                .filter(|record| {
                    record.event_refs.contains(&event.id)
                        && record.event_refs.contains(&matches[0].id)
                })
                .count()
                != 1
        {
            return Err(io::Error::other("ambiguous Mock retired narrative"));
        }
        for marker in markers
            .as_array()
            .filter(|markers| markers.len() == 1)
            .ok_or_else(|| io::Error::other("invalid Mock retired sequences"))?
        {
            let sequence = marker
                .as_u64()
                .filter(|value| *value > 0)
                .ok_or_else(|| io::Error::other("invalid Mock retired sequence"))?;
            if records.iter().any(|record| record.seq == sequence) || !retired.insert(sequence) {
                return Err(io::Error::other("occupied Mock retired sequence"));
            }
        }
    }
    Ok(retired)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (
        ConversationArchiveContext,
        ProviderLogCaptureState,
        Vec<AgentChatEvent>,
    ) {
        let mut context = ConversationArchiveContext::for_agent_id("mirror-agent", "mock");
        context.provider_session_ids = vec!["owned".into()];
        context.provider_source_key = Some("mock:session:owned".into());
        let source = ProviderLogCaptureState {
            provider_source_key: context.provider_source_key.clone().unwrap(),
            path: "mock.jsonl".into(),
            native_identity: crate::commands::provider_log_acquisition::ProviderLogNativeIdentity {
                platform: "test".into(),
                primary: 1,
                secondary: 2,
            },
            committed_offset: 100,
            continuity_anchor: Default::default(),
            normalizer: Default::default(),
            status: "complete".into(),
            reason: None,
            unknown_before_offset: None,
            policy_generation: 0,
            disabled_spans: Vec::new(),
            open_disabled_from: None,
        };
        let epoch = chat_read_store::digest(&serde_json::to_vec(&source.native_identity).unwrap());
        let coordinate = format!("source:mirror-agent:{epoch}:10:{}:0", "b".repeat(64));
        let identity = Identity {
            agent: context.agent_id.clone(),
            source_key: source.provider_source_key.clone(),
            path: source.path.clone(),
            session: "owned".into(),
            incarnation: "c60e52bf-71f2-43f1-a313-9ea96fe2f7bc".into(),
            observation: "c60e52bf-71f2-43f1-a313-9ea96fe2f7bc:1".into(),
            payload: "a".repeat(64),
            kind: AgentChatEventKind::Message,
            role: Some(AgentChatRole::Assistant),
            source: Some("model".into()),
            turn: Some("same-turn".into()),
        };
        let mut native: AgentChatEvent = serde_json::from_value(json!({
            "id": coordinate, "session_id": context.agent_id, "provider": "mock", "kind": "message",
            "role": "assistant", "source": "model", "turn_id": "same-turn", "text": "equal text",
            "metadata": {"provider_log": true, "log_path": source.path,
                "chat_source_ref": coordinate, "chat_source_epoch": epoch, "chat_source_start": 10, "chat_source_end": 100,
                "mock_observation_id": identity.observation, "mock_provider_incarnation": identity.incarnation,
                "mock_provider_session_id": identity.session, "mock_mirror_logged": true,
                "mock_observation_digest": identity.payload}
        })).unwrap();
        native.metadata[NATIVE] = serde_json::to_value(Native {
            identity: identity.clone(),
            coordinate,
            epoch,
            start: 10,
            end: 100,
            policy: 0,
        })
        .unwrap();
        let mut watch = native.clone();
        watch.id = "mirror-agent:0000000000000007:model".into();
        for key in [
            NATIVE,
            "provider_log",
            "chat_source_ref",
            "chat_source_epoch",
            "chat_source_start",
            "chat_source_end",
        ] {
            watch.metadata.as_object_mut().unwrap().remove(key);
        }
        watch.metadata[WATCH] = serde_json::to_value(identity).unwrap();
        watch.metadata["chat_recent_key"] = json!("admission:watch:7");
        watch.metadata["chat_recent_version"] = json!("c".repeat(64));
        (context, source, vec![native, watch])
    }

    #[test]
    fn committed_source_is_required_before_watch_handoff() {
        let (_, mut source, mut events) = fixture();
        let original = events[0].clone();
        source.committed_offset = 99;
        project(&mut events, &[source.clone()]).unwrap();
        assert_eq!(events.len(), 2);
        source.committed_offset = 100;
        project(&mut events, &[source]).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, original.id);
        assert_eq!(
            events[0].metadata["chat_source_ref"],
            original.metadata["chat_source_ref"]
        );
        assert_eq!(
            events[0].metadata["mock_watch_alias"],
            "mirror-agent:0000000000000007:model"
        );
        assert_eq!(events[0].metadata["chat_recent_key"], "admission:watch:7");
    }

    #[test]
    fn one_receipt_never_selects_two_physical_records() {
        let (_, mut source, mut events) = fixture();
        let mut duplicate = events[0].clone();
        let mut binding = native(&duplicate).unwrap();
        binding.start = 110;
        binding.end = 200;
        binding.coordinate = format!(
            "source:mirror-agent:{}:110:{}:0",
            binding.epoch,
            "d".repeat(64)
        );
        duplicate.id.clone_from(&binding.coordinate);
        duplicate.metadata["chat_source_ref"] = json!(binding.coordinate);
        duplicate.metadata["chat_source_start"] = json!(110);
        duplicate.metadata["chat_source_end"] = json!(200);
        duplicate.metadata[NATIVE] = serde_json::to_value(binding).unwrap();
        events.push(duplicate);
        source.committed_offset = 200;
        assert!(project(&mut events, &[source]).is_err());
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn receipt_payload_and_policy_conflicts_keep_both_observations() {
        for variant in 0..5 {
            let (_, mut source, mut events) = fixture();
            match variant {
                0 => events[1].metadata["mock_observation_digest"] = json!("e".repeat(64)),
                1 => source.policy_generation = 1,
                2 => events[1].session_id = "foreign".into(),
                3 => events[1].metadata["mock_mirror_logged"] = json!(false),
                _ => events[1].metadata["chat_source_epoch"] = json!("partial-framed-claim"),
            }
            project(&mut events, &[source]).unwrap();
            assert_eq!(events.len(), 2, "conflicting variant {variant}");
        }
    }

    #[test]
    fn generic_metadata_cannot_construct_mirror_authority() {
        let (context, source, mut events) = fixture();
        for event in &mut events {
            bind(&context, event, None, None).unwrap();
        }
        project(&mut events, &[source]).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(
            |event| event.metadata.get(WATCH).is_none() && event.metadata.get(NATIVE).is_none()
        ));
    }

    #[test]
    fn different_observations_in_one_turn_keep_distinct_native_owners() {
        let (_, mut source, mut events) = fixture();
        let mut next_native = events[0].clone();
        let mut next_watch = events[1].clone();
        let mut binding = native(&next_native).unwrap();
        binding.identity.observation = format!("{}:2", binding.identity.incarnation);
        binding.identity.payload = "e".repeat(64);
        binding.start = 110;
        binding.end = 200;
        binding.coordinate = format!(
            "source:mirror-agent:{}:110:{}:0",
            binding.epoch,
            "f".repeat(64)
        );
        next_native.id.clone_from(&binding.coordinate);
        next_native.metadata["chat_source_ref"] = json!(binding.coordinate);
        next_native.metadata["chat_source_start"] = json!(110);
        next_native.metadata["chat_source_end"] = json!(200);
        next_watch.id = "mirror-agent:0000000000000008:model".into();
        for event in [&mut next_native, &mut next_watch] {
            event.metadata["mock_observation_id"] = json!(binding.identity.observation);
            event.metadata["mock_observation_digest"] = json!(binding.identity.payload);
        }
        next_watch.metadata[WATCH] = serde_json::to_value(&binding.identity).unwrap();
        next_native.metadata[NATIVE] = serde_json::to_value(binding).unwrap();
        events.extend([next_native, next_watch]);
        source.committed_offset = 200;
        project(&mut events, &[source]).unwrap();
        assert_eq!(events.len(), 2);
        assert_ne!(events[0].id, events[1].id);
        assert!(events
            .iter()
            .all(|event| event.text.as_deref() == Some("equal text")));
    }

    #[test]
    fn retired_narrative_is_backed_by_one_persisted_pair() {
        let (_, source, mut events) = fixture();
        let mut records = vec![
            narrative_from_chat_event(&events[0], 1).unwrap(),
            narrative_from_chat_event(&events[1], 2).unwrap(),
        ];
        records[0].event_refs = vec![events[0].id.clone()];
        records[1].event_refs = vec![events[1].id.clone()];
        assert!(reconcile(&mut records, &mut events, &[source]).unwrap());
        assert_eq!(records.len(), 1);
        assert_eq!(
            retired_sequences(&records, &events).unwrap(),
            HashSet::from([2])
        );
        records[0].event_refs.retain(|id| id != &events[1].id);
        assert!(retired_sequences(&records, &events).is_err());
    }
}
