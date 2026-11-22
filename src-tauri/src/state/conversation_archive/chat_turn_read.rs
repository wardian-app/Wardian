//! Bounded reads of producer-owned turn metadata and step output.
use super::*;

/// Attach metadata only to this fully qualified canonical view. Preview cohorts
/// retain their existing path, and absent prepared metadata adds no seek fanout.
pub(super) fn attach(
    store: &mut Store,
    published_head: Option<&Head>,
    head_ref: Option<&str>,
    recent: bool,
    page: &mut AgentChatPage,
) -> io::Result<()> {
    if !matches!(
        page.progress.as_str(),
        "ready" | "provisional" | "source_unavailable"
    ) || page.events.is_empty()
        || page.recent_admission.is_some()
        || page.events.iter().any(|event| {
            event.metadata["_wardian_canonical_row"] != true
                || event.metadata["chat_provisional"] == true
                || event.metadata["chat_identity_resolution"] == "unresolved"
        })
    {
        return Ok(());
    }
    let Some(head) = published_head.filter(|head| {
        head.progress == "ready"
            && !head.turns.is_empty()
            && page.session_id == head.agent_id
            && page.conversation_id.as_deref() == Some(head.conversation_id.as_str())
            && page.generation.as_deref() == Some(head.generation.as_str())
            && page.source_epoch == head.source_epoch
    }) else {
        return Ok(());
    };
    let mut prepared = false;
    let mut prepared_value = false;
    let packed = if recent {
        head.recent_page.as_deref().and_then(|reference| {
            prepared = true;
            match store.read_allowance(2 * PAGE_ROWS + 2, packed_recent::PACKED_BYTES, |store| {
                packed_recent::read_presentation(store, reference, head)
            }) {
                Ok(value) => {
                    prepared_value = value.is_some();
                    value
                }
                Err(_) => {
                    eprintln!("Chat turn metadata unavailable; using legacy events");
                    None
                }
            }
        })
    } else {
        None
    };
    let selected = packed.and_then(|value| window(value, &page.events, head_ref));
    let selected = selected
        .or_else(|| {
            if prepared && !prepared_value {
                return None;
            }
            store
                .read_allowance(
                    super::super::chat_read_store::READ_OBJECTS,
                    super::super::chat_read_store::READ_BYTES,
                    |store| head.turns.page(store, &page.events, true),
                )
                .ok()
                .flatten()
        })
        .and_then(|value| window(value, &page.events, head_ref));
    if let Some(value) = selected {
        page.presentation = Some(wardian_core::models::chat_presentation::ChatPresentation {
            version: wardian_core::models::chat_presentation::CHAT_PRESENTATION_VERSION,
            payload: serde_json::to_value(value).map_err(io::Error::other)?,
        });
    }
    Ok(())
}

