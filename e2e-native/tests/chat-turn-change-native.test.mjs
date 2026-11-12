// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { By } from "selenium-webdriver";

import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
  invokeTauri,
} from "../lib/harness.mjs";
import { chatBodyWitness, freshSavedHome, stageSavedFixture, verifySavedFixture } from "../lib/chatSavedFixture.mjs";

const { chatAcceptanceFixture } = createRequire(import.meta.url)("../../scripts/mock-agent.cjs");

/**
 * Passive WebView instrumentation. It makes no IPC calls and never dispatches
 * input. Keep this named boundary executable by the app-free harness tests.
 */
function installChatObservation(config) {
  const state = { owner: null, phase: null, phases: [], calls: [], receipts: [], presentations: [], errors: [], intents: [],
    captureCostMs: 0, coverage: "PAGE_LOADER_ONLY", broaderIpcRuntime: "UNOBSERVED", activation: null, rejections: 0, disposed: false };
  let retired = false;
  let subscription = null;
  let disposalReceipt = null;
  // Queued frame callbacks may outlive the subscription; never admit their writes.
  const frame = (callback) => requestAnimationFrame(() => { if (!retired) callback(); });
  const bounded = (value) => typeof value === "string" && value.length <= 1024 ? value : null;
  const containerNodes = () => Array.from(document.querySelectorAll(
    '[data-testid="agent-session-surface"], [data-agent-grid-card-id]'));
  const inspectContainer = (node) => {
    const kind = node.getAttribute("data-testid") === "agent-session-surface" ? "agent-session" : "agents-overview";
    const owner = bounded(node.getAttribute(kind === "agent-session" ? "data-resource-key" : "data-agent-grid-card-id"));
    const semantic = kind === "agent-session" ? node : node.closest('[data-testid="agents-overview-surface"]');
    const panel = semantic?.closest('[data-testid="surface-panel"]');
    const host = panel?.closest('[data-testid="workbench-host"]');
    const surfaceId = bounded(semantic?.getAttribute("data-surface-id"));
    const tabs = Array.from(document.querySelectorAll('[role="tab"][data-surface-id]')).filter((item) =>
      host?.contains(item) && item.getAttribute("data-surface-id") === surfaceId
      && item.getAttribute("data-surface-type") === kind && item.getAttribute("aria-selected") === "true");
    const tab = tabs.length === 1 ? tabs[0] : null;
    const tabId = bounded(tab?.getAttribute("id"));
    const panelId = bounded(tab?.getAttribute("aria-controls"));
    const tabpanel = panelId ? document.getElementById(panelId) : null;
    const ancestor = panel?.closest('[role="tabpanel"]');
    // Dockview renders panel content in its sibling overlay, outside the ARIA
    // tabpanel. Bind that placement to the same Dockview root and real selected
    // tab's bidirectional ARIA link; ordinary nested tabpanels remain supported.
    const portal = ancestor ? null : panel?.closest('.dv-render-overlay');
    const dockRoot = panel?.closest('.dv-shell');
    const placement = ancestor ? ancestor === tabpanel : Boolean(portal && dockRoot
      && dockRoot === tabpanel?.closest('.dv-shell') && dockRoot === tab?.closest('.dv-shell'));
    const hosts = Array.from(document.querySelectorAll('[data-testid="workbench-host"]'))
      .filter((item) => item.isConnected && readable(item));
    const rect = node.getBoundingClientRect();
    const visible = document.visibilityState === "visible" && [node, semantic, panel, tabpanel, tab, host]
      .every((item) => item?.isConnected && !item.hidden && readable(item))
      && rect.width > 0 && rect.height > 0 && rect.right > 0 && rect.bottom > 0
      && rect.left < innerWidth && rect.top < innerHeight;
    const valid = Boolean(bounded(state.owner) && owner === state.owner && surfaceId && visible
      && hosts.length === 1 && host === hosts[0] && host.contains(node) && host.contains(tab) && host.contains(tabpanel)
      && !host.inert && host.getAttribute("data-reset-pending") !== "true"
      && panel.getAttribute("data-surface-id") === surfaceId && panel.getAttribute("data-surface-type") === kind
      && placement && tabId && tabpanel.getAttribute("role") === "tabpanel" && tabpanel.getAttribute("aria-labelledby") === tabId
      && tab.getAttribute("role") === "tab" && tab.getAttribute("aria-selected") === "true"
      && tab.getAttribute("aria-controls") === tabpanel.getAttribute("id")
      && tab.getAttribute("data-surface-id") === surfaceId && tab.getAttribute("data-surface-type") === kind
      && (kind !== "agent-session" || (panel.getAttribute("data-resource-key") === owner
        && tab.getAttribute("data-resource-key") === owner)));
    return { node, panel, tabpanel, tab, host, portal, dockRoot, valid,
      identity: { kind, owner, surfaceId, panelId: bounded(tabpanel?.getAttribute("id")), tabId },
      visible, selected: tab?.getAttribute("aria-selected") === "true" };
  };
  const ownedContainers = () => containerNodes().filter((node) => node.getAttribute("data-resource-key") === state.owner
      || node.getAttribute("data-agent-grid-card-id") === state.owner).map(inspectContainer).filter((item) => item.valid);
  const ownedContainer = () => {
    const matches = ownedContainers();
    return matches.length === 1 ? matches[0] : null;
  };
  const surface = () => ownedContainer()?.node ?? null;
  const now = () => performance.now();
  const members = (value) => Array.isArray(value) && value.length <= 384
    && value.every((id) => bounded(id)) ? [...value] : null;
  const scopeValue = (value) => Boolean(bounded(value)) && value.trim().length > 0;
  const nullableScopeValue = (value) => value === null || scopeValue(value);
  const scopeMode = (scope) => {
    if (!scope || !scopeValue(scope.session) || !scopeValue(scope.revision) || !nullableScopeValue(scope.epoch)) return null;
    if (scope.admission !== null) return typeof scope.admission === "string" && scope.admission.length > 0
      && scope.admission.length <= 256 && nullableScopeValue(scope.conversation) && nullableScopeValue(scope.generation)
      ? "recent-preview" : null;
    return scopeValue(scope.conversation) && scopeValue(scope.generation) ? "durable" : null;
  };
  const scopeKey = (scope) => {
    const mode = scopeMode(scope);
    return mode ? JSON.stringify([mode, scope.session, scope.conversation, scope.generation,
      scope.epoch, scope.revision, scope.admission]) : null;
  };
  // Match the consumer's validRecentPreview capability, not a generic nullable
  // canonical scope. A preview never grants Older, detail or renewal authority.
  const validRecentPreview = (dto) => scopeMode({ session: dto.session_id, conversation: dto.conversation_id,
    generation: dto.generation, epoch: dto.source_epoch, revision: dto.revision, admission: dto.recent_admission }) === "recent-preview"
    && typeof dto.reset === "boolean" && typeof dto.unchanged === "boolean"
    && (dto.progress === "provisional" || dto.progress === "no_recent")
    && dto.next_before === null && dto.detail === null
    && (!Object.prototype.hasOwnProperty.call(dto, "older_page") || dto.older_page === false)
    && dto.revalidated_ids == null && Array.isArray(dto.aliases) && dto.aliases.length === 0
    && Array.isArray(dto.removed_ids) && dto.removed_ids.length === 0
    && dto.events.length <= 80 && new Set(dto.events.map((event) => event.id)).size === dto.events.length
    && dto.events.every((event) => event.session_id === dto.session_id && typeof event.id === "string" && event.id.length > 0
      && (event.text == null || typeof event.text === "string") && new TextEncoder().encode(event.text ?? "").length <= 1024);
  const parse = (value) => { try { return JSON.parse(value); } catch { return null; } };
  const sameBody = (left, right) => left && right && left.node === right.node && left.row === right.row && left.scroll === right.scroll
    && ["node", "panel", "tabpanel", "tab", "host", "portal", "dockRoot"].every((key) => left.container[key] === right.container[key])
    && JSON.stringify({ ...left.evidence, receiptAt: null, matchingBodySlots: null })
      === JSON.stringify({ ...right.evidence, receiptAt: null, matchingBodySlots: null });
  let inputEvidence = null;
  let wheelEvidence = null;
  let paintedBody = null;
  const witness = config.witness ?? { kind: "whole", text: config.body };
  const matchesText = (text) => typeof text === "string" && (witness.kind === "whole"
    ? text === witness.text : witness.kind === "needle" && witness.text.length >= 16
      && witness.text.length <= 128 && text.indexOf(witness.text) >= 0
      && text.indexOf(witness.text, text.indexOf(witness.text) + 1) === -1);
  const readable = (element) => {
    for (let node = element; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (style.visibility === "hidden" || style.visibility === "collapse" || style.display === "none"
        || style.opacity === "0" || node.getAttribute("aria-hidden") === "true") return false;
    }
    return true;
  };
  const textRange = (content) => {
    if (!matchesText(content.textContent)) return null;
    const range = document.createRange();
    if (witness.kind === "whole") { range.selectNodeContents(content); return range; }
    // Anchor the independent needle itself, including when emphasis splits text
    // nodes. A visible header elsewhere in this message cannot stand in for it.
    const start = content.textContent.indexOf(witness.text);
    const end = start + witness.text.length;
    const walker = document.createTreeWalker(content, 4 /* SHOW_TEXT */);
    let offset = 0;
    let count = 0;
    let anchored = false;
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (++count > 8192) return null;
      const next = offset + node.textContent.length;
      if (next > start && offset < end && !readable(node.parentElement)) return null;
      if (!anchored && next > start) { range.setStart(node, start - offset); anchored = true; }
      if (anchored && next >= end) {
        range.setEnd(node, end - offset);
        return range.toString() === witness.text ? range : null;
      }
      offset = next;
    }
    return null;
  };
  const fail = (error) => { if (!retired && state.errors.length < 8) state.errors.push(String(error).slice(0, 160)); };
  const record = (command, args, dto, returnedAt) => {
    if (retired || command !== "load_agent_chat_page" || !dto || !Array.isArray(dto.events)) return;
    if (state.receipts.length >= 128 || dto.events.length > 384) throw new Error("passive receipt bound exceeded");
    if (dto.recent_admission != null && (typeof dto.recent_admission !== "string"
      || !dto.recent_admission || dto.recent_admission.length > 256)) throw new Error("invalid recent admission");
    if (dto.recent_admission != null && !validRecentPreview(dto)) throw new Error("invalid recent preview capability");
    const aliases = Array.isArray(dto.aliases) && dto.aliases.length <= 384 ? dto.aliases.map((alias) => ({
      observation: bounded(alias.observation_id), canonical: bounded(alias.canonical_id),
    })) : [];
    state.receipts.push({
      at: returnedAt, session: bounded(dto.session_id), conversation: bounded(dto.conversation_id),
      requestSession: bounded(args?.sessionId),
      generation: bounded(dto.generation), epoch: dto.source_epoch === null ? null : bounded(dto.source_epoch),
      validEpoch: dto.source_epoch === null || Boolean(bounded(dto.source_epoch)), revision: bounded(dto.revision), aliases,
      admission: dto.recent_admission == null ? null : bounded(dto.recent_admission),
      progress: bounded(dto.progress), older: dto.older_page, reset: dto.reset,
      before: bounded(args?.cursor), next: bounded(dto.next_before), detail: Boolean(dto.detail),
      rows: dto.events.map((row) => ({
        id: bounded(row.id), key: bounded(row.metadata?.chat_display_key) ?? bounded(row.id),
        session: bounded(row.session_id), provider: bounded(row.provider), role: row.role, kind: row.kind,
        turn: bounded(row.turn_id), text: typeof row.text === "string" ? row.text.slice(0, 96) : null,
        length: typeof row.text === "string" ? row.text.length : null,
        members: members(row.metadata?.chat_display_member_ids),
        refs: members(row.metadata?.archive_record?.event_refs),
        target: row.kind === "message" && row.role === "assistant" && row.provider === (config.provider ?? "mock")
          && row.text === config.body && (config.eventId == null || row.id === config.eventId
            || members(row.metadata?.chat_display_member_ids)?.includes(config.eventId)
            || members(row.metadata?.archive_record?.event_refs)?.includes(config.eventId)
            || aliases.some((alias) => alias.observation === config.eventId && alias.canonical === row.id)),
      })),
    });
  };
  const observeRequest = (args, result) => {
    if (retired) return;
    const command = "load_agent_chat_page";
    if (state.calls.length >= 128) fail("passive call bound exceeded");
    else state.calls.push({ command, at: now(), session: bounded(args?.sessionId),
      before: bounded(args?.cursor), detail: bounded(args?.detailRef),
      revalidate: Array.isArray(args?.revalidateIds) });
    // Attach directly to the original Promise. Do not return a chained Promise,
    // await it, clone a result, or turn a rejection into a successful value.
    try { result.then((dto) => {
      if (retired) return;
      const started = now();
      try { record(command, args, dto, started); } catch (error) { fail(error); }
      finally { state.captureCostMs += now() - started; }
    },
      () => { if (!retired) state.rejections++; }); } catch (error) { fail(error); }
  };
  const intersectsViewport = (rect, region) => rect.width > 0 && rect.height > 0
    && rect.bottom > Math.max(0, region.top) && rect.top < Math.min(innerHeight, region.bottom)
    && rect.right > Math.max(0, region.left) && rect.left < Math.min(innerWidth, region.right);
  const visibleBody = (subject = null, diagnostic = null) => {
    const reject = (gate) => { if (diagnostic) diagnostic.firstRejectionGate = gate; return null; };
    if (document.visibilityState !== "visible") return reject("document-hidden");
    const container = ownedContainer();
    const root = container?.node;
    if (!root) return reject("owned-container");
    const scroll = root?.querySelector('[data-testid="agent-chat-scroll-region"]');
    if (!scroll) return reject("chat-scroll");
    if (!root.querySelector('button[title="Switch to Terminal"]')) return reject("chat-mode");
    // Read the consumer's admitted page, not merely the last returned DTO.
    const admitted = parse(scroll.getAttribute("data-chat-page-scope"));
    const admittedKey = scopeKey(admitted);
    if (!admittedKey) return reject("admitted-scope");
    if (admitted.session !== state.owner) return reject("admitted-owner");
    if (config.conversation != null && admitted.conversation !== config.conversation) return reject("admitted-conversation");
    const region = scroll.getBoundingClientRect();
    const contents = Array.from(root.querySelectorAll('article[aria-label="assistant message"] .chat-message-content'));
    let firstTargetRejection = null;
    const rejectedTarget = (gate) => { firstTargetRejection ??= gate; };
    // A duplicate may become first without changing the measured subject.
    // Revalidate the requested node; never fall back to another equal-text row.
    for (const content of subject ? contents.filter((node) => node === subject.node) : contents) {
      if (!content.isConnected || !readable(content)) {
        if (matchesText(content.textContent)) rejectedTarget("target-visibility");
        continue;
      }
      const range = textRange(content);
      if (!range) continue;
      const slot = content.closest("[data-chat-row-key]");
      if (!slot?.isConnected || !root.contains(slot)) { rejectedTarget("target-row"); continue; }
      const key = slot?.getAttribute("data-chat-row-key");
      const identity = parse(slot?.getAttribute("data-chat-row-identity"));
      const receipt = state.receipts.findLast((item) => item.session === state.owner
        && item.requestSession === state.owner && item.validEpoch && !item.detail
        && scopeKey(item) === admittedKey && item.rows.some((row) => row.target
          && row.session === state.owner && row.id === identity?.id
          && JSON.stringify(row.members) === JSON.stringify(identity.members)
          && (row.key === key || item.aliases.some((alias) => alias.observation === key && alias.canonical === row.id))));
      if (!receipt || !key) { rejectedTarget("target-receipt"); continue; }
      // Measure the actual body text, not the row/header of a tall message.
      const rects = Array.from(range.getClientRects());
      if (!rects.some((rect) => intersectsViewport(rect, region))) { rejectedTarget("target-viewport"); continue; }
      if (diagnostic) diagnostic.firstRejectionGate = null;
      return { node: content, row: slot, scroll, container, evidence: { key, eventId: identity.id, members: identity.members,
        container: container.identity,
        scopeMode: scopeMode(receipt), session: receipt.session, receiptAt: receipt.at, window: receipt.revision, revision: receipt.revision, generation: receipt.generation,
        epoch: receipt.epoch, conversation: receipt.conversation, admission: receipt.admission,
        witnessKind: witness.kind, bodySha256: witness.bodySha256 ?? null,
        matchingBodySlots: contents.filter((node) => matchesText(node.textContent)).length } };
    }
    return reject(firstTargetRejection ?? "target-text");
  };
  const composerState = (root) => {
    const composers = root ? Array.from(root.querySelectorAll('textarea[aria-label="Message agent"]')) : [];
    const candidate = composers.length === 1 ? composers[0] : null;
    const rect = candidate?.getBoundingClientRect();
    const bounds = root?.getBoundingClientRect();
    return { candidate, flags: { present: composers.length > 0, unique: composers.length === 1,
      visible: Boolean(candidate?.isConnected && !candidate.hidden && root.contains(candidate) && readable(candidate)
        && document.visibilityState === "visible" && rect.width > 0 && rect.height > 0
        && rect.right > Math.max(0, bounds.left) && rect.bottom > Math.max(0, bounds.top)
        && rect.left < Math.min(innerWidth, bounds.right) && rect.top < Math.min(innerHeight, bounds.bottom)),
      enabled: Boolean(candidate && !candidate.disabled && !candidate.readOnly && candidate.getAttribute("aria-disabled") !== "true"),
      empty: Boolean(candidate && candidate.value === "") } };
  };
  const checkUsable = (phase) => {
    if (retired || !phase || phase.exposureAt === null || phase.usableAt !== null || phase.usableChecking
      || paintedBody?.phase !== phase) return;
    const body = paintedBody.body;
    const valid = () => sameBody(body, visibleBody(body)) && ((inputEvidence?.phase === phase
      && sameBody(inputEvidence.body, body)
      && inputEvidence.node.value === config.probe && document.activeElement === inputEvidence.node)
      || (wheelEvidence?.phase === phase && wheelEvidence.received && sameBody(wheelEvidence.body, body)
        && wheelEvidence.node.scrollTop !== wheelEvidence.before));
    if (!valid()) return;
    phase.usableChecking = true;
    frame(() => frame(() => {
      phase.usableChecking = false;
      if (state.phase === phase && valid()) phase.usableAt = now();
    }));
  };
  const sample = () => {
    if (retired) return;
    const presented = visibleBody(paintedBody?.phase === state.phase ? paintedBody.body : null);
    if (presented && state.presentations.length < 64
      && JSON.stringify(state.presentations.at(-1)?.body) !== JSON.stringify(presented.evidence)) {
      state.presentations.push({ at: now(), body: presented.evidence, ...presented.evidence });
    }
    const phase = state.phase;
    if (phase && phase.rosterAt !== null && presented && phase.exposureAt === null) {
      phase.exposureAt = now();
      phase.exposureKind = phase.chatAt === null ? "automatic" : "explicit-toggle";
    }
    checkUsable(phase);
    if (!phase || phase.exposureAt === null || phase.paintAt !== null || phase.checking) return;
    const body = presented;
    if (!body || body.evidence.receiptAt > now()) return;
    phase.checking = true;
    const commitAt = now();
    frame(() => frame(() => {
      phase.checking = false;
      if (state.phase !== phase) return;
      const current = visibleBody(body);
      if (sameBody(current, body)) {
        phase.commitAt = commitAt;
        phase.paintAt = now();
        phase.body = current.evidence;
        paintedBody = { phase, body: current };
        checkUsable(phase);
      }
    }));
  };
  const rosterSelection = (event) => {
    if (retired || !event.isTrusted || !state.phase) return;
    const row = event.target.closest(`[aria-label="Agent ${config.name}"]`);
    if (row?.getAttribute("aria-label") === `Agent ${config.name}` && state.phase.rosterAt === null) {
      state.phase.rosterAt = now();
      state.phase.rosterAction = event.type;
      state.phase.preselectionPage = surface()?.querySelector('[data-testid="agent-chat-scroll-region"]')
        ?.getAttribute("data-chat-page-scope") ?? null;
    }
  };
  const click = (event) => {
    if (retired || !event.isTrusted || !state.phase) return;
    rosterSelection(event);
    const phase = state.phase;
    const root = surface();
    const button = event.target.closest('button[title="Switch to Chat"]');
    if (button && root?.contains(button) && phase.chatAt === null) {
      phase.chatAt = now();
      sample();
    }
    const intent = event.target.closest("button");
    if (intent && root?.contains(intent) && state.intents.length < 64 && /Load older transcript|Show full details|Read more/.test(intent.textContent)) {
      state.intents.push({ kind: /older/.test(intent.textContent) ? "older" : "detail", at: now() });
    }
  };
  const input = (event) => {
    const phase = state.phase;
    if (retired || !event.isTrusted || !phase || phase.exposureAt === null || phase.usableAt !== null
      || !surface()?.contains(event.target) || event.target.getAttribute("aria-label") !== "Message agent"
      || event.target.value !== config.probe || document.activeElement !== event.target) return;
    inputEvidence = { phase, node: event.target, body: visibleBody(paintedBody?.phase === phase ? paintedBody.body : null) };
    checkUsable(phase);
  };
  const wheel = (event) => {
    if (retired) return;
    const scroll = surface()?.querySelector('[data-testid="agent-chat-scroll-region"]');
    if (!retired && event.isTrusted && scroll && (scroll === event.target || scroll.contains(event.target)) && state.intents.length < 64) {
      state.intents.push({ kind: event.deltaY < 0 ? "older" : "wheel", at: now() });
      const phase = state.phase;
      if (phase && wheelEvidence?.phase === phase && wheelEvidence.node === scroll && phase.usableAt === null) {
        wheelEvidence.received = true;
        wheelEvidence.eventTop = scroll.scrollTop;
        // Compositor-first scrolling is already eligible for the same two-frame
        // witness as input. The queued check also covers event-first scrolling.
        checkUsable(phase);
        frame(() => checkUsable(phase));
      }
    }
  };
  document.addEventListener("pointerdown", rosterSelection, true);
  document.addEventListener("click", click, true);
  document.addEventListener("input", input, true);
  document.addEventListener("wheel", wheel, true);
  const observer = new MutationObserver(sample);
  observer.observe(document.documentElement, { childList: true, subtree: true, characterData: true,
    attributes: true, attributeFilter: ["data-chat-page-scope", "data-chat-row-identity"] });
  window.__chatAcceptance = {
    state,
    container(owner) {
      if (owner !== state.owner) throw new Error("container owner does not match bound observation");
      return surface();
    },
    containerTab(owner) {
      if (owner !== state.owner) throw new Error("container owner does not match bound observation");
      return ownedContainer()?.tab ?? null;
    },
    describeContainers() {
      const nodes = containerNodes();
      const matches = ownedContainers();
      return { status: "OBSERVED_CONTAINERS", count: nodes.length, truncated: nodes.length > 64,
        ownedMatchCount: matches.length, ambiguous: matches.length > 1,
        containers: nodes.slice(0, 64).map((node) => {
          const item = inspectContainer(node);
          return { ...item.identity, connected: node.isConnected, visible: item.visible,
            selected: item.selected, eligible: item.valid, expectedOwner: item.identity.owner === state.owner };
        }) };
    },
    subscriptionStatus() { return subscription?.status() ?? { active: false, fault_count: 0, request_count: 0 }; },
    subjectStatus() {
      const body = paintedBody?.phase === state.phase ? paintedBody.body : null;
      const fresh = body ? visibleBody(body) : null;
      const candidate = visibleBody();
      return { latched: Boolean(body), latchedValid: Boolean(sameBody(body, fresh)),
        latchedEventId: body?.evidence.eventId ?? null, currentCandidateEventId: candidate?.evidence.eventId ?? null,
        candidateMatchesLatch: Boolean(sameBody(body, candidate)),
        paintedMatchingBodySlots: body?.evidence.matchingBodySlots ?? null,
        currentMatchingBodySlots: fresh?.evidence.matchingBodySlots ?? candidate?.evidence.matchingBodySlots ?? null };
    },
    // Failure-only DOM facts. This never changes an endpoint or dispatches input.
    candidateStatus() {
      const root = surface();
      const scroll = root?.querySelector('[data-testid="agent-chat-scroll-region"]');
      const admitted = parse(scroll?.getAttribute("data-chat-page-scope"));
      const fields = ["session", "conversation", "generation", "epoch", "revision", "admission"];
      const actualScope = admitted ? Object.fromEntries(fields.map((key) => [key,
        admitted[key] === null ? null : bounded(admitted[key])])) : null;
      const contents = root ? Array.from(root.querySelectorAll('article[aria-label="assistant message"] .chat-message-content')) : [];
      const targetIndex = contents.findIndex((node) => matchesText(node.textContent));
      const target = contents[targetIndex];
      const row = target?.closest("[data-chat-row-key]");
      const identity = parse(row?.getAttribute("data-chat-row-identity"));
      const range = target ? textRange(target) : null;
      const rects = range ? Array.from(range.getClientRects()) : [];
      const region = scroll?.getBoundingClientRect();
      const diagnostic = { firstRejectionGate: null };
      const candidate = visibleBody(null, diagnostic);
      return { ...diagnostic, scopeMode: scopeMode(admitted) ?? "invalid", actualScope,
        scopeFieldsPresent: Object.fromEntries(fields.map((key) => [key, Boolean(admitted && Object.hasOwn(admitted, key))])),
        candidateValid: Boolean(candidate), assistantContents: contents.length,
        candidateIndex: candidate ? contents.indexOf(candidate.node) : null,
        targetPresent: targetIndex >= 0, targetIndex: targetIndex >= 0 ? targetIndex : null,
        targetIdentity: identity ? { id: bounded(identity.id), members: members(identity.members) } : null,
        scroll: scroll ? { top: scroll.scrollTop, clientHeight: scroll.clientHeight, scrollHeight: scroll.scrollHeight } : null,
        range: { count: rects.length, captured: Math.min(64, rects.length), truncated: rects.length > 64,
          intersects: Boolean(region && rects.some((rect) => intersectsViewport(rect, region))),
          rects: rects.slice(0, 64).map((rect) => ({ top: rect.top, bottom: rect.bottom, left: rect.left, right: rect.right,
            width: rect.width, height: rect.height })) } };
    },
    interactionStatus() {
      return { ...state.interaction, currentComposer: composerState(surface()).flags,
        wheel: wheelEvidence ? { prepared: true, received: wheelEvidence.received,
        before: wheelEvidence.before, eventTop: wheelEvidence.eventTop, after: wheelEvidence.node.scrollTop,
        changed: wheelEvidence.node.scrollTop !== wheelEvidence.before } : { prepared: false, received: false,
        before: null, eventTop: null, after: null, changed: false } };
    },
    prepareInteraction(owner, requestedScroll) {
      if (retired || owner !== state.owner) throw new Error("interaction owner is not active");
      const root = surface();
      const scroll = root?.querySelector('[data-testid="agent-chat-scroll-region"]');
      if (!scroll || scroll !== requestedScroll) throw new Error("interaction region is not the current owned Chat");
      const body = paintedBody?.phase === state.phase ? paintedBody.body : null;
      const fresh = body ? visibleBody(body) : null;
      const latchValid = Boolean(sameBody(body, fresh));
      const { candidate, flags } = composerState(root);
      const safe = flags.unique && flags.visible && flags.enabled && flags.empty;
      const overflow = scroll.scrollHeight > scroll.clientHeight && scroll.clientHeight > 0;
      const control = !latchValid ? "none" : safe ? "keyboard" : overflow ? "wheel" : "none";
      const reason = !latchValid ? "invalid_painted_subject" : safe ? "safe_empty_composer"
        : !flags.present ? "composer_absent" : !flags.unique ? "composer_ambiguous"
          : !flags.visible ? "composer_hidden" : !flags.enabled ? "composer_disabled" : "composer_nonempty";
      state.interaction = { control, reason, composer: flags, latchValid, overflow };
      // Capture before dispatch, not in a wheel callback that may follow compositor scrolling.
      wheelEvidence = control === "wheel" ? { phase: state.phase, node: scroll, before: scroll.scrollTop,
        eventTop: null, received: false, body: fresh } : null;
      return { ...state.interaction, input: control === "keyboard" ? candidate : null, top: scroll.scrollTop };
    },
    bind(owner) {
      if (retired || (state.owner && state.owner !== owner)) throw new Error("owner cannot change or bind after disposal");
      if (subscription) return;
      const facade = window.__wardianChatPageObservation;
      if (facade?.version !== 1 || typeof facade.subscribe !== "function") throw new Error("page observer facade unavailable/incompatible");
      const ack = facade.subscribe({ version: 1, owner, onRequest: observeRequest });
      if (!ack.active || ack.version !== 1 || ack.owner !== owner || !Number.isSafeInteger(ack.token)
        || ack.token <= 0 || typeof ack.status !== "function" || !ack.status().active) {
        ack.unsubscribe?.();
        throw new Error("page observer activation failed");
      }
      subscription = ack;
      state.owner = owner;
      state.activation = { active: true, version: ack.version, owner, token: ack.token, at: now() };
    },
    arm(label) {
      if (retired || !subscription?.status().active || subscription.status().fault_count) throw new Error("page observer is not active/healthy");
      if (state.phases.length >= 4) throw new Error("measurement phase bound exceeded");
      inputEvidence = null;
      wheelEvidence = null;
      paintedBody = null;
      state.interaction = null;
      state.phase = { label, rosterAt: null, chatAt: null, exposureAt: null, exposureKind: null,
        commitAt: null, paintAt: null, usableAt: null, body: null, checking: false };
      state.phases.push(state.phase);
    },
    sample,
    dispose() {
      if (retired) return disposalReceipt;
      retired = true;
      subscription?.unsubscribe();
      observer.disconnect();
      document.removeEventListener("pointerdown", rosterSelection, true);
      document.removeEventListener("click", click, true);
      document.removeEventListener("input", input, true);
      document.removeEventListener("wheel", wheel, true);
      inputEvidence = null; wheelEvidence = null; paintedBody = null;
      state.disposed = true;
      const status = subscription?.status() ?? null;
      disposalReceipt = { status: status ? "OBSERVED_POST_DISPOSE" : "DISPOSED_WITHOUT_SUBSCRIPTION",
        disposed: state.disposed, token: subscription?.token ?? null, owner: state.owner, subscription: status,
        callback_count: status?.request_count ?? null, callback_fault_count: status?.fault_count ?? null,
        request_count: state.calls.length, receipt_count: state.receipts.length,
        observer_error_count: state.errors.length, rejection_count: state.rejections };
      return disposalReceipt;
    },
  };
}

async function readObservation(driver) {
  return driver.executeScript(() => ({ ...window.__chatAcceptance.state, subscription: window.__chatAcceptance.subscriptionStatus() }));
}

/** Return the acknowledgement from the existing disposal, before its realm closes. */
async function disposeObservation(driver) {
  return driver.executeScript(() => window.__chatAcceptance?.dispose() ?? { status: "UNOBSERVED_POST_DISPOSE_ACK" });
}

/** Maintained normal-flow case; app-free checks never import this module. */
const skipNativeBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
const RUN_ID = `${process.pid}-${Date.now()}`;

function withEnv(t, key, value) {
  const previous = process.env[key];
  process.env[key] = value;
  t.after(() => {
    if (previous === undefined) delete process.env[key];
    else process.env[key] = previous;
  });
}

/** Failure-only metadata; layout boxes are diagnostics, never a paint witness. */
function captureRosterDiagnostics(sessionId, name, done) {
  const bounded = (value) => typeof value === "string" ? value.slice(0, 160) : null;
  const result = { roster: null, registry: null, containers: null };
  let finished = false;
  let timer;
  const finish = (registry) => {
    if (finished) return;
    finished = true;
    clearTimeout(timer);
    done({ ...result, registry });
  };
  try {
    const roster = document.querySelector('[data-testid="agent-watchlist"]');
    const rows = roster ? Array.from(roster.querySelectorAll('[aria-label^="Agent "]')) : [];
    const geometry = (node) => {
      const rect = node.getBoundingClientRect();
      const style = window.getComputedStyle(node);
      return { width: rect.width, height: rect.height,
        hasLayout: node.isConnected && rect.width > 0 && rect.height > 0
          && style.display !== "none" && style.visibility !== "hidden" };
    };
    result.roster = { present: Boolean(roster), geometry: roster ? geometry(roster) : null,
      rowCount: rows.length, truncated: rows.length > 64,
      expectedNamePresent: rows.some((row) => row.getAttribute("aria-label") === "Agent " + name),
      rows: rows.slice(0, 64).map((row) => ({
        aria_label: bounded(row.getAttribute("aria-label")), ...geometry(row),
      })),
      filter: bounded(roster?.querySelector("input")?.value),
      heading: bounded(roster?.querySelector("h2")?.textContent) };
  } catch (error) {
    result.roster = { error: bounded(String(error)) };
  }
  try {
    result.containers = window.__chatAcceptance?.describeContainers?.() ?? { status: "OBSERVER_UNAVAILABLE" };
  } catch (error) {
    result.containers = { status: "CONTAINER_CAPTURE_ERROR", error: bounded(String(error)) };
  }
  // Bound the passive IPC inside the WebView so a late result cannot replace
  // a timeout or leave this diagnostic callback waiting for the backend.
  timer = setTimeout(() => finish({ status: "REGISTRY_TIMEOUT" }), 2_000);
  Promise.resolve().then(() => window.__TAURI_INTERNALS__.invoke("list_agents", {})).then(
    (agents) => {
      if (!Array.isArray(agents)) return finish({ status: "MALFORMED_REGISTRY" });
      finish({ status: "OBSERVED_REGISTRY", count: agents.length, truncated: agents.length > 64,
        expectedOwnerPresent: agents.some((agent) => agent?.session_id === sessionId),
        expectedNamePresent: agents.some((agent) => agent?.session_name === name),
        agents: agents.slice(0, 64).map((agent) => ({
          session_id: bounded(agent?.session_id), session_name: bounded(agent?.session_name),
          provider: bounded(agent?.provider), agent_class: bounded(agent?.agent_class),
          is_off: typeof agent?.is_off === "boolean" ? agent.is_off : null,
        })) });
    },
    (error) => finish({ status: "REGISTRY_ERROR", code: bounded(error?.code),
      error: bounded(error?.message ?? String(error)) }),
  );
}