/// A complete native replay can account for frames skipped by the small seed.
/// Only authenticated Rows participate in the turn index. A visible status
/// after the published closure stays an independently owned provisional row.
pub(super) fn attach_captured_claude_window(
    store: &mut Store,
    head: &Head,
    native: &super::super::chat_source_index::SourcePage,
    page: &mut AgentChatPage,
) -> io::Result<()> {
    use wardian_core::models::chat::{AgentChatEventKind, AgentChatRole};
    use wardian_core::models::chat_presentation::*;

    if head.progress != "ready"
        || head.source_epoch.as_deref() != Some(native.epoch.as_str())
        || page.source_epoch != head.source_epoch
        || page.conversation_id.as_deref() != Some(head.conversation_id.as_str())
        || native.events.is_empty()
    {
        return Ok(());
    }

    let coordinates = |event: &AgentChatEvent| {
        event.metadata["chat_source_start"]
            .as_u64()
            .zip(event.metadata["chat_source_end"].as_u64())
            .filter(|(start, end)| start < end && *end <= native.target_extent)
    };
    let mut canonical = Vec::new();
    let mut tail = Vec::new();
    let mut present = BTreeMap::new();
    for event in &page.events {
        let Some(reference) = event.metadata["chat_source_ref"].as_str() else {
            return Ok(());
        };
        let Some(position) = coordinates(event) else {
            return Ok(());
        };
        if event.session_id != head.agent_id
            || event.provider != "claude"
            || event.metadata["chat_source_epoch"].as_str() != Some(native.epoch.as_str())
            || present.insert(reference, (position, &event.kind)).is_some()
        {
            return Ok(());
        }
        if event.metadata["_wardian_canonical_row"] == true
            && event.metadata["chat_provisional"] != true
            && event.metadata["chat_identity_resolution"] != "unresolved"
        {
            let mut row = event.clone();
            if event.kind == AgentChatEventKind::Message
                && event.role == Some(AgentChatRole::Assistant)
                && event.metadata["provider_log"] == true
                && event.id == reference
                && event.metadata["chat_display_physical_id"].as_str() == Some(reference)
            {
                // The exact native Row owns this item. Its collapsed Watch
                // mirror is a display alias and has no native membership.
                // Keep the alias on the returned page for existing readers.
                if let Some(metadata) = row.metadata.as_object_mut() {
                    metadata.remove("chat_display_member_ids");
                }
            }
            canonical.push(row);
        } else if event.kind == AgentChatEventKind::Status
            && event.id == reference
            && event.metadata["raw_type"] == "turn_duration"
            && event.metadata["chat_provisional"] == true
            && event.metadata["chat_source_admission"].as_str() == Some(native.admission.as_str())
        {
            tail.push(event);
        } else {
            return Ok(());
        }
    }
    // Require the same physical rows in both windows. Replay alone cannot
    // explain an uncaptured observation or authorize an old archive row.
    if native.events.iter().any(|event| {
        event.metadata["chat_source_ref"]
            .as_str()
            .and_then(|reference| present.remove(reference))
            .is_none_or(|(position, kind)| {
                coordinates(event) != Some(position) || kind != &event.kind
            })
    }) || !present.is_empty()
    {
        return Ok(());
    }
    let Some(latest) = canonical.iter().max_by_key(|event| coordinates(event)) else {
        return Ok(());
    };
    let Some(root) = latest.metadata["request_root_id"].as_str() else {
        return Ok(());
    };
    let scope = super::super::chat_turn_index::Scope {
        agent_id: &head.agent_id,
        conversation_id: &head.conversation_id,
        source_key: head.source_key.as_deref(),
        source_epoch: head.source_epoch.as_deref(),
        can_activate: false,
    };
    let qualified = store.read_allowance(
        super::super::chat_read_store::READ_OBJECTS,
        super::super::chat_read_store::READ_BYTES,
        |store| {
            let closed = head.turns.closed_roots_reusing_nodes(
                store,
                &scope,
                &BTreeSet::from([root.to_owned()]),
                &mut SeekCache::default(),
            )?;
            let Some(order) = closed.get(root) else {
                return Ok(None);
            };
            if tail.iter().any(|event| {
                coordinates(event)
                    .zip(coordinates(latest))
                    .is_none_or(|(tail, latest)| tail.0 < latest.1)
                    || !super::super::chat_turn_index::Index::after_closed_source_order(
                        event, order,
                    )
            }) {
                return Ok(None);
            }
            head.turns.page(store, &canonical, true)
        },
    );
    let Some(mut value) = qualified.ok().flatten() else {
        return Ok(());
    };
    if let Some(first) = tail.first() {
        value.legacy_turns.push(ChatLegacyTurn {
            turn_key: format!("legacy:{}", digest(first.id.as_bytes())),
            order_key: presentation_order(first).unwrap_or_else(|| first.id.clone()),
            event_refs: tail.iter().map(|event| event.id.clone()).collect(),
            kind: ChatLegacyTurnKind::EarlierActivity,
            membership_complete: false,
        });
    }
    if value.validate().is_err() {
        return Ok(());
    }
    // The exception supplies presentation only. It cannot extend native
    // coverage or issue detail and Older cursors across a provisional tail.
    for item in &mut value.items {
        if let Some(step) = &mut item.step {
            if step.detail_ref.take().is_some() {
                step.detail_availability = ChatDetailAvailability::Pending;
            }
        }
    }
    page.next_before = None;
    page.progress = "provisional".into();
    page.presentation = Some(ChatPresentation {
        version: CHAT_PRESENTATION_VERSION,
        payload: serde_json::to_value(value).map_err(io::Error::other)?,
    });
    Ok(())
}