async function visible(driver, selector, within = driver) {
  return driver.wait(async () => {
    for (const element of await within.findElements(By.css(selector))) {
      if (await element.isDisplayed()) return element;
    }
    return false;
  }, 10_000, `owned visible element missing: ${selector}`);
}

async function ownedSurface(driver, sessionId) {
  return driver.wait(() => driver.executeScript((owner) => window.__chatAcceptance.container(owner), sessionId),
    10_000, "owned visible current Chat container missing or ambiguous");
}

/** Wait only for the real raw producer, never a transcript/index/admission IPC. */
async function waitForProducer(driver, harness, sessionId, fixture) {
  const log = path.join(harness.isolatedHome, "agents", sessionId, "mock-transcript.jsonl");
  return driver.wait(() => {
    if (!fs.existsSync(log)) return false;
    const fd = fs.openSync(log, "r");
    try {
      const stat = fs.fstatSync(fd, { bigint: true });
      assert.ok(stat.isFile() && stat.size <= 1024n * 1024n, "bounded plain Mock log required");
      const raw = fs.readFileSync(fd, "utf8");
      if (!raw.endsWith("\n")) return false;
      const frames = raw.trimEnd().split("\n").map((line) => JSON.parse(line));
      if (!frames.some((frame) => frame.type === "model" && frame.turn_id === fixture.latestTurn
        && frame.content === fixture.latestBody)) return false;
      if (!frames.some((frame) => frame.type === "result" && frame.turn_id === fixture.latestTurn
        && frame.status === "success")) return false;
      assert.ok(frames.length <= fixture.maxRawFrames);
      return { path: log, dev: String(stat.dev), ino: String(stat.ino), bytes: String(stat.size),
        sha256: createHash("sha256").update(raw).digest("hex"), frames: frames.length };
    } finally { fs.closeSync(fd); }
  }, 30_000, "independently declared completed Mock body/result never published");
}

/** Failure-only DOM metadata. Never reads body text or sends another IPC request. */
function captureChatFailureMetadata(sessionId, stage) {
  const observer = window.__chatAcceptance;
  let root = null;
  try { root = observer?.container(sessionId); } catch { /* Ambiguity remains in container metadata. */ }
  const region = root?.querySelector('[data-testid="agent-chat-scroll-region"]');
  const rect = region?.getBoundingClientRect();
  const allRows = region ? Array.from(region.querySelectorAll('[data-chat-row-identity]')) : [];
  const rows = allRows.slice(0, 64);
  const phase = observer?.state.phase;
  return {
    status: "OBSERVED_LATE_DOM", stage, visibility: document.visibilityState,
    containers: observer?.describeContainers() ?? null,
    mode: root?.querySelector('button[title="Switch to Terminal"]') ? "chat"
      : root?.querySelector('button[title="Switch to Chat"]') ? "terminal" : "unresolved",
    mountedChat: Boolean(region), suspended: Boolean(root?.querySelector('[data-agent-chat-state="suspended"]')),
    scopePresent: Boolean(region?.getAttribute("data-chat-page-scope")), memberRows: rows.length,
    totalMemberRows: allRows.length, capturedMemberRows: rows.length, memberRowsTruncated: allRows.length > rows.length,
    geometry: rect ? { width: rect.width, height: rect.height, top: rect.top, left: rect.left } : null,
    observer: observer ? { activation: observer.state.activation, subscription: observer.subscriptionStatus(),
      subject: observer.subjectStatus?.() ?? null, interaction: observer.interactionStatus?.() ?? null,
      candidate: observer.candidateStatus?.() ?? null,
      calls: observer.state.calls.length, receipts: observer.state.receipts.length,
      errors: observer.state.errors.length, rejections: observer.state.rejections,
      endpoints: phase ? Object.fromEntries(["rosterAt", "chatAt", "exposureAt", "commitAt", "paintAt", "usableAt"]
        .map((key) => [key, phase[key]])) : null } : null,
  };
}