/// Resolve a reference through the committed step owner and retain only its
/// final output chunks. No source file or archive JSONL is opened here.
pub(super) fn detail_tail(
    store: &mut Store,
    head: &Head,
    reference: &str,
    revision: &str,
) -> io::Result<wardian_core::models::chat_presentation::ChatStepDetail> {
    use wardian_core::models::chat::AgentChatEventKind;
    use wardian_core::models::chat_presentation::{
        ChatOutputFormat, ChatStepDetail, ChatStepStatus, ChatToolCategory, CHAT_STEP_OUTPUT_BYTES,
    };
    let requested: Row = store.read(reference)?;
    if requested.agent_id != head.agent_id
        || requested.conversation_id != head.conversation_id
        || store.get(&head.root, &requested.key)?.as_deref() != Some(reference)
    {
        return Err(io::Error::other("stale step detail owner"));
    }
    let step_key = head
        .turns
        .step_key(store, &requested.event.id)?
        .ok_or_else(|| io::Error::other("step metadata pending"))?;
    let output_id = head
        .turns
        .step_output_event(store, &requested.event.id)?
        .ok_or_else(|| io::Error::other("step output pending"))?;
    let output_ref = store
        .get(&head.identities, &digest(output_id.as_bytes()))?
        .ok_or_else(|| io::Error::other("step backing pending"))?;
    let output: Row = store.read(&output_ref)?;
    if output.agent_id != head.agent_id
        || output.conversation_id != head.conversation_id
        || store.get(&head.root, &output.key)?.as_deref() != Some(output_ref.as_str())
        || output.event.metadata["chat_turn_item_key"].as_str() != Some(step_key.as_str())
    {
        return Err(io::Error::other("step backing changed"));
    }
    let mut input_text = None;
    let mut input_truncated = false;
    if let Some(input_id) = head.turns.step_input_event(store, &requested.event.id)? {
        let input_ref = store
            .get(&head.identities, &digest(input_id.as_bytes()))?
            .ok_or_else(|| io::Error::other("step invocation pending"))?;
        let input: Row = store.read(&input_ref)?;
        if input.agent_id != head.agent_id
            || input.conversation_id != head.conversation_id
            || store.get(&head.root, &input.key)?.as_deref() != Some(input_ref.as_str())
            || input.event.metadata["chat_turn_item_key"].as_str() != Some(step_key.as_str())
        {
            return Err(io::Error::other("step invocation changed"));
        }
        if let Some(text) = tool_input_body(&input.event) {
            input_truncated = text.len() > PREVIEW_BYTES
                || input.event.metadata["chat_tool_input_truncated"] == true;
            input_text = Some(clip(&text, PREVIEW_BYTES));
        }
    }
    if input_text.is_none() && output.event.kind == AgentChatEventKind::Approval {
        if let Some(text) = tool_input_body(&output.event)
            .or_else(|| output.event.command.clone())
            .or_else(|| output.event.text.clone())
        {
            input_truncated = text.len() > PREVIEW_BYTES
                || output.event.metadata["chat_step_inline_complete"] != true;
            input_text = Some(clip(&text, PREVIEW_BYTES));
        }
    }
    let unavailable = output.event.metadata["chat_body_unavailable"] == true;
    let has_output = output.event.kind == AgentChatEventKind::ToolResult;
    let waiting_for_output = matches!(
        head.turns.step_status(store, &requested.event.id)?,
        Some(ChatStepStatus::Running | ChatStepStatus::ActionRequired)
    );
    let (mut text, source_complete, total) = if !has_output {
        (String::new(), true, 0)
    } else if let Some(body) = &output.body {
        let output_start = output.event.metadata["chat_step_output_start"]
            .as_u64()
            .unwrap_or(0);
        let mut chunks = store.page(&body.root, None, 2)?;
        chunks.reverse();
        let mut text = String::new();
        let mut end = None;
        for (key, reference) in chunks {
            let offset = key.parse::<u64>().map_err(io::Error::other)?;
            if end.is_some_and(|end| end != offset) {
                return Err(io::Error::other("step tail gap"));
            }
            let chunk = String::from_utf8(store.read_bytes(&reference, BODY_BYTES)?)
                .map_err(io::Error::other)?;
            end = Some(offset + chunk.len() as u64);
            let skip = output_start.saturating_sub(offset).min(chunk.len() as u64) as usize;
            if !chunk.is_char_boundary(skip) {
                return Err(io::Error::other("step output boundary changed"));
            }
            text.push_str(&chunk[skip..]);
        }
        if body.bytes > 0 && end != Some(body.bytes) {
            return Err(io::Error::other("step tail checkpoint pending"));
        }
        (text, body.complete, body.bytes.saturating_sub(output_start))
    } else if output.event.metadata["chat_step_inline_complete"] == true {
        let text = output.event.text.clone().unwrap_or_default();
        let total = text.len() as u64;
        (text, true, total)
    } else {
        (String::new(), false, 0)
    };
    let mut start = text.len().saturating_sub(CHAT_STEP_OUTPUT_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    if start > 0 {
        text.drain(..start);
    }
    let truncated_before = total > text.len() as u64;
    let pending = (!source_complete || waiting_for_output) && !unavailable;
    let complete = has_output && !pending && !unavailable && !truncated_before;
    let json = complete
        .then(|| serde_json::from_str::<serde_json::Value>(&text).ok())
        .flatten();
    let category = if json.is_some() {
        store
            .read_allowance(64, 64 * 1024, |store| {
                head.turns
                    .page(store, std::slice::from_ref(&output.event), true)
            })
            .ok()
            .flatten()
            .and_then(|page| {
                page.items
                    .into_iter()
                    .find_map(|item| item.step.map(|step| step.category))
            })
    } else {
        None
    };
    let format = if json.is_some() {
        ChatOutputFormat::Json
    } else {
        ChatOutputFormat::Text
    };
    let result_summary = json.as_ref().map(|value| {
        if category == Some(ChatToolCategory::Browser) {
            value["tabs"]
                .as_array()
                .map(|tabs| format!("{} tabs", tabs.len()))
                .unwrap_or_else(|| "Result available".into())
        } else {
            "Result available".into()
        }
    });
    Ok(ChatStepDetail {
        step_key,
        revision: revision.into(),
        text,
        input_text,
        input_truncated,
        result_summary,
        format,
        pending,
        unavailable,
        complete,
        truncated_before,
        total_bytes: (has_output && source_complete).then_some(total),
    })
}

/// Select a bounded delta from producer metadata while retaining whole-turn
/// totals. Every event must belong to this same committed representation.
pub(super) fn window(
    mut value: wardian_core::models::chat_presentation::ChatPresentationV1,
    events: &[AgentChatEvent],
    head_ref: Option<&str>,
) -> Option<wardian_core::models::chat_presentation::ChatPresentationV1> {
    let ids = events
        .iter()
        .map(|event| event.id.as_str())
        .collect::<BTreeSet<_>>();
    value.items.retain_mut(|item| {
        item.event_refs.retain(|id| ids.contains(id.as_str()));
        if let Some(step) = &mut item.step {
            if let Some(reference) = &step.detail_ref {
                if reference.split(':').count() == 2 {
                    step.detail_ref = head_ref.map(|head| format!("{head}:{reference}"));
                }
            }
        }
        !item.event_refs.is_empty()
    });
    value
        .state_event_refs
        .retain(|reference| ids.contains(reference.event_id.as_str()));
    value.legacy_turns.retain_mut(|turn| {
        turn.event_refs.retain(|id| ids.contains(id.as_str()));
        !turn.event_refs.is_empty()
    });
    let covered = value
        .items
        .iter()
        .flat_map(|item| item.event_refs.iter().map(String::as_str))
        .chain(
            value
                .state_event_refs
                .iter()
                .map(|reference| reference.event_id.as_str()),
        )
        .chain(
            value
                .legacy_turns
                .iter()
                .flat_map(|turn| turn.event_refs.iter().map(String::as_str)),
        )
        .collect::<BTreeSet<_>>();
    if covered != ids {
        return None;
    }
    let item_keys = value
        .items
        .iter()
        .map(|item| item.item_key.clone())
        .collect::<BTreeSet<_>>();
    value.turns.retain_mut(|turn| {
        let prompt_count = turn.prompt_item_keys.len();
        let answer_count = turn.answer_item_keys.len();
        turn.prompt_item_keys.retain(|key| item_keys.contains(key));
        turn.answer_item_keys.retain(|key| item_keys.contains(key));
        turn.references_complete &= prompt_count == turn.prompt_item_keys.len()
            && answer_count == turn.answer_item_keys.len();
        value
            .items
            .iter()
            .any(|item| item.turn_key == turn.turn_key)
            || value
                .state_event_refs
                .iter()
                .any(|reference| reference.turn_key == turn.turn_key)
    });
    value.validate().ok()?;
    Some(value)
}