async function measureChat(driver, sessionId, label, name, evidence, requireTerminalToggle = true) {
  await driver.executeScript((phase) => window.__chatAcceptance.arm(phase), label);
  const deadline = Date.now() + 5_000;
  let surface;
  let mode;
  let stage = "roster-click";
  try {
    await (await visible(driver, `[aria-label="Agent ${name}"]`)).click();
    stage = "surface-resolution";
    surface = await ownedSurface(driver, sessionId);
    stage = "mode-resolution";
    mode = await visible(driver, 'button[title="Switch to Chat"], button[title="Switch to Terminal"]', surface);
    stage = "chat-toggle";
    const explicit = await mode.getAttribute("title") === "Switch to Chat";
    if (requireTerminalToggle) assert.ok(explicit, "live Terminal measurement requires its real Chat toggle");
    if (explicit) await mode.click();
    const waitForEndpoint = (field) => driver.wait(async () => {
      const observation = await readObservation(driver);
      assert.deepEqual(observation.errors, [], "passive observer failed");
      assert.ok(observation.subscription.active && observation.subscription.fault_count === 0, "passive subscription failed");
      return observation.phase[field] !== null;
    }, Math.max(1, deadline - Date.now()), `${label}: no source-bound ${field}`);
    stage = "paint";
    await waitForEndpoint("paintAt");
    stage = "input";
    const scroll = await visible(driver, '[data-testid="agent-chat-scroll-region"]', surface);
    const plan = await driver.executeScript((node, owner) => window.__chatAcceptance.prepareInteraction(owner, node), scroll, sessionId);
    assert.ok(["keyboard", "wheel"].includes(plan.control), "no safe owned usability control");
    const input = plan.control === "keyboard" ? plan.input : null;
    if (input) {
      assert.ok(await input.isDisplayed(), "unsent composer is no longer visible");
      assert.ok(await input.isEnabled(), "no safe enabled unsent composer interaction");
      assert.equal(await input.getAttribute("value"), "", "fixture must not replace an existing draft");
      await input.sendKeys("native-input-probe");
    } else {
      assert.equal(plan.control, "wheel", "planned keyboard control is missing its actual composer");
      await driver.actions({ async: true }).scroll(0, 0, 0, plan.top > 0 ? -20 : 20, scroll).perform();
    }
    stage = "usability";
    await waitForEndpoint("usableAt");
    stage = "assertions";
    const state = await readObservation(driver);
    const phase = state.phase;
    assert.ok(Number.isFinite(phase.rosterAt), "original trusted Roster selection was not observed");
    if (explicit) {
      assert.ok(Number.isFinite(phase.chatAt) && phase.chatAt >= phase.rosterAt,
        "trusted owned Chat toggle was not observed");
      assert.equal(phase.exposureKind, "explicit-toggle");
      assert.ok(phase.exposureAt >= phase.chatAt);
    } else {
      assert.equal(phase.chatAt, null, "automatic Chat must not invent an explicit toggle timestamp");
      assert.equal(phase.exposureKind, "automatic");
    }
    assert.ok(Number.isFinite(phase.exposureAt) && phase.exposureAt >= phase.rosterAt
      && phase.commitAt >= phase.exposureAt && phase.paintAt >= phase.commitAt);
    assert.ok(phase.usableAt >= phase.paintAt);
    assert.ok(phase.paintAt - phase.rosterAt < 1000, `${label}: Roster-to-readable Chat exceeded 1000ms`);
    assert.ok(phase.usableAt - phase.rosterAt < 1000, `${label}: Roster-to-usable Chat exceeded 1000ms`);
    assert.ok(state.calls.every((call) => call.at >= state.phases[0].rosterAt), "Chat was prewarmed before Roster selection");
    assert.equal(state.coverage, "PAGE_LOADER_ONLY");
    assert.equal(state.broaderIpcRuntime, "UNOBSERVED");
    assert.ok(state.activation?.active && state.activation.owner === sessionId
      && state.activation.at <= state.phases[0].rosterAt, "page observation was not activated before Roster");
    assert.ok(phase.body && state.calls.some((call) => call.session === sessionId
      && call.at >= state.phases[0].rosterAt && call.at <= phase.body.receiptAt), "no original owned page request");
    assert.ok(state.receipts.some((receipt) => receipt.session === sessionId && receipt.requestSession === sessionId
      && receipt.at === phase.body.receiptAt && receipt.at <= phase.paintAt), "no actual receipt linked to admitted paint");
    return phase;
  } catch (error) {
    const early = ["roster-click", "surface-resolution", "mode-resolution"].includes(stage);
    const failures = early ? evidence.rosterFailures : (evidence.measurementFailures ??= []);
    try {
      const metadata = early ? await driver.executeAsyncScript(captureRosterDiagnostics, sessionId, name)
        : await driver.executeScript(captureChatFailureMetadata, sessionId, stage);
      failures.push({ label, stage, metadata });
    } catch (captureError) {
      failures.push({ label, stage, status: "CAPTURE_FAILED", error: String(captureError).slice(0, 160) });
    }
    throw error;
  }
}

/** Preserve the live Terminal route when requested; never toggle a Saved Off view. */
async function closeTerminalSurface(driver, sessionId, restoreLiveTerminal = false) {
  const surface = await ownedSurface(driver, sessionId);
  if (restoreLiveTerminal) await (await visible(driver, 'button[title="Switch to Terminal"]', surface)).click();
  const tab = await driver.executeScript((owner) => window.__chatAcceptance.containerTab(owner), sessionId);
  assert.ok(tab, "owned surface has no actual selected tab");
  await (await visible(driver, "[data-tab-close]", tab)).click();
}

function canonicalHistoryReceipts(receipts, sessionId) {
  let scope = null;
  let canonical = [];
  for (const receipt of receipts) {
    if (receipt.session !== sessionId) continue;
    if (receipt.reset === true) { canonical = []; scope = null; }
    // Revision is opaque: Older changes its seed. Explicit scope and reset
    // delimit this controlled current cohort, not parsed token components.
    if (typeof receipt.generation !== "string" || !receipt.generation
      || typeof receipt.revision !== "string" || !receipt.revision
      || typeof receipt.conversation !== "string" || !receipt.conversation
      || !(receipt.epoch === null || (typeof receipt.epoch === "string" && receipt.epoch))) continue;
    const next = JSON.stringify([receipt.conversation, receipt.generation, receipt.epoch]);
    if (scope !== null && scope !== next) canonical = [];
    scope = next;
    if (receipt.admission === null && receipt.progress === "ready" && !receipt.detail) canonical.push(receipt);
  }
  return canonical;
}

/** Real scroll/click demand only; opaque cursors are compared, never decoded. */
async function exerciseHistory(driver, sessionId, fixture) {
  const surface = await ownedSurface(driver, sessionId);
  const scroll = await visible(driver, '[data-testid="agent-chat-scroll-region"]', surface);
  await driver.wait(async () => canonicalHistoryReceipts((await readObservation(driver)).receipts, sessionId)
    .some((receipt) => receipt.next),
  30_000, "normal canonical page/cursor never arrived");
  const before = await readObservation(driver);
  assert.equal(before.calls.some((call) => call.before || call.detail), false, "lazy work before user demand");
  const used = new Set();
  const bound = Math.ceil(fixture.maxRawFrames / 80) + 2;
  for (let index = 0; index < bound; index += 1) {
    const state = await readObservation(driver);
    const cohort = canonicalHistoryReceipts(state.receipts, sessionId);
    const page = cohort.findLast((receipt) => !receipt.older);
    const older = cohort.filter((receipt) => receipt.older === true);
    const cursor = older.length ? older.at(-1).next : page?.next;
    if (!cursor) break;
    assert.equal(used.has(cursor), false, "opaque Older cursor repeated");
    used.add(cursor);
    const count = older.length;
    await driver.actions({ async: true }).scroll(0, 0, 0, -10000, scroll).perform();
    const buttons = await surface.findElements(By.xpath('.//button[contains(., "Load older transcript")]'));
    if (buttons.length && await buttons[0].isDisplayed()) await buttons[0].click();
    await driver.wait(async () => canonicalHistoryReceipts((await readObservation(driver)).receipts, sessionId)
      .filter((receipt) => receipt.older === true).length > count,
    10_000, "trusted Older demand produced no normal page");
  }
  const state = await readObservation(driver);
  const pages = canonicalHistoryReceipts(state.receipts, sessionId).filter((receipt) => receipt.older === true);
  assert.ok(pages.length > 0 && pages.at(-1).next === null, "Older did not exhaust within declared frame bound");
  assert.ok(state.intents.some((intent) => intent.kind === "older"), "no trusted Older input");
  for (const call of state.calls.filter((call) => call.before)) {
    assert.ok(state.intents.some((intent) => intent.kind === "older" && intent.at <= call.at));
  }
  const rows = new Map(canonicalHistoryReceipts(state.receipts, sessionId)
    .flatMap((receipt) => receipt.rows).map((row) => [row.key, row]));
  assert.ok(rows.size > 80, "fixture did not exercise more than one normal header cohort");
  for (let index = 0; index < fixture.historyTurns; index += 1) {
    assert.ok([...rows.values()].some((row) => row.role === "assistant"
      && row.text === `Controlled assistant history ${index} is complete.`), `history ${index} skipped`);
  }
  const repeats = [...rows.values()].filter((row) => row.role === "assistant" && row.text === fixture.repeatBody);
  assert.equal(repeats.length, 2, "equal text must retain two assistant identities");
  assert.equal(new Set(repeats.map((row) => row.id)).size, 2);
  assert.equal(new Set(repeats.map((row) => row.turn)).size, 2);
  const long = [...rows.values()].find((row) => row.role === "assistant" && row.turn === "chat-acceptance-long");
  assert.ok(long, "independent full-body owner absent from actual pages");
  const longRow = await visible(driver, `[data-chat-row-key="${long.key}"]`, surface);
  const detailButtons = await longRow.findElements(By.xpath('.//button[normalize-space()="Show full details"]'));
  assert.equal(detailButtons.length, 1, "long body did not retain lazy details");
  await detailButtons[0].click();
  const detailBound = Math.ceil(Buffer.byteLength(fixture.longBody) / 65536) + 1;
  for (let index = 0; index < detailBound; index += 1) {
    const pre = await visible(driver, "pre", longRow);
    if (await pre.getText() === fixture.longBody) break;
    const more = await longRow.findElements(By.xpath('.//button[normalize-space()="Read more"]'));
    assert.equal(more.length, 1, "full-body window incomplete without continuation");
    await more[0].click();
  }
  assert.equal(await (await visible(driver, "pre", longRow)).getText(), fixture.longBody);
  const completed = await readObservation(driver);
  assert.ok(completed.calls.some((call) => call.detail && completed.intents
    .some((intent) => intent.kind === "detail" && intent.at <= call.at)), "detail did not follow trusted intent");
  return { olderPages: pages.length, distinctHeaders: rows.size, detailBytes: Buffer.byteLength(fixture.longBody) };
}

function redactObservation(value) {
  if (typeof value === "string") return createHash("sha256").update(value).digest("hex");
  if (Array.isArray(value)) return value.map(redactObservation);
  if (!value || typeof value !== "object") return value;
  const result = {};
  for (const [key, item] of Object.entries(value)) {
    const publicToken = ["label", "command", "kind", "provider", "role", "progress"].includes(key)
      || (key === "exposureKind" && ["automatic", "explicit-toggle"].includes(item))
      || (key === "witnessKind" && ["whole", "needle"].includes(item))
      || (key === "status" && ["UNPROVEN", "OBSERVED_ENDPOINTS_CLOSURE_UNPROVEN",
        "OBSERVED_REGISTRY", "REGISTRY_TIMEOUT", "MALFORMED_REGISTRY", "REGISTRY_ERROR", "CAPTURE_FAILED",
        "OBSERVED_CONTAINERS", "OBSERVER_UNAVAILABLE", "CONTAINER_CAPTURE_ERROR", "OBSERVED_LATE_DOM",
        "OBSERVED_POST_DISPOSE", "DISPOSED_WITHOUT_SUBSCRIPTION", "UNOBSERVED_POST_DISPOSE_ACK"].includes(item))
      || (key === "stage" && ["roster-click", "surface-resolution", "mode-resolution", "chat-toggle", "paint",
        "input", "usability", "assertions", "draft-cleanup"].includes(item))
      || (key === "mode" && ["chat", "terminal", "unresolved"].includes(item))
      || (key === "visibility" && ["visible", "hidden", "prerender"].includes(item))
      || (key === "coverage" && item === "PAGE_LOADER_ONLY")
      || (key === "broaderIpcRuntime" && item === "UNOBSERVED")
      || (key === "scopeMode" && ["recent-preview", "durable", "invalid"].includes(item))
      || (key === "firstRejectionGate" && ["document-hidden", "owned-container", "chat-scroll", "chat-mode",
        "admitted-scope", "admitted-owner", "admitted-conversation", "target-text", "target-visibility",
        "target-row", "target-receipt", "target-viewport"].includes(item))
      || (key === "control" && ["keyboard", "wheel", "none"].includes(item))
      || (key === "reason" && ["invalid_painted_subject", "safe_empty_composer", "composer_absent",
        "composer_ambiguous", "composer_hidden", "composer_disabled", "composer_nonempty"].includes(item))
      || (/Sha256$/.test(key) && typeof item === "string" && /^[a-f0-9]{64}$/i.test(item))
      || (["agentPolicy", "globalPolicy", "effectivePolicy"].includes(key) && ["default", "enabled", "disabled"].includes(item));
    result[key] = typeof item === "string" && publicToken ? item : redactObservation(item);
  }
  return result;
}

// Native execution is intentionally separate from the app-free observer tests.
// Startup/cleanup errors fail this case; no catch-to-skip or synthetic UI input.
test("chat transcript renders the per-turn change surface", { timeout: 600_000 }, async (t) => {
  const harness = await createNativeHarness();
  if (!skipNativeBuild) ensureNativeAppBuilt(harness);
  assert.ok(harness.appPath);
  const fixture = chatAcceptanceFixture();
  const name = `Chat-Changes-${RUN_ID}`;
  withEnv(t, "WARDIAN_MOCK_SCENARIO", "chat_acceptance");
  withEnv(t, "WARDIAN_MOCK_DELAY_MS", "5");
  withEnv(t, "WARDIAN_NATIVE_SESSION_START_ATTEMPTS", "1");
  const externalManifest = process.env.WARDIAN_CHAT_SAVED_FIXTURE_MANIFEST;
  const freshHome = externalManifest ? freshSavedHome(harness) : null;
  prepareIsolatedHome(harness);
  const archive = externalManifest ? stageSavedFixture(harness, externalManifest, freshHome) : null;
  if (archive) withEnv(t, "CODEX_HOME", archive.codexHome);
  if (archive) withEnv(t, "CLAUDE_CONFIG_DIR", archive.claudeConfigDir);
  const workspace = path.join(harness.isolatedHome, `chat-workspace-${RUN_ID}`);
  fs.mkdirSync(workspace, { recursive: true });
  const sessions = [];
  const cleanupOwner = { sessionId: null };
  let paused = false;
  const evidence = { timings: [], history: null, endpoints: "UNRUN", preselectionIndex: [], rosterFailures: [], disposalAcknowledgements: [],
    closure: { status: "UNPROVEN", reason: "original-handle/fullbirth/exit+stream-EOF/Job-empty caller receipt required; timer is not proof" },
    cacheState: "cold process only; raw producer inspection warms filesystem", transition: "UNRUN" };
  t.after(async () => {
    const errors = [];
    for (const entry of sessions.toReversed()) {
      if (entry.closed) continue;
      try { evidence.lastObservation = await readObservation(entry.session.driver); }
      catch (error) { evidence.observationReadError = String(error).slice(0, 160); }
      try {
        const acknowledgement = await (entry.disposalPromise ??= disposeObservation(entry.session.driver));
        evidence.disposalAcknowledgements[sessions.indexOf(entry)] = { realm: sessions.indexOf(entry), acknowledgement };
      }
      catch (error) { errors.push(error); }
      try {
        if (cleanupOwner.sessionId && !paused) {
          await invokeTauri(entry.session.driver, "pause_agent", { sessionId: cleanupOwner.sessionId });
          paused = true;
        }
      } catch (error) { errors.push(error); }
      try { await entry.session.close(); entry.closed = true; } catch (error) { errors.push(error); }
    }
    const evidenceDir = path.join(harness.isolatedHome, "artifacts", "chat-normal-flow");
    fs.mkdirSync(evidenceDir, { recursive: true });
    // Redact once at the write boundary, including failure-path partial timings.
    fs.writeFileSync(path.join(evidenceDir, "result.json"), JSON.stringify(redactObservation(evidence), null, 2));
    if (errors.length) throw new AggregateError(errors, "owned native cleanup failed");
  });
  const open = async (observation = { name, body: fixture.latestBody, witness: chatBodyWitness(fixture.latestBody),
    probe: "native-input-probe" }) => {
    const session = await startNativeSession(harness);
    const entry = { session, closed: false };
    sessions.push(entry);
    await waitForAppShell(session.driver, 30_000);
    await session.driver.executeScript(installChatObservation, observation);
    return entry;
  };
  const publishedState = (sessionId, label) => {
    const index = path.join(harness.isolatedHome, "agents", sessionId, "conversations", "index.jsonl");
    evidence.preselectionIndex.push({ label, present: fs.existsSync(index),
      bytes: fs.existsSync(index) ? fs.statSync(index).size : null,
      observation: "filesystem metadata only before original Roster action; not an unindexed/OS-cold claim" });
  };
  if (archive) {
    const external = await open({ name: archive.name, body: archive.body, witness: archive.witness, provider: archive.provider,
      eventId: archive.eventId, conversation: archive.conversation, probe: "native-input-probe" });
    await external.session.driver.executeScript((id) => window.__chatAcceptance.bind(id), archive.sessionId);
    publishedState(archive.sessionId, "genuine-archive-saved-off-cold-process");
    const timings = [await measureChat(external.session.driver, archive.sessionId,
      "genuine-archive-saved-off-cold-process", archive.name, evidence, false)];
    await closeTerminalSurface(external.session.driver, archive.sessionId);
    timings.push(await measureChat(external.session.driver, archive.sessionId,
      "genuine-archive-saved-off-warm-reopen", archive.name, evidence, false));
    const actual = (await invokeTauri(external.session.driver, "list_agents"))
      .find((agent) => agent.session_id === archive.sessionId);
    assert.equal(actual?.is_off, true, "genuine Saved fixture resumed a provider");
    assert.equal(actual.provider, archive.provider);
    assert.equal(actual.conversation_logging, archive.policy, "source policy was overridden");
    const shell = await invokeTauri(external.session.driver, "load_shell_settings");
    evidence.genuineArchiveSaved = { ...archive.metadata, timings,
      agentPolicy: actual.conversation_logging, globalPolicy: shell.settings.conversation_logging,
      effectivePolicy: actual.conversation_logging === "default" ? shell.settings.conversation_logging : actual.conversation_logging,
      observation: await readObservation(external.session.driver) };
    verifySavedFixture(harness, archive);
    await closeTerminalSurface(external.session.driver, archive.sessionId);
    const externalDisposal = await (external.disposalPromise ??= disposeObservation(external.session.driver));
    evidence.disposalAcknowledgements[sessions.indexOf(external)] = { realm: sessions.indexOf(external), acknowledgement: externalDisposal };
    await external.session.close();
    external.closed = true;
  } else evidence.genuineArchiveSaved = "UNRUN: optional external manifest absent; small Mock Saved is a separate witness";
  const first = await open();
  const driver = first.session.driver;
  const agent = await invokeTauri(driver, "spawn_agent", { req: { sessionName: name, agentClass: "Coder", folder: workspace,
    isOff: false, configOverride: { provider: "mock", conversation_logging: "enabled" } } });
  const sessionId = agent.session_id ?? agent.sessionId;
  cleanupOwner.sessionId = sessionId;
  assert.ok(sessionId);
  await driver.executeScript((id) => window.__chatAcceptance.bind(id), sessionId);
  const producer = await waitForProducer(driver, harness, sessionId, fixture);
  publishedState(sessionId, "live-cold-process-first-chat");
  evidence.timings.push(await measureChat(driver, sessionId, "live-cold-process-first-chat", name, evidence));
  await closeTerminalSurface(driver, sessionId, true);
  evidence.timings.push(await measureChat(driver, sessionId, "live-same-process-warm-reopen", name, evidence));
  const surface = await ownedSurface(driver, sessionId);
  evidence.history = await exerciseHistory(driver, sessionId, fixture);

  // Preserve the original two tool-turn contracts, after all timing and lazy UI
  // claims. This legacy read is explicitly forbidden before the first Chat.
  const events = await invokeTauri(driver, "load_agent_chat_transcript", { sessionId });
  const edits = events.filter((event) => event?.metadata?.tool_name === "Edit");
  assert.ok(edits.length >= 2);
  assert.equal(edits[0].metadata.tool_input.old_string, "const WORK_GROUP_MIN_ENTRIES = 4;");
  assert.equal(edits[0].metadata.files_written[0], "src/features/chat/chatPresentation.ts");
  const write = events.find((event) => event?.metadata?.tool_name === "Write");
  assert.ok(write.metadata.tool_input.content.includes("Mock spec"));
  const card = await visible(driver, '[data-testid="turn-change-card"]', surface);
  const cardText = await card.getText();
  assert.match(cardText, /changed files?/);
  assert.match(cardText, /chatPresentation\.ts/);
  assert.doesNotMatch(cardText, /Created/);
  assert.ok((await surface.findElements(By.css('[data-testid="chat-work-group"]'))).length >= 1);
  const panel = await visible(driver, '[data-testid="tool-structured-edit"]', surface);
  assert.match(await panel.getText(), /Before\/after|New contents/);
  evidence.live = await readObservation(driver);
  const presentations = (await readObservation(driver)).presentations;
  const preview = presentations.find((item) => item.admission);
  const canonical = preview && presentations.find((item) => item.at > preview.at && item.admission === null);
  if (canonical) {
    assert.equal(preview.matchingBodySlots, 1);
    assert.equal(canonical.matchingBodySlots, 1, "preview was not retired to one canonical target slot");
    // Preview and canonical IDs deliberately differ; no alias is invented.
    // Each observation independently binds its actual response row to the DOM.
    const current = canonicalHistoryReceipts((await readObservation(driver)).receipts, sessionId)
      .find((receipt) => receipt.rows.some((row) => row.target && row.key === canonical.key));
    assert.ok(current, "visible canonical target must belong to the current durable cohort");
    assert.equal(canonical.generation, current.generation);
    assert.equal(canonical.conversation, current.conversation);
    assert.equal(canonical.epoch, current.epoch);
    evidence.transition = "observed visible preview retirement to one actual canonical target slot; IDs not equated";
  } else evidence.transition = "UNRUN: canonical-first or no observed visible handoff";

  // Save Terminal mode through normal input before the same-home restart. The
  // restart does not reset the home, reseed an archive, or resume the provider.
  await (await visible(driver, 'button[title="Switch to Terminal"]', surface)).click();
  await invokeTauri(driver, "pause_agent", { sessionId });
  paused = true;
  const liveDisposal = await (first.disposalPromise ??= disposeObservation(driver));
  evidence.disposalAcknowledgements[sessions.indexOf(first)] = { realm: sessions.indexOf(first), acknowledgement: liveDisposal };
  await first.session.close();
  first.closed = true;
  assert.ok(first.session.tauriDriver.exitCode !== null || first.session.tauriDriver.signalCode !== null,
    "original driver child did not join before same-home restart");
  const saved = await open();
  await saved.session.driver.executeScript((id) => window.__chatAcceptance.bind(id), sessionId);
  publishedState(sessionId, "small-mock-saved-off-cold-process");
  evidence.timings.push(await measureChat(saved.session.driver, sessionId, "small-mock-saved-off-cold-process", name, evidence, false));
  await closeTerminalSurface(saved.session.driver, sessionId);
  evidence.timings.push(await measureChat(saved.session.driver, sessionId, "small-mock-saved-off-warm-reopen", name, evidence, false));
  const agents = await invokeTauri(saved.session.driver, "list_agents");
  assert.equal(agents.find((entry) => entry.session_id === sessionId)?.is_off, true, "saved session resumed");
  const after = await waitForProducer(saved.session.driver, harness, sessionId, fixture);
  assert.deepEqual(after, producer, "saved-Off changed the original provider inode/content");
  evidence.saved = await readObservation(saved.session.driver);
  evidence.status = "OBSERVED_ENDPOINTS_CLOSURE_UNPROVEN";
  evidence.endpoints = "OBSERVED: owned normal UI endpoints; final qualification still requires positive caller closure receipts";
});
