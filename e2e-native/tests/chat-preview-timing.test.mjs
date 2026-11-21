// @tier nightly — App-free observer and evidence contracts; no native app or performance claim.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import vm from "node:vm";
import React from "react";
import Markdown from "react-markdown";
import { renderToStaticMarkup } from "react-dom/server";
import { JSDOM } from "jsdom";
import { randomUUID, createHash } from "node:crypto";
import { acquireHomeLock } from "../lib/sessionHome.mjs";
import {
  CHAT_PREVIEW_BUDGET_MS, CHAT_PREVIEW_SEED_TIMEOUT_MS, CHAT_PREVIEW_RULER_VERSION, chatPreviewWatchdogMs,
  syntheticCodexSource, stageSyntheticStoppedSource, verifySyntheticSource, pendingCaptureWitness, seedNativePreviewSource,
  assertPendingCaptureBracket, trustedClickWithPendingCapture,
  chatTimingReceipt, chatTimingNativeEvidence, chatTimingCandidateDetails, chatPreviewObserverScript, assertChatTimingSample, recordChatTimingCase, writeChatTimingReport,
  recordRejectedChatScope, driveChatPreviewProbe, assertChatOpeningSample, chatTimingHandoffEvidence, recordChatContinuity, assertChatFixtureShape,
} from "../lib/chatPreviewTiming.mjs";

const fixture = { sessionId: "owned-agent", provider: "codex", body: "A uniquely completed synthetic answer.", name: "Synthetic Agent",
  synthetic: true, sourceKey: "native-source", sourceEpoch: "source-epoch", targetTurn: "synthetic-completed-unit", targetMessageId: "answer-128" };
const v8Probe = "synthetic-unsent-timing-probe";

test("setup watchdog adds one 180-second preparation allowance per surface/mode without changing the UI budget", () => {
  assert.equal(CHAT_PREVIEW_SEED_TIMEOUT_MS, 180_000); assert.equal(CHAT_PREVIEW_BUDGET_MS, 1_000);
  assert.equal(chatPreviewWatchdogMs(1), 840_000); assert.equal(chatPreviewWatchdogMs(2), 1_200_000);
  assert.equal(chatPreviewWatchdogMs(1) - 2 * 180_000, 480_000);
  assert.equal(chatPreviewWatchdogMs(2) - 4 * 180_000, 480_000);
  for (const modes of [0, 3, 1.5]) assert.throws(() => chatPreviewWatchdogMs(modes), { code: "invalid_watchdog_fixture_modes" });
});

function page({ preview = false, provider = "codex", complete = true, body = fixture.body } = {}) {
  return { session_id: fixture.sessionId, conversation_id: preview ? null : "conversation", generation: preview ? null : "generation",
    source_epoch: preview ? null : fixture.sourceEpoch, revision: preview ? "recent-revision" : "canonical-revision", recent_admission: preview ? "admission" : null,
    progress: preview ? "provisional" : "ready", events: [{ id: preview ? "preview-id" : "canonical-id", session_id: fixture.sessionId,
      provider, kind: "message", role: "assistant", text: body,
      metadata: preview ? {} : { chat_source_ref: "canonical-record", chat_source_epoch: fixture.sourceEpoch } }],
    presentation: preview ? null : { version: 1, payload: { turns: [{ turn_key: "committed-turn", membership_complete: complete, status: "completed" }],
      items: [{ item_key: "canonical-answer", turn_key: "committed-turn", kind: "answer", provenance: "canonical",
        evidence: { kind: "source", source_key: fixture.sourceKey, source_epoch: fixture.sourceEpoch, record_key: "canonical-record" }, event_refs: ["canonical-id"] }] } } };
}

function presentedPreview() {
  const dto = page({ preview: true });
  Object.assign(dto, { conversation_id: "conversation", generation: "generation", source_epoch: "source-epoch" });
  dto.events[0].metadata = { chat_provisional: true, chat_source_ref: "source-record", chat_source_epoch: "source-epoch" };
  dto.presentation = { version: 1, payload: {
    preview_bridge: { canonical_head: "a".repeat(64), admission_id: dto.recent_admission },
    turns: [{ turn_key: "presented-turn", membership_complete: true, status: "completed" }],
    items: [{ item_key: "presented-answer", turn_key: "presented-turn", kind: "answer", provenance: "provisional",
      admission_id: dto.recent_admission,
      evidence: { kind: "source", source_key: "native-source", source_epoch: "source-epoch", record_key: "source-record" },
      event_refs: [dto.events[0].id] }],
  } };
  return dto;
}

function unscopedReset() {
  return { session_id: fixture.sessionId, conversation_id: null, generation: null, source_epoch: null,
    revision: "opaque-unscoped-reset", events: [], next_before: null, unchanged: false, reset: true,
    progress: "projection_pending", aliases: [], removed_ids: [], detail: null, bytes_read: 0, records_decoded: 0 };
}

function nativeLegacyPage({ id = "producer literal / ? λ", sourceRef = "different physical source reference", sequence = 17 } = {}) {
  return { ...unscopedReset(), reset: false, source_epoch: fixture.sourceEpoch, revision: "opaque-native-revision",
    progress: "provisional", events: [{ ...page({ preview: true }).events[0], id, sequence,
      metadata: { chat_source_ref: sourceRef, chat_source_epoch: fixture.sourceEpoch } }] };
}

function canonicalNativePage() {
  const dto = presentedPreview(); dto.events[0].metadata.chat_provisional = false;
  dto.presentation.payload.items[0].provenance = "canonical";
  return dto;
}

test("v8 native opening evidence preserves each producer's literal IDs without ordinal or DOM identity inference", () => {
  assert.equal(CHAT_PREVIEW_RULER_VERSION, 8);
  for (const dto of [nativeLegacyPage(), nativeLegacyPage({ id: "other producer literal", sequence: 38712743 })]) {
    const receipt = chatTimingReceipt(dto, fixture);
    assert.equal(receipt.nativeOpening.verified, true);
    assert.deepEqual(receipt.nativeOpening.backing_ids, [dto.events[0].id]);
    assert.deepEqual(receipt.nativeOpening.source_refs, [dto.events[0].metadata.chat_source_ref]);
    assert.equal(receipt.nativeOpening.dom_identity_binding, "NOT_CLAIMED");
    assert.equal(receipt.nativeOpening.ordinal_or_canonical_key_inference, false);
    assert.equal(receipt.targets[0].qualifiedTurn, null);
    assert.equal(receipt.targets[0].presentedAnswer, null);
    assert.equal(receipt.scope.admission, null);
  }
});

test("v8 native opening rejects foreign, optimistic, missing-source and mismatched body evidence", () => {
  const cases = {
    foreignSession: (p) => { p.events[0].session_id = "foreign"; },
    foreignProvider: (p) => { p.events[0].provider = "mock"; },
    optimistic: (p) => { p.events[0].metadata.optimistic = true; },
    noId: (p) => { p.events[0].id = ""; },
    longId: (p) => { p.events[0].id = "😀".repeat(129); },
    noSource: (p) => { delete p.events[0].metadata.chat_source_ref; },
    sourceEpoch: (p) => { p.events[0].metadata.chat_source_epoch = "other"; },
    body: (p) => { p.events[0].text = "different body"; },
  };
  for (const [name, mutate] of Object.entries(cases)) {
    const dto = nativeLegacyPage(); mutate(dto);
    const proof = chatTimingNativeEvidence(dto, fixture);
    assert.equal(proof.verified, false, name); assert.deepEqual(proof.backing_ids, [], name);
  }
  assert.equal(chatTimingNativeEvidence(unscopedReset(), fixture).verified, false);
  assert.equal(chatTimingNativeEvidence({ ...nativeLegacyPage(), events: [] }, fixture).verified, false);
});

test("v8 source-legacy admission cannot conceal a partial scope or claimed presentation", () => {
  for (const change of [{ conversation_id: "partial" }, { generation: "partial" }, { presentation: { version: 1, payload: {} } }]) {
    assert.throws(() => chatTimingReceipt({ ...nativeLegacyPage(), ...change }, fixture), /invalid_page_scope/);
  }
});

function browser({ surface = "remote", disclosure = false, modeNeedsToggle = false, body = fixture.body, captureDetails = false, rulerVersion = 7 } = {}) {
  const observedFixture = { ...fixture, body };
  let time = 10; let ordinal = 0;
  const frames = new Map(); const listeners = new Map();
  const rect = { top: 0, left: 0, bottom: 400, right: 400, width: 400, height: 400 };
  const element = (attributes = {}) => ({ isConnected: true, hidden: false, inert: false, parentElement: null,
    attributes, getAttribute(key) { return this.attributes[key] ?? null; }, getBoundingClientRect: () => rect });
  const host = element({ "data-agent-grid-card-id": fixture.sessionId }); const scroll = element(); const row = element(); const content = element(); const committed = element(); const anchor = element();
  const panel = element({ "data-surface-id": "overview" }); const workbench = element();
  const tab = element({ id: "tab", "aria-controls": "tabpanel", "data-surface-id": "overview" });
  const tabpanel = element({ "aria-labelledby": "tab" });
  host.closest = () => panel; panel.closest = () => workbench; workbench.querySelectorAll = () => [tab];
  const input = { ...element(), value: "", disabled: false, readOnly: false };
  const workButton = element({ "aria-expanded": "false" }); workButton.closest = () => workButton;
  const modeButton = element({ title: modeNeedsToggle ? "Switch to Chat" : "Switch to Terminal" }); modeButton.closest = () => modeButton;
  host.contains = (node) => [input, workButton, scroll, row, content].includes(node);
  host.querySelector = (selector) => selector === "h1" ? { textContent: fixture.name } : selector === ".chat-transcript-scroll" ? scroll : null;
  host.querySelectorAll = (selector) => selector === ".chat-transcript-scroll" ? [scroll] : selector === "textarea" ? [input]
    : selector.includes("Switch to Chat") ? [modeButton] : (selector.includes("chat-turn-work") || selector.includes("data-chat-anchor-keys")) && disclosure ? [workButton] : [];
  scroll.scrollTop = 50; scroll.scrollHeight = 1_000; scroll.clientHeight = 400;
  const transcriptAlert = element(); transcriptAlert.hidden = true;
  scroll.querySelectorAll = (selector) => selector === ':scope > [role="alert"]' ? [transcriptAlert]
    : selector === "[data-chat-row-identity]" ? [row] : [content];
  scroll.contains = (value) => value === content;
  content.textContent = body;
  Object.defineProperty(content, "outerHTML", { get: () => `<div class="chat-message-content">${content.textContent}</div>` });
  content.contains = (node) => node === content;
  let shared = false;
  row.querySelector = () => shared ? committed : null;
  content.closest = (selector) => selector === "[data-chat-row-key]" ? row
    : selector === "[data-chat-anchor-key]" ? shared ? anchor : null
      : selector === '[data-testid="chat-committed-turn"]' && shared ? committed : null;
  committed.attributes["data-turn-key"] = "committed-turn";
  const document = { visibilityState: "visible", activeElement: null, querySelectorAll: () => [host], getElementById: () => tabpanel,
    createRange: () => ({ selectNodeContents() {}, getClientRects: () => [rect] }),
    addEventListener(name, handler) { if (!listeners.has(name)) listeners.set(name, new Set()); listeners.get(name).add(handler); },
    removeEventListener(name, handler) { listeners.get(name)?.delete(handler); if (!listeners.get(name)?.size) listeners.delete(name); } };
  let callback; let active = false; let requests = 0;
  const window = { __wardianChatPageObservation: { version: 1, subscribe({ onRequest }) {
    callback = onRequest; active = true;
    return { active: true, status: () => ({ active, fault_count: 0, request_count: requests }), unsubscribe() { active = false; } };
  } } };
  const context = vm.createContext({ window, document, innerWidth: 500, innerHeight: 500, performance: { now: () => time },
    getComputedStyle: () => ({ display: "block", visibility: "visible", opacity: "1" }),
    requestAnimationFrame: (callback) => { frames.set(++ordinal, callback); return ordinal; },
    cancelAnimationFrame: (id) => frames.delete(id), setTimeout, clearTimeout, TextEncoder });
  vm.runInContext(chatPreviewObserverScript({ ...observedFixture, surface, probe: rulerVersion === 8 ? v8Probe : "probe", captureDetails, rulerVersion }), context);
  const api = window.__chatPreviewTiming;
  const adopt = (dto, marker = true, rendered = dto) => {
    const receipt = chatTimingReceipt(dto, observedFixture);
    // Both provisional and canonical presentations use the real shared renderer.
    shared = Boolean(rendered.presentation?.payload?.turns.length);
    scroll.attributes["data-chat-page-scope"] = marker ? JSON.stringify(receipt.scope) : undefined;
    row.attributes["data-chat-row-key"] = shared ? rendered.presentation.payload.turns[0].turn_key : rendered.events[0]?.id;
    row.attributes["data-chat-row-identity"] = shared ? undefined : JSON.stringify({ id: rendered.events[0]?.id });
    workButton.attributes["data-chat-anchor-keys"] = JSON.stringify(rendered.events.map((event) => event.id));
    committed.attributes["data-turn-key"] = row.attributes["data-chat-row-key"];
    anchor.attributes["data-chat-anchor-key"] = rendered.events[0].metadata.chat_display_key ?? rendered.events[0].id;
    const request = rulerVersion === 8 ? api.noteRequest({ sessionId: fixture.sessionId }) : undefined;
    api.receive(receipt, captureDetails ? chatTimingCandidateDetails(dto, observedFixture) : null, request);
  };
  return { api, adopt, input, content, document, listeners, tab, scroll, workButton, row, committed, anchor, transcriptAlert,
    async deliver(dto, args = { sessionId: fixture.sessionId }) { requests++; await callback(args, Promise.resolve(dto)); },
    async reject(error) { requests++; await callback({ sessionId: fixture.sessionId }, Promise.reject(error)); },
    observe(promise, args = { sessionId: fixture.sessionId }) { requests++; return callback(args, promise); },
    click(trusted = true) {
      const button = element({ "aria-label": surface === "desktop" ? `Agent ${fixture.name}` : `Open ${fixture.name} details` }); button.closest = () => button;
      for (const handler of listeners.get("click")) handler({ isTrusted: trusted, target: button });
    },
    normalModeToggle() {
      for (const handler of listeners.get("click")) handler({ isTrusted: true, target: modeButton });
      modeButton.attributes.title = "Switch to Terminal";
    },
    wheelEvent(trusted = true, deltaY) { for (const handler of listeners.get("wheel")) handler({ isTrusted: trusted, target: scroll, deltaY }); },
    inputEvent(trusted = true) { for (const handler of listeners.get("input")) handler({ isTrusted: trusted, target: input }); },
    expandWork() {
      const plan = api.prepareExpansion(); assert.ok(plan, "actual provisional/legacy work disclosure required");
      for (const handler of listeners.get("click")) handler({ isTrusted: true, target: workButton });
      workButton.attributes["aria-expanded"] = workButton.attributes["aria-expanded"] === "true" ? "false" : "true";
    },
    elapse(ms) { time += ms; },
    advance(count = 1) {
      for (let index = 0; index < count; index++) {
        time += 16; const current = [...frames.values()]; frames.clear(); for (const callback of current) callback();
      }
    },
  };
}

function v8Opening(b, dto = nativeLegacyPage(), marker = false) {
  b.api.arm("v8-opening"); b.click(); b.adopt(dto, marker); b.advance(3);
  const plan = b.api.prepareInteraction();
  assert.equal(plan?.kind, "input"); assert.equal(v8Probe.length, 29); b.input.value = v8Probe; b.document.activeElement = b.input;
  b.inputEvent(); b.advance(3);
  return b.api.snapshot();
}

test("v8 common M1 has literal native wire evidence plus separate visible content without Remote DOM markers or M2 credit", () => {
  const b = browser({ rulerVersion: 8 });
  const snapshot = v8Opening(b);
  const result = assertChatOpeningSample(snapshot, { sourceRole: "baseline" });
  assert.ok(result.m1.usable_ms < 1_000); assert.equal(result.m1.dom_identity_binding, "NOT_CLAIMED");
  assert.equal(result.m2.status, "N/A_LEGACY_UNSUPPORTED");
  assert.equal(snapshot.phase.projectionAt, null); assert.equal(snapshot.phase.qualifiedAt, null);
  assert.throws(() => assertChatOpeningSample(snapshot), /missing_qualified_handoff/);
  b.api.dispose();
});

test("v8 disabled composer never falls back to wheel; API-only, reset and unrelated visible content cannot start M1", () => {
  const b = browser({ rulerVersion: 8 }); b.api.arm("no-credit"); b.click();
  b.adopt(nativeLegacyPage(), false); b.content.textContent = "unrelated"; b.advance(4);
  assert.equal(b.api.snapshot().phase.readableAt, null);
  b.content.textContent = fixture.body; b.advance(3); b.input.disabled = true;
  assert.equal(b.api.prepareInteraction(), null); b.wheelEvent(); b.scroll.scrollTop += 10; b.advance(4);
  assert.equal(b.api.snapshot().phase.usableAt, null);
  b.api.receive(chatTimingReceipt(unscopedReset(), fixture)); b.advance(3);
  assert.equal(b.api.snapshot().health.current_native, false); b.api.dispose();
});

test("v8 a control or changed source between M1 frames fences old evidence and requires fresh producer and input frames", () => {
  const b = browser({ rulerVersion: 8 }); b.api.arm("fenced"); b.click();
  b.adopt(nativeLegacyPage(), false); b.advance(2);
  b.api.receive(chatTimingReceipt(unscopedReset(), fixture)); b.advance(3);
  assert.equal(b.api.snapshot().phase.readableAt, null);
  b.adopt(nativeLegacyPage({ id: "fresh literal source ID" }), false); b.advance(3);
  const plan = b.api.prepareInteraction(); assert.equal(plan.kind, "input");
  b.input.value = v8Probe; b.document.activeElement = b.input; b.inputEvent(); b.advance(1);
  b.adopt(nativeLegacyPage({ id: "replacement literal ID" }), false); b.advance(3);
  assert.equal(b.api.snapshot().phase.usableAt, null); b.api.dispose();
});

test("v8 M2 separately requires admitted item/turn/source DOM proofs; M1 never qualifies a provisional answer as canonical", () => {
  const b = browser({ rulerVersion: 8, disclosure: true });
  const first = v8Opening(b, presentedPreview(), true);
  const result = assertChatOpeningSample(first, { requireStableHandoff: false });
  assert.equal(result.m2.status, "QUALIFIED_PRESENTATION"); assert.equal(result.m2.provenance, "provisional");
  assert.equal(result.m2.canonical_qualified_ms, null); assert.equal(first.phase.qualifiedAt, null);
  assert.throws(() => assertChatOpeningSample(first, { requireCanonical: true, requireStableHandoff: false }), /missing_canonical_qualification/);
  assert.throws(() => assertChatOpeningSample(first), /unproven_handoff_continuity/);
  b.expandWork(); b.advance(3);
  b.adopt(canonicalNativePage(), true); b.advance(3);
  assert.equal(assertChatOpeningSample(b.api.snapshot(), { requireCanonical: true }).m2.status, "QUALIFIED_HANDOFF");
  assert.equal(b.api.snapshot().phase.continuity.source_switches, 0);
  assert.equal(b.api.snapshot().phase.continuity.in_place_replacements, 1);
  assert.equal(b.api.snapshot().phase.usableAt, first.phase.usableAt); b.api.dispose();
});

test("v8 M2 rejects wrong scope/key/ref/evidence, ambiguous answer and missing/wrong bridge or item admission", () => {
  const mutations = [
    (d) => { d.presentation.payload.items[0].event_refs = ["foreign"]; },
    (d) => { d.presentation.payload.items[0].evidence.record_key = "foreign"; },
    (d) => { d.presentation.payload.items[0].evidence.source_key = "foreign"; },
    (d) => { d.presentation.payload.items.push(structuredClone(d.presentation.payload.items[0])); },
    (d) => { delete d.presentation.payload.preview_bridge; },
    (d) => { d.presentation.payload.preview_bridge.admission_id = "foreign"; },
    (d) => { delete d.presentation.payload.items[0].admission_id; },
    (d) => { d.presentation.payload.items[0].admission_id = "foreign"; },
  ];
  for (const mutate of mutations) {
    const b = browser({ rulerVersion: 8 }); const dto = presentedPreview(); mutate(dto);
    const snapshot = v8Opening(b, dto, true);
    assert.equal(snapshot.phase.projectionAt, null); assert.equal(snapshot.phase.qualifiedAt, null);
    assert.throws(() => assertChatOpeningSample(snapshot), /missing_qualified_handoff/); b.api.dispose();
  }
  for (const corrupt of [
    (b) => { b.anchor.attributes["data-chat-anchor-key"] = "foreign"; },
    (b) => { b.row.attributes["data-chat-row-key"] = "foreign"; },
    (b) => { b.scroll.attributes["data-chat-page-scope"] = JSON.stringify({ ...chatTimingReceipt(presentedPreview(), fixture).scope, revision: "stale" }); },
  ]) {
    const b = browser({ rulerVersion: 8 }); b.api.arm("bad-dom"); b.click(); b.adopt(presentedPreview(), true); b.advance(2);
    corrupt(b); b.advance(3); assert.equal(b.api.snapshot().phase.projectionAt, null); b.api.dispose();
  }
});

test("v8 canonical M2 binds the literal answer anchor and cannot borrow an item changed between frame witnesses", () => {
  const b = browser({ rulerVersion: 8 }); const legacy = presentedPreview(); legacy.presentation = null;
  v8Opening(b, legacy, true);
  b.adopt(canonicalNativePage(), true); b.advance(2); b.anchor.attributes["data-chat-anchor-key"] = "wrong-answer"; b.advance();
  assert.equal(b.api.snapshot().phase.projectionAt, null); assert.equal(b.api.snapshot().phase.qualifiedAt, null);
  b.adopt(canonicalNativePage(), true); b.advance(2);
  const changed = canonicalNativePage(); changed.presentation.payload.items[0].item_key = "new-literal-item";
  b.adopt(changed, true); b.advance(1);
  assert.equal(b.api.snapshot().phase.projectionAt, null);
  b.advance(3); assert.ok(b.api.snapshot().phase.projectionAt); b.api.dispose();
});

test("v8 review canonical evidence rejects each missing or foreign source component", () => {
  for (const field of ["source_key", "source_epoch", "record_key"]) {
    for (const value of [undefined, "foreign"]) {
      const dto = canonicalNativePage();
      if (value === undefined) delete dto.presentation.payload.items[0].evidence[field];
      else dto.presentation.payload.items[0].evidence[field] = value;
      assert.equal(chatTimingReceipt(dto, fixture).targets[0].qualifiedAnswer, null, `${field}:${value}`);
    }
  }
  for (const provisional of [false, true]) {
    const dto = provisional ? presentedPreview() : canonicalNativePage();
    dto.source_epoch = "self-consistent-foreign"; dto.events[0].metadata.chat_source_epoch = dto.source_epoch;
    dto.presentation.payload.items[0].evidence.source_epoch = dto.source_epoch;
    const target = chatTimingReceipt(dto, fixture).targets[0];
    assert.equal(target.qualifiedTurn, null); assert.equal(target.qualifiedAnswer, null); assert.equal(target.presentedAnswer, null);
  }
  assert.equal(chatTimingReceipt(canonicalNativePage(), { ...fixture, sourceEpoch: undefined }).targets[0].qualifiedAnswer, null);
  for (const returnedId of ["canonical-id", "display:logical-returned-id"]) {
    const missingPhysical = canonicalNativePage(); delete missingPhysical.events[0].metadata.chat_source_ref;
    missingPhysical.events[0].id = returnedId;
    missingPhysical.presentation.payload.items[0].event_refs = [returnedId];
    missingPhysical.presentation.payload.items[0].evidence.record_key = returnedId;
    const rejected = chatTimingReceipt(missingPhysical, fixture).targets[0];
    assert.equal(rejected.qualifiedTurn, null, "returned ID is not physical-owner evidence");
    assert.equal(rejected.qualifiedAnswer, null);
  }
  const certified = canonicalNativePage(); delete certified.events[0].metadata.chat_source_ref;
  certified.events[0].metadata.chat_display_member_ids = ["literal-physical-owner", "other-physical-member"];
  certified.presentation.payload.items[0].evidence.record_key = "literal-physical-owner";
  assert.ok(chatTimingReceipt(certified, fixture).targets[0].qualifiedAnswer, "explicit producer physical-member certificate");
  delete certified.events[0].metadata.chat_display_member_ids;
  certified.events[0].metadata.chat_display_physical_id = "literal-physical-owner";
  assert.ok(chatTimingReceipt(certified, fixture).targets[0].qualifiedAnswer, "explicit producer physical identity");
});

test("v8 review merged canonical/provisional answer evidence uses the literal producer member certificate, not the display ID", () => {
  for (const provisional of [false, true]) {
    const dto = provisional ? presentedPreview() : canonicalNativePage();
    dto.events[0].id = "display:literal-mirror";
    dto.events[0].metadata.chat_source_ref = "physical-second";
    dto.events[0].metadata.chat_display_member_ids = ["physical-first", "physical-second"];
    dto.presentation.payload.items[0].event_refs = [dto.events[0].id];
    dto.presentation.payload.items[0].evidence.record_key = "physical-first";
    const target = chatTimingReceipt(dto, fixture).targets[0];
    assert.ok(provisional ? target.presentedAnswer : target.qualifiedAnswer);
    for (const members of [undefined, [], ["physical-second"], ["foreign-first", "physical-second"], ["physical-first", "physical-first"]]) {
      const bad = structuredClone(dto); bad.events[0].metadata.chat_display_member_ids = members;
      const rejected = chatTimingReceipt(bad, fixture).targets[0];
      assert.equal(provisional ? rejected.presentedAnswer : rejected.qualifiedAnswer, null);
    }
  }
});

test("v8 review shared provisional promotion is one in-place replacement, independently of the one Legacy source switch", () => {
  const state = { samples: [], raw_frames: 0, coalesced_frames: 0, blank_frames: 0, mixed_frames: 0,
    source_switches: 0, in_place_replacements: 0, invalid_replacements: 0, extra_reflows: 0, overflow_count: 0,
    disclosure_requested: true, disclosure_confirmed: true, disclosure_frames: 2, expected_expanded: "true" };
  const base = { at: 1, content_count: 1, renderer: "shared", qualified: true, mixed_source_rows: 0,
    anchor_bound: true, anchor_top: 10, geometry: "stable", expansion_bound: true, expanded: "true",
    representation: { kind: "provisional", item_key: "literal-item", turn_key: "literal-turn",
      source_key: "native-source", source_epoch: "source-epoch", record_key: "source-record" } };
  recordChatContinuity(state, base);
  recordChatContinuity(state, { ...base, at: 2, representation: { ...base.representation, kind: "canonical" } });
  assert.equal(state.source_switches, 0); assert.equal(state.in_place_replacements, 1);
  assert.equal(state.anchor_preserved, true); assert.equal(state.expansion_preserved, true);
  recordChatContinuity(state, { ...base, at: 3 });
  assert.ok(state.invalid_replacements > 0, "canonical-to-provisional regression must fail");
});

test("v8 review Legacy handoff plus shared promotion permits one of each with preserved literal anchor and work expansion", () => {
  const b = browser({ rulerVersion: 8, disclosure: true }); const legacy = presentedPreview(); legacy.presentation = null;
  v8Opening(b, legacy, true); b.expandWork(); b.advance(3);
  b.adopt(presentedPreview(), true); b.advance(3);
  b.adopt(canonicalNativePage(), true); b.advance(3);
  const result = assertChatOpeningSample(b.api.snapshot(), { requireCanonical: true });
  assert.equal(result.m2.source_switches, 1); assert.equal(result.m2.in_place_replacements, 1);
  assert.equal(result.m2.continuity, "PROVED"); b.api.dispose();
});

test("v8 review shared promotion rejects missing expansion, lost anchor, changed item and repeated/regressing provenance", () => {
  for (const defect of ["no-expansion", "lost-anchor", "changed-item", "repeat"]) {
    const b = browser({ rulerVersion: 8, disclosure: true }); v8Opening(b, presentedPreview(), true);
    if (defect !== "no-expansion") { b.expandWork(); b.advance(3); }
    const canonical = canonicalNativePage();
    if (defect === "lost-anchor") b.content.getBoundingClientRect = () => ({ top: 20, left: 0, bottom: 420, right: 400, width: 400, height: 400 });
    if (defect === "changed-item") canonical.presentation.payload.items[0].item_key = "other-literal-item";
    b.adopt(canonical, true); b.advance(3);
    if (defect === "repeat") { b.adopt(presentedPreview(), true); b.advance(3); b.adopt(canonical, true); b.advance(3); }
    assert.throws(() => assertChatOpeningSample(b.api.snapshot(), { requireCanonical: true }), /unproven_handoff_continuity/);
    b.api.dispose();
  }
});

test("v8 review ordinary M1 miss retains null metrics and observes independent M2 at ten seconds", async () => {
  const timing = await import("../lib/chatPreviewTiming.mjs");
  const b = browser({ rulerVersion: 8 }); const record = { metrics: null };
  b.api.arm("m1-miss"); b.click(); b.input.disabled = true;
  const miss = Object.assign(new Error("readable/probe deadline"), { code: "probe_deadline_exceeded" });
  assert.equal(typeof timing.observeChatOpening, "function");
  await assert.rejects(timing.observeChatOpening({ record, options: { sourceRole: "candidate", requireCanonical: true },
    measureM1: async () => { b.elapse(5_000); throw miss; },
    observeM2: async () => { b.elapse(5_000); b.adopt(canonicalNativePage(), true); b.advance(6); },
    snapshot: async () => b.api.snapshot(),
  }), (error) => error === miss);
  assert.equal(record.metrics, null); assert.equal(record.m2_observed.status, "QUALIFIED_FIRST_FRAME");
  assert.ok(record.m2_observed.open_to_qualified_handoff_ms >= 10_000);
  b.api.dispose();
});

test("v8 review fatal authority errors do not enter the passive continuation and M2 after the original 30 seconds receives no credit", async () => {
  const timing = await import("../lib/chatPreviewTiming.mjs");
  const b = browser({ rulerVersion: 8 }); b.api.arm("fatal"); b.click();
  const fatal = new Error("invalid_page_scope"); let observed = false;
  await assert.rejects(timing.observeChatOpening({ record: {}, options: {}, measureM1: async () => { throw fatal; },
    observeM2: async () => { observed = true; }, snapshot: async () => b.api.snapshot(),
  }), (error) => error === fatal);
  assert.equal(observed, false);
  b.elapse(30_001); b.adopt(canonicalNativePage(), true); b.advance(6);
  const snapshot = b.api.snapshot(); assert.equal(snapshot.phase.projectionAt, null); assert.equal(snapshot.phase.qualifiedAt, null);
  assert.throws(() => timing.assertChatHandoffSample(snapshot), /missing_qualified_handoff/);
  await b.api.finishObservation(); b.api.dispose();
  await assert.rejects(b.api.finishObservation(), /timing observation invalidated/);
});

test("v8 old-phase wire replies and a request begun before the click cannot credit otherwise identical visible content", () => {
  const b = browser({ rulerVersion: 8 }); b.api.arm("old-phase");
  const old = b.api.noteRequest(); b.advance(); b.click();
  b.api.receive(chatTimingReceipt(nativeLegacyPage(), fixture), null, old); b.advance(4);
  assert.equal(b.api.snapshot().phase.readableAt, null);
  assert.equal(b.api.snapshot().prewarmed_request_observed, true); b.api.dispose();
});

test("v8 historical first endpoints cannot bypass a reset fence before fresh current health frames", () => {
  const b = browser({ rulerVersion: 8 }); const first = v8Opening(b);
  b.api.receive(chatTimingReceipt(unscopedReset(), fixture)); b.advance(3);
  b.adopt(nativeLegacyPage(), false);
  assert.throws(() => assertChatOpeningSample(b.api.snapshot(), { sourceRole: "baseline" }), /missing_fresh_post_control_health/);
  b.advance(2);
  assert.throws(() => assertChatOpeningSample(b.api.snapshot(), { sourceRole: "baseline" }), /missing_fresh_post_control_health/);
  b.advance();
  assert.equal(assertChatOpeningSample(b.api.snapshot(), { sourceRole: "baseline" }).m1.usable_ms, first.phase.usableAt - first.phase.clickAt);
  b.api.dispose();
});

test("v8 older/detail responses and duplicate literal IDs grant neither opening nor handoff credit", () => {
  for (const purpose of ["older", "detail"]) {
    const b = browser({ rulerVersion: 8 }); b.api.arm("non-opening-request"); b.click();
    const request = b.api.noteRequest({ purpose });
    b.api.receive(chatTimingReceipt(nativeLegacyPage(), fixture), null, request); b.advance(4);
    assert.equal(b.api.snapshot().phase.readableAt, null); b.api.dispose();
  }
  const duplicate = nativeLegacyPage(); duplicate.events.push(structuredClone(duplicate.events[0]));
  assert.equal(chatTimingNativeEvidence(duplicate, fixture).verified, false);
});

test("v8 first healthy M1 survives a later busy fact but final acceptance needs fresh post-rejection receipt and clear two-frame health", async () => {
  const b = browser({ rulerVersion: 8, surface: "desktop" });
  const first = v8Opening(b, nativeLegacyPage(), false);
  await b.reject(new Error("chat source temporarily busy")); b.transcriptAlert.hidden = false; b.advance(3);
  assert.throws(() => assertChatOpeningSample(b.api.snapshot(), { sourceRole: "baseline" }), /unhealthy_opening/);
  b.adopt(nativeLegacyPage(), false); b.advance(3);
  assert.equal(b.api.snapshot().busy_retries[0].recovered_at, null);
  b.transcriptAlert.hidden = true; b.advance(1);
  assert.equal(b.api.snapshot().busy_retries[0].recovered_at, null);
  b.advance(3);
  const recovered = b.api.snapshot();
  assert.equal(recovered.phase.usableAt, first.phase.usableAt);
  assert.equal(assertChatOpeningSample(recovered, { sourceRole: "baseline" }).m1.usable_ms, first.phase.usableAt - first.phase.clickAt);
  b.api.dispose();
});

test("v8 whole-window handoff uses all 80 producer aliases, never the latest-eight hint subset", () => {
  const before = chatTimingReceipt(nativeLegacyPage(), fixture);
  before.nativeOpening.window_ids = Array.from({ length: 80 }, (_, index) => `literal-${index}`);
  before.scope.admission = "actual-admission";
  const after = structuredClone(before);
  after.nativeOpening.window_ids = before.nativeOpening.window_ids.map((id) => `published-${id}`);
  after.aliases = before.nativeOpening.window_ids.map((id, index) => ({ observation_id: id, canonical_id: after.nativeOpening.window_ids[index] }));
  assert.equal(chatTimingHandoffEvidence(before, after).verified, true);
  for (const mutate of [
    (p) => { p.aliases = p.aliases.slice(-8); },
    (p) => { p.nativeOpening.window_ids.reverse(); },
    (p) => { p.scope.epoch = "foreign"; },
    (p) => { p.scope.admission = "other admission"; },
    (p) => { p.aliases.push({ observation_id: "literal-0", canonical_id: "ambiguous" }); },
  ]) { const wrong = structuredClone(after); mutate(wrong); assert.equal(chatTimingHandoffEvidence(before, wrong).verified, false); }
});

test("v8 continuity retains one switch, expansion and anchor with bounded duplicate frames and rejects blank/mixed/reflow", () => {
  const state = { raw_frames: 0, blank_frames: 0, mixed_frames: 0, source_switches: 0, extra_reflows: 0,
    samples: [], coalesced_frames: 0, overflow_count: 0, disclosure_requested: true, disclosure_frames: 0, expected_expanded: "true" };
  const legacy = { at: 1, content_count: 1, renderer: "legacy", mixed_source_rows: 0, anchor_bound: true,
    anchor_top: 10, geometry: "original", expanded: "true", expansion_bound: true, representation: { kind: "legacy" } };
  for (let at = 1; at <= 1_000; at++) recordChatContinuity(state, { ...legacy, at });
  const representation = { kind: "provisional", item_key: "item", turn_key: "turn", source_key: "source", source_epoch: "epoch", record_key: "record" };
  recordChatContinuity(state, { ...legacy, at: 1_001, renderer: "shared", qualified: true, representation });
  assert.equal(state.samples.length, 2); assert.equal(state.coalesced_frames, 999);
  assert.equal(state.source_switches, 1); assert.equal(state.anchor_preserved, true); assert.equal(state.expansion_preserved, true);
  recordChatContinuity(state, { ...legacy, at: 1_002, renderer: "shared", representation, geometry: "changed", content_count: 0, mixed_source_rows: 1 });
  assert.equal(state.blank_frames, 1); assert.equal(state.mixed_frames, 1); assert.equal(state.extra_reflows, 1);
  for (let at = 2_000; at < 2_100; at++) recordChatContinuity(state, { ...legacy, at, geometry: String(at) });
  assert.ok(state.samples.length <= 64); assert.ok(state.bytes <= 16_384); assert.ok(state.overflow_count > 0);
});

test("v8 fixture shape catches per-turn workspace length bias before clock without normalizing literal source hashes", () => {
  const a = syntheticCodexSource({ workspace: path.resolve("fixed-source-A"), turns: 128, padding: 128 });
  const b = syntheticCodexSource({ workspace: path.resolve("fixed-source-B"), turns: 128, padding: 128 });
  const expected = { generation_sha256: a.generationSha256, source_bytes: a.bytes.length };
  assert.deepEqual(a.generation, b.generation); assert.notEqual(a.sourceSha256, b.sourceSha256);
  assertChatFixtureShape({ ...b, sourceBytes: b.bytes.length }, expected);
  const longer = syntheticCodexSource({ workspace: path.resolve("longer-fixed-source"), turns: 128, padding: 128 });
  assert.throws(() => assertChatFixtureShape({ ...longer, sourceBytes: longer.bytes.length }, expected), /fixture_generation_mismatch_before_clock/);
});

test("raw synthetic Codex source has explicit request, native turn bracket and successful final output", () => {
  const source = syntheticCodexSource({ workspace: path.resolve("synthetic-workspace"), turns: 128, padding: 128 });
  const rows = source.bytes.toString("utf8").trimEnd().split("\n").map(JSON.parse);
  assert.equal(rows[0].type, "session_meta");
  assert.equal(rows[0].payload.id, source.nativeSessionId);
  assert.equal(rows.at(-1).payload.type, "task_complete");
  assert.equal(rows.at(-1).payload.turn_id, source.targetTurn);
  assert.equal(rows.at(-1).payload.last_agent_message, source.body);
  assert.equal(rows.at(-2).payload.phase, "final_answer");
  assert.equal(rows.at(-2).payload.content[0].text, source.body);
  assert.equal(Buffer.byteLength(source.body), 950);
  assert.match(source.body, /^[\x00-\x7f]+$/);
  assert.equal(source.body.split("\n\n").length, 10);
  assert.equal(source.bodySha256, createHash("sha256").update(source.body).digest("hex"));
  assert.equal(rows.filter((row) => row.type === "turn_context").length, 129);
  const targetPrompt = rows.findIndex((row) => row.type === "response_item" && row.payload.role === "user"
    && row.payload.internal_chat_message_metadata_passthrough.turn_id === source.targetTurn);
  const progress = rows.filter((row) => row.payload.type === "agent_message");
  assert.equal(progress.length, 12);
  assert.ok(progress.every((row) => row.payload.turn_id === source.targetTurn && row.payload.phase === "commentary"
    && rows.indexOf(row) > targetPrompt && rows.indexOf(row) < rows.length - 2));
  assert.ok(rows.slice(-8).every((row) => row.payload.turn_id === source.targetTurn
    || row.payload.internal_chat_message_metadata_passthrough?.turn_id === source.targetTurn));
  assert.throws(() => syntheticCodexSource({ workspace: path.resolve("synthetic-workspace"), turns: 20_001 }), /invalid_turn_count/);
});

test("native response qualification excludes Mock, preview and incomplete membership", () => {
  assert.equal(chatTimingReceipt(page(), fixture).targets[0].qualifiedTurn, "committed-turn");
  for (const value of [page({ preview: true }), page({ complete: false })]) {
    assert.equal(chatTimingReceipt(value, fixture).targets[0].qualifiedTurn, null);
  }
  const mock = { ...fixture, provider: "mock" };
  assert.equal(chatTimingReceipt(page({ provider: "mock" }), mock).targets[0].qualifiedTurn, null);
  assert.throws(() => chatTimingReceipt({ ...page(), session_id: "foreign" }, fixture), /foreign_or_unbounded_page/);
  assert.throws(() => chatTimingReceipt({ ...page(), generation: null }, fixture), /invalid_page_scope/);
});

test("public empty unscoped reset is a noncandidate control with no scope or endpoint authority", () => {
  const receipt = chatTimingReceipt(unscopedReset(), fixture);
  assert.equal(receipt.control, "unscoped_reset");
  assert.deepEqual(receipt.targets, []);
  assert.equal(receipt.scope.conversation, null); assert.equal(receipt.scope.generation, null);
  assert.equal(receipt.scope.epoch, null); assert.equal(receipt.scope.admission, null);
  const b = browser(); b.api.arm("empty-reset"); b.click(); b.api.noteRequest(); b.api.receive(receipt); b.advance(5);
  const state = b.api.snapshot();
  assert.equal(state.phase.readableAt, null); assert.equal(state.phase.usableAt, null);
  assert.equal(state.phase.previewAt, null); assert.equal(state.phase.qualifiedAt, null);
  assert.equal(state.faults.length, 0); assert.equal(b.api.prepareInteraction(), null); b.api.dispose();
});

test("unscoped reset recognition rejects partial/missing scope, rows and every authority-bearing control variation", () => {
  const invalid = {
    foreignSession: (p) => { p.session_id = "foreign"; },
    missingRevision: (p) => { delete p.revision; }, emptyRevision: (p) => { p.revision = " "; },
    longRevision: (p) => { p.revision = "r".repeat(1025); },
    missingC: (p) => { delete p.conversation_id; }, missingG: (p) => { delete p.generation; }, missingE: (p) => { delete p.source_epoch; },
    partialC: (p) => { p.conversation_id = "claimed"; }, partialG: (p) => { p.generation = "claimed"; }, partialE: (p) => { p.source_epoch = "claimed"; },
    rows: (p) => { p.events = page().events; }, admission: (p) => { p.recent_admission = " "; },
    undefinedAdmission: (p) => { p.recent_admission = undefined; },
    notReset: (p) => { p.reset = false; }, missingReset: (p) => { delete p.reset; },
    unchanged: (p) => { p.unchanged = true; }, missingUnchanged: (p) => { delete p.unchanged; },
    presentation: (p) => { p.presentation = { version: 1, payload: {} }; }, detail: (p) => { p.detail = {}; },
    cursor: (p) => { p.cursor = "claimed"; }, nextBefore: (p) => { p.next_before = "claimed"; },
    older: (p) => { p.older_page = true; }, hasMore: (p) => { p.has_more = true; }, archive: (p) => { p.archive_read_ready = true; },
    revalidation: (p) => { p.revalidated_ids = []; }, alias: (p) => { p.aliases = [{ observation_id: "a", canonical_id: "b" }]; },
    removal: (p) => { p.removed_ids = ["claimed"]; }, malformedAliases: (p) => { p.aliases = null; },
    malformedPaging: (p) => { p.older_page = null; }, missingProgress: (p) => { delete p.progress; },
  };
  for (const [name, change] of Object.entries(invalid)) {
    const dto = unscopedReset(); change(dto);
    assert.throws(() => chatTimingReceipt(dto, fixture), /invalid_page_scope|foreign_or_unbounded_page/, name);
  }
  const explicitNone = { ...unscopedReset(), recent_admission: null, presentation: null, revalidated_ids: null,
    older_page: false, archive_read_ready: false, cursor: null, has_more: false };
  assert.equal(chatTimingReceipt(explicitNone, fixture).control, "unscoped_reset");
  assert.equal(chatTimingReceipt(page(), fixture).control, undefined);
  assert.equal(chatTimingReceipt(page({ preview: true }), fixture).control, undefined);
});

test("admission-scoped empty resets retain existing scope validation and grant no endpoint", () => {
  const dto = { ...page({ preview: true }), events: [], reset: true, unchanged: false };
  const receipt = chatTimingReceipt(dto, fixture);
  assert.equal(receipt.control, undefined); assert.equal(receipt.scope.admission, dto.recent_admission);
  assert.equal(receipt.targets.length, 0);
  const b = browser(); b.api.arm("admission-empty-reset"); b.click(); b.api.noteRequest();
  try {
    b.scroll.attributes["data-chat-page-scope"] = JSON.stringify(receipt.scope);
    b.api.receive(receipt); b.advance(4);
    const state = b.api.snapshot(); assert.equal(state.faults.length, 0);
    for (const field of ["readableAt", "usableAt", "qualifiedAt"]) assert.equal(state.phase[field], null);
  } finally { b.api.dispose(); }
});

test("control between frames fences old canonical, provisional and legacy receipts despite matching old DOM", () => {
  for (const dto of [page(), presentedPreview(), page({ preview: true })]) {
    const b = browser(); b.api.arm("control-fence"); b.click(); b.api.noteRequest(); b.adopt(dto); b.advance(2);
    b.api.receive(chatTimingReceipt(unscopedReset(), fixture)); b.advance(5);
    const z = b.api.snapshot().phase;
    assert.equal(z.readableAt, null); assert.equal(z.usableAt, null);
    assert.equal(z.previewAt, null); assert.equal(z.qualifiedAt, null);
    assert.equal(b.api.prepareInteraction(), null); b.api.dispose();
  }
});

test("a post-control identical scoped response coalesces but must establish two fresh frames", () => {
  for (const dto of [page(), presentedPreview()]) {
    const b = browser(); b.api.arm("control-resume"); b.click(); b.api.noteRequest(); b.adopt(dto); b.advance(2);
    b.api.receive(chatTimingReceipt(unscopedReset(), fixture)); b.adopt(dto); b.advance();
    assert.equal(b.api.snapshot().phase.readableAt, null); b.advance();
    assert.equal(b.api.snapshot().phase.readableAt, null); b.advance();
    const s = b.api.snapshot(); assert.ok(s.phase.readableAt); assert.equal(s.phase.readable.admitted, true);
    assert.ok(s.phase.frames.readableAt[0] > s.receipts.find(r => r.control).last_at);
    assert.equal(s.raw_receipt_count, 3); assert.equal(s.receipts.length, 2);
    assert.equal(s.receipts.find(r => !r.control).last_sequence, 3); b.api.dispose();
  }
});

test("control preserves first endpoint facts and prevents stale input credit until fresh frames and a new trusted interaction", () => {
  const b = browser(); const dto = presentedPreview(); b.api.arm("control-after-readable"); b.click(); b.api.noteRequest(); b.adopt(dto); b.advance(3);
  const first = b.api.snapshot().phase.readableAt; const firstFrames = b.api.snapshot().phase.frames.readableAt;
  b.api.prepareInteraction(); b.api.receive(chatTimingReceipt(unscopedReset(), fixture));
  b.input.value = "probe"; b.document.activeElement = b.input; b.inputEvent(); b.advance(3);
  assert.equal(b.api.snapshot().phase.usableAt, null); assert.equal(b.api.prepareInteraction(), null);
  b.adopt(dto); b.advance(2); assert.equal(b.api.prepareInteraction(), null); b.advance();
  const resumed = b.api.snapshot().phase;
  assert.equal(resumed.readableAt, first); assert.deepEqual(resumed.frames.readableAt, firstFrames);
  assert.equal(resumed.readable_after_control.control_sequence, 2);
  assert.equal(resumed.readable_after_control.frames.length, 2);
  assert.equal(b.api.prepareInteraction().kind, "wheel"); b.wheelEvent(); b.scroll.scrollTop -= 4; b.advance(3);
  assert.ok(b.api.snapshot().phase.usableAt); assert.equal(b.api.snapshot().phase.qualifiedAt, null);
  assert.equal(b.api.snapshot().phase.readableAt, first); b.api.dispose();
});

test("control cancels an already trusted pending usable proof and cannot infer authority from an unmarked later DOM", () => {
  const b = browser(); b.api.arm("control-after-input"); b.click(); b.api.noteRequest(); b.adopt(presentedPreview()); b.advance(3);
  b.api.prepareInteraction(); b.input.value = "probe"; b.document.activeElement = b.input; b.inputEvent(); b.advance(2);
  b.api.receive(chatTimingReceipt(unscopedReset(), fixture)); b.adopt(page(), false); b.advance(5);
  assert.equal(b.api.snapshot().phase.usableAt, null); assert.equal(b.api.snapshot().phase.qualifiedAt, null);
  assert.equal(b.api.prepareInteraction(), null); b.api.dispose();
});

test("coalesced controls keep their raw sequence fence across a warm phase without granting source identity", () => {
  const b = browser(); b.api.arm("old"); b.click(); b.api.noteRequest(); b.adopt(page()); b.advance(3);
  const firstQ = b.api.snapshot().phase.qualifiedAt;
  const dto = unscopedReset(); dto.revision = "opaque.not-a-source-key~control";
  for (let n = 0; n < 700; n++) b.api.receive(chatTimingReceipt(dto, fixture));
  assert.equal(b.api.snapshot().phase.qualifiedAt, firstQ);
  b.api.arm("warm"); b.click(); b.advance(5); assert.equal(b.api.snapshot().phase.readableAt, null);
  assert.equal(b.api.snapshot().receipts.length, 2); assert.equal(b.api.snapshot().raw_receipt_count, 701);
  b.adopt(page()); b.advance(3); assert.ok(b.api.snapshot().phase.qualifiedAt); b.api.dispose();
});

test("desktop rejected scope evidence is failure-only, content-free and correlated with its request promise", async () => {
  const b = browser({ surface: "desktop" }); b.api.arm("rejected-scope"); b.click();
  const dto = page(); dto.generation = null; dto.conversation_id = "SECRET-CONVERSATION";
  delete dto.recent_admission;
  dto.revision = "SECRET-RESPONSE-REVISION"; dto.source_epoch = "SECRET-EPOCH"; dto.events[0].text = "SECRET-TRANSCRIPT";
  const args = { sessionId: fixture.sessionId, recentPreview: true, revision: "SECRET-REQUEST-REVISION", previewBackingIds: ["SECRET-BACKING"] };
  const delivery = b.deliver(dto, args);
  args.revision = "changed-after-request"; args.previewBackingIds.push("LATER-BACKING");
  await delivery;
  const s = b.api.snapshot(); assert.deepEqual([...s.faults], ["invalid_page_scope"]); assert.equal(s.raw_error_count, 1);
  const e = s.rejected_scopes[0]; assert.equal(e.clock_basis, "browser:performance.now");
  assert.equal(e.fields.generation.is_null, true); assert.equal(e.fields.generation.valid_id, false);
  assert.equal(e.fields.conversation_id.type, "string"); assert.equal(e.fields.conversation_id.nonblank, true);
  assert.equal(e.fields.recent_admission.present, false); assert.equal(e.flags_and_counts.events.count, 1);
  assert.equal(e.request.sequence, 1); assert.equal(e.request.association, "same_onRequest_promise");
  assert.equal(e.request.recent_preview, true); assert.equal(e.request.backing_hint_count, 1);
  assert.equal(e.request.revision_length, "SECRET-REQUEST-REVISION".length);
  assert.ok(Number.isFinite(e.observed_at)); assert.ok(Number.isFinite(e.request.at));
  assert.equal(JSON.stringify(s.rejected_scopes).includes("SECRET"), false);
  await assert.rejects(b.api.wait("qualifiedAt", 50), /timing observation invalidated/); b.api.dispose();
});

test("scope rejection maps SDK cursor and detail requests without retaining their values", async () => {
  for (const [args, older, detail] of [[{ cursor: "SECRET-CURSOR" }, true, false], [{ detailRef: "SECRET-DETAIL" }, false, true]]) {
    const b = browser({ surface: "desktop" }); b.api.arm("request-purpose"); b.click();
    try {
      await b.deliver({ ...page(), generation: null }, { sessionId: fixture.sessionId, recentPreview: false, ...args });
      const request = b.api.snapshot().rejected_scopes[0].request;
      assert.equal(request.older_requested, older);
      assert.equal(request.detail_requested, detail);
      assert.equal(JSON.stringify(request).includes("SECRET"), false);
    } finally { b.api.dispose(); }
  }
});

test("rejected scope metadata remains bounded with accounted overflow and unknown Remote request association", () => {
  let rejected;
  try { chatTimingReceipt({ ...page(), generation: null }, fixture); } catch (error) { rejected = error; }
  const target = {};
  for (let n = 0; n < 100; n++) recordRejectedChatScope(target, rejected, { at: 123 + n, clock_basis: "node:performance.now" });
  assert.ok(target.rejected_scopes.length > 0 && target.rejected_scopes.length <= 4);
  assert.ok(target.rejected_scope_bytes <= 8192); assert.equal(target.rejected_scope_overflow_count, 100 - target.rejected_scopes.length);
  assert.ok(target.rejected_scope_overflow_bytes > 0);
  assert.equal(target.rejected_scope_bytes, target.rejected_scopes.reduce((total, e) => total + Buffer.byteLength(JSON.stringify(e)), 0));
  assert.equal(target.rejected_scopes[0].clock_basis, "node:performance.now");
  assert.equal(target.rejected_scopes[0].request.sequence, null); assert.equal(target.rejected_scopes[0].request.association, "unknown");
  assert.equal(target.rejected_scopes[0].request.at, null); assert.equal(target.rejected_scopes[0].request.args_available, false);
  const before = target.rejected_scopes.length; recordRejectedChatScope(target, new Error("other failure"));
  assert.equal(target.rejected_scopes.length, before);
});

test("valid scope and empty reset control never emit rejected-scope metadata", async () => {
  const b = browser({ surface: "desktop" }); b.api.arm("valid-control"); b.click(); await b.deliver(page());
  await b.deliver(unscopedReset()); assert.equal(b.api.snapshot().faults.length, 0);
  assert.equal(b.api.snapshot().rejected_scopes.length, 0); assert.equal(b.api.snapshot().rejected_scope_bytes, 0); b.api.dispose();
});

test("an admitted provisional source answer in the shared renderer is readable and usable without canonical qualification", () => {
  const b = browser(); const dto = presentedPreview();
  b.api.arm("bound-preview"); b.click(); b.api.noteRequest(); b.adopt(dto);
  b.advance(2); assert.equal(b.api.snapshot().phase.readableAt, null);
  b.advance(); assert.ok(b.api.snapshot().phase.readableAt);
  assert.ok(b.api.snapshot().phase.previewAt); assert.equal(b.api.snapshot().phase.qualifiedAt, null);
  assert.equal(chatTimingReceipt(dto, fixture).targets[0].qualifiedTurn, null);
  const plan = b.api.prepareInteraction(); assert.equal(plan.kind, "input");
  b.input.value = "probe"; b.document.activeElement = b.input; b.inputEvent(); b.advance(3);
  const state = b.api.snapshot();
  const result = assertChatTimingSample(state, { requireQualified: false, requireTransition: false });
  assert.equal(result.renderer_at_readable, "PROVISIONAL_SHARED_TURN_DOM");
  assert.equal(state.phase.readable.presented_answer.provenance, "provisional");
  assert.equal(state.phase.readable.canonical_qualified, false);
  assert.equal(state.phase.qualifiedAt, null); b.api.dispose();
});

test("the presented answer receipt binds literal producer refs separately from canonical qualification", () => {
  const dto = presentedPreview(); dto.events[0].metadata.chat_display_key = "explicit-display-key";
  assert.deepEqual(chatTimingReceipt(dto, fixture).targets[0].presentedAnswer, {
    turnKey: "presented-turn", itemKey: "presented-answer", eventRef: "preview-id", displayKey: "explicit-display-key",
    canonicalHead: "a".repeat(64),
    provenance: "provisional", evidence: { kind: "source", source_key: "native-source", source_epoch: "source-epoch", record_key: "source-record" },
  });
  assert.equal(chatTimingReceipt(dto, fixture).targets[0].qualifiedTurn, null);
  assert.equal(chatTimingReceipt(page(), fixture).targets[0].qualifiedTurn, "committed-turn");
});

test("presented binding rejects unsupported, optimistic, foreign-ref and wrong source evidence", () => {
  const invalid = {
    provider: (p) => { p.events[0].provider = "mock"; },
    optimistic: (p) => { p.events[0].metadata.optimistic = true; },
    nonprovisional: (p) => { p.events[0].metadata.chat_provisional = false; },
    missingAdmission: (p) => { p.recent_admission = null; },
    missingConversation: (p) => { p.conversation_id = null; },
    missingGeneration: (p) => { p.generation = null; },
    missingEpoch: (p) => { p.source_epoch = null; },
    foreignRef: (p) => { p.presentation.payload.items[0].event_refs = ["another-event"]; },
    wrongKind: (p) => { p.presentation.payload.items[0].kind = "prompt"; },
    wrongProvenance: (p) => { p.presentation.payload.items[0].provenance = "canonical"; },
    wrongEvidence: (p) => { p.presentation.payload.items[0].evidence.kind = "inferred"; },
    wrongSourceEpoch: (p) => { p.presentation.payload.items[0].evidence.source_epoch = "other-epoch"; },
    wrongEventEpoch: (p) => { p.events[0].metadata.chat_source_epoch = "other-epoch"; },
    wrongRecord: (p) => { p.presentation.payload.items[0].evidence.record_key = "other-record"; },
    missingSource: (p) => { p.presentation.payload.items[0].evidence.source_key = ""; },
    foreignSource: (p) => { p.presentation.payload.items[0].evidence.source_key = "another-native-source"; },
    missingNativeRef: (p) => { delete p.events[0].metadata.chat_source_ref; },
    missingTurn: (p) => { p.presentation.payload.turns = []; },
  };
  for (const [name, change] of Object.entries(invalid)) {
    const dto = presentedPreview(); change(dto);
    const b = browser(); b.api.arm(name); b.click(); b.api.noteRequest(); b.adopt(dto, true, presentedPreview()); b.advance(5);
    assert.equal(b.api.snapshot().phase.readableAt, null, name);
    assert.equal(b.api.snapshot().phase.qualifiedAt, null, name); b.api.dispose();
  }
});

test("presented admission requires the matching producer bridge and provisional item admission", () => {
  const invalid = {
    missingBridge: (p) => { delete p.presentation.payload.preview_bridge; },
    missingBridgeAdmission: (p) => { delete p.presentation.payload.preview_bridge.admission_id; },
    wrongBridgeAdmission: (p) => { p.presentation.payload.preview_bridge.admission_id = "foreign-admission"; },
    missingItemAdmission: (p) => { delete p.presentation.payload.items[0].admission_id; },
    wrongItemAdmission: (p) => { p.presentation.payload.items[0].admission_id = "foreign-admission"; },
    missingHead: (p) => { delete p.presentation.payload.preview_bridge.canonical_head; },
    invalidHead: (p) => { p.presentation.payload.preview_bridge.canonical_head = "not-a-head-reference"; },
  };
  for (const [name, change] of Object.entries(invalid)) {
    const dto = presentedPreview(); change(dto);
    assert.equal(chatTimingReceipt(dto, fixture).targets[0].presentedAnswer, null, name);
    const b = browser(); b.api.arm(name); b.click(); b.api.noteRequest(); b.adopt(dto, true, presentedPreview()); b.advance(5);
    assert.equal(b.api.snapshot().phase.readableAt, null, name);
    assert.equal(b.api.snapshot().phase.previewAt, null, name);
    assert.equal(b.api.snapshot().phase.qualifiedAt, null, name); b.api.dispose();
  }
});

test("presented binding rejects ambiguous items, turns, refs, targets and display keys", () => {
  const invalid = {
    duplicateItem: (p) => { p.presentation.payload.items.push(structuredClone(p.presentation.payload.items[0])); },
    repeatedRef: (p) => { p.presentation.payload.items[0].event_refs.push(p.events[0].id); },
    extraRef: (p) => { p.presentation.payload.items[0].event_refs.push("another-event"); },
    otherItemOwner: (p) => { p.presentation.payload.items.push({ ...p.presentation.payload.items[0], item_key: "other-item", kind: "prompt" }); },
    repeatedItemKey: (p) => { p.presentation.payload.items.push({ ...p.presentation.payload.items[0], event_refs: ["other-event"] }); },
    duplicateTurn: (p) => { p.presentation.payload.turns.push(structuredClone(p.presentation.payload.turns[0])); },
    duplicateTarget: (p) => { p.events.push({ ...p.events[0], id: "other-target" }); },
    duplicateDisplayKey: (p) => { p.events.push({ ...p.events[0], text: "different body" }); },
  };
  for (const [name, change] of Object.entries(invalid)) {
    const dto = presentedPreview(); change(dto);
    const b = browser(); b.api.arm(name); b.click(); b.api.noteRequest(); b.adopt(dto); b.advance(5);
    assert.equal(b.api.snapshot().phase.readableAt, null, name); b.api.dispose();
  }
});

test("a shared preview requires its literal DOM anchor and turn, never equal text or legacy identity alone", () => {
  const invalid = {
    wrongAnchor: (b) => { b.anchor.attributes["data-chat-anchor-key"] = "wrong-event"; },
    missingAnchor: (b) => { delete b.anchor.attributes["data-chat-anchor-key"]; },
    wrongRow: (b) => { b.row.attributes["data-chat-row-key"] = "wrong-turn"; },
    wrongTurn: (b) => { b.committed.attributes["data-turn-key"] = "wrong-turn"; },
    forgedLegacyIdentity: (b) => { b.row.attributes["data-chat-row-identity"] = JSON.stringify({ id: "preview-id" }); b.anchor.attributes["data-chat-anchor-key"] = "wrong-event"; },
    displayKeyIsNotEventId: (b) => { b.anchor.attributes["data-chat-anchor-key"] = "preview-id"; },
  };
  for (const [name, change] of Object.entries(invalid)) {
    const dto = presentedPreview(); dto.events[0].metadata.chat_display_key = "explicit-display-key";
    const b = browser(); b.api.arm(name); b.click(); b.api.noteRequest(); b.adopt(dto); change(b); b.advance(5);
    assert.equal(b.api.snapshot().phase.readableAt, null, name); b.api.dispose();
  }
});

test("presented recognition fails closed for missing or unequal session C G E A and revision markers", () => {
  for (const field of [null, "session", "conversation", "generation", "epoch", "admission", "revision"]) {
    for (const missing of [false, true]) {
      const b = browser(); b.api.arm(`${field}-${missing}`); b.click(); b.api.noteRequest(); b.adopt(presentedPreview());
      const scope = JSON.parse(b.scroll.attributes["data-chat-page-scope"]);
      if (field === null) delete b.scroll.attributes["data-chat-page-scope"];
      else { if (missing) delete scope[field]; else scope[field] = "different"; b.scroll.attributes["data-chat-page-scope"] = JSON.stringify(scope); }
      b.advance(5); assert.equal(b.api.snapshot().phase.readableAt, null, `${field}-${missing}`); b.api.dispose();
    }
  }
});

test("a later same-scope invalid binding cannot reuse an older valid presented receipt", () => {
  const b = browser(); const dto = presentedPreview(); b.api.arm("replaced-proof"); b.click(); b.api.noteRequest(); b.adopt(dto);
  b.advance(2); const invalid = structuredClone(dto); invalid.presentation.payload.items[0].evidence.record_key = "wrong-record";
  b.adopt(invalid); b.advance(5);
  assert.equal(b.api.snapshot().phase.readableAt, null); assert.equal(b.api.snapshot().phase.previewAt, null); b.api.dispose();
});

test("a revision replacement between frames resets the presented proof and stamps only the new exact scope", () => {
  const b = browser(); const dto = presentedPreview(); b.api.arm("revision"); b.click(); b.api.noteRequest(); b.adopt(dto); b.advance(2);
  dto.revision = "replacement-revision"; b.adopt(dto); b.advance();
  assert.equal(b.api.snapshot().phase.readableAt, null); b.advance(3);
  assert.equal(b.api.snapshot().phase.readable.scope.revision, "replacement-revision");
  assert.equal(b.api.snapshot().phase.qualifiedAt, null); b.api.dispose();
});

test("a producer bridge head replacement between frames resets the presented binding", () => {
  const b = browser(); const dto = presentedPreview(); b.api.arm("bridge-head"); b.click(); b.api.noteRequest(); b.adopt(dto); b.advance(2);
  dto.presentation.payload.preview_bridge.canonical_head = "b".repeat(64); b.adopt(dto); b.advance();
  assert.equal(b.api.snapshot().phase.readableAt, null); b.advance(3);
  assert.equal(b.api.snapshot().phase.readable.presented_answer.canonicalHead, "b".repeat(64));
  assert.equal(b.api.snapshot().phase.qualifiedAt, null); b.api.dispose();
});

test("an out-of-revision marker cannot borrow the previous presented proof", () => {
  const b = browser(); b.api.arm("stale"); b.click(); b.api.noteRequest(); b.adopt(presentedPreview()); b.advance(2);
  const scope = JSON.parse(b.scroll.attributes["data-chat-page-scope"]); scope.revision = "unobserved-revision";
  b.scroll.attributes["data-chat-page-scope"] = JSON.stringify(scope); b.advance(5);
  assert.equal(b.api.snapshot().phase.readableAt, null); assert.equal(b.api.snapshot().phase.previewAt, null); b.api.dispose();
});

test("scope and anchor replacements between frames cannot complete the presented witness", () => {
  for (const field of ["session", "conversation", "generation", "epoch", "admission", "anchor", "turn"]) {
    const b = browser(); b.api.arm(field); b.click(); b.api.noteRequest(); b.adopt(presentedPreview()); b.advance(2);
    if (field === "anchor") b.anchor.attributes["data-chat-anchor-key"] = "different-anchor";
    else if (field === "turn") b.committed.attributes["data-turn-key"] = "different-turn";
    else {
      const scope = JSON.parse(b.scroll.attributes["data-chat-page-scope"]); scope[field] = "different";
      b.scroll.attributes["data-chat-page-scope"] = JSON.stringify(scope);
    }
    b.advance(5); assert.equal(b.api.snapshot().phase.readableAt, null, field);
    assert.equal(b.api.snapshot().phase.previewAt, null, field); b.api.dispose();
  }
});

test("a presented proof replaced after readable cannot complete the original trusted interaction", () => {
  const b = browser(); const dto = presentedPreview(); b.api.arm("stale-interaction"); b.click(); b.api.noteRequest(); b.adopt(dto); b.advance(3);
  b.api.prepareInteraction(); dto.revision = "replacement-revision"; b.adopt(dto);
  b.input.value = "probe"; b.document.activeElement = b.input; b.inputEvent(); b.advance(5);
  assert.equal(b.api.snapshot().phase.usableAt, null); b.api.dispose();
});

test("admitted provisional shared rendering supports trusted wheel and a later separate canonical qualification", () => {
  const b = browser(); b.input.disabled = true; b.api.arm("shared-to-canonical"); b.click(); b.api.noteRequest(); b.adopt(presentedPreview()); b.advance(3);
  assert.equal(b.api.prepareInteraction().kind, "wheel"); b.wheelEvent(); b.advance(3);
  assert.equal(b.api.snapshot().phase.usableAt, null);
  b.scroll.scrollTop += 8; b.advance(3); assert.ok(b.api.snapshot().phase.usableAt);
  assert.equal(b.api.snapshot().phase.qualifiedAt, null); b.adopt(page()); b.advance(3);
  const result = assertChatTimingSample(b.api.snapshot()); assert.equal(result.transition, "OBSERVED");
  assert.equal(result.renderer_at_readable, "PROVISIONAL_SHARED_TURN_DOM"); b.api.dispose();
});

test("desktop provisional shared diagnostics retain literal anchor and presented binding without claiming canonical", () => {
  const b = browser({ surface: "desktop", captureDetails: true }); b.api.arm("desktop-bound"); b.click(); b.api.noteRequest(); b.adopt(presentedPreview()); b.advance(3);
  const state = b.api.snapshot(), diagnostic = state.candidate_diagnostic;
  assert.ok(state.phase.readableAt); assert.equal(state.phase.readable.admitted, true);
  assert.equal(diagnostic.dom.bodies[0].anchor_key, "preview-id");
  assert.equal(diagnostic.dom.candidate_presented_answer.provenance, "provisional");
  assert.equal(diagnostic.dom.candidate_qualified, false); b.api.dispose();
});

test("bounded diagnostics retain the full explicit typed-final candidate and its producer evidence without unrelated content", () => {
  const dto = page();
  const typed = { ...dto.events[0], text: fixture.body.slice(0, 18), turn_id: fixture.targetMessageId,
    metadata: { raw_type: "message", provider_phase: "final_answer", provider_turn_id: fixture.targetTurn,
      chat_source_ref: "opaque-producer-ref", chat_source_start: 185143, chat_source_end: 186425 } };
  dto.events = [typed, { ...typed, id: "unrelated", turn_id: "answer-older", text: "UNRELATED_CONTENT_MUST_NOT_BE_RETAINED",
    metadata: { raw_type: "message", provider_phase: "final_answer", provider_turn_id: "synthetic-history-older" } },
  { ...typed, id: "commentary", turn_id: null, text: "UNRELATED_PROGRESS_CONTENT", metadata: { provider_turn_id: fixture.targetTurn, provider_phase: "commentary" } }];
  const details = chatTimingCandidateDetails(dto, fixture);
  assert.equal(details.status, "FULL_SYNTHETIC_TARGETS_ONLY"); assert.equal(details.selected_events, 1);
  assert.deepEqual(details.events, [typed]); assert.deepEqual(details.items, dto.presentation.payload.items);
  assert.deepEqual(details.turns, dto.presentation.payload.turns); assert.equal(details.page_fields.revision, dto.revision);
  assert.equal(JSON.stringify(details).includes("UNRELATED_CONTENT"), false);
  assert.equal(JSON.stringify(details).includes("UNRELATED_PROGRESS"), false);
  typed.metadata.provider_phase = "mutated-after-observation";
  assert.equal(details.events[0].metadata.provider_phase, "final_answer");
  assert.equal(chatTimingCandidateDetails(dto, { ...fixture, synthetic: false }).status, "EXCLUDED_NON_SYNTHETIC");
  const oversized = structuredClone(dto); oversized.events[0].metadata.large = "x".repeat(20_000);
  const excluded = chatTimingCandidateDetails(oversized, fixture);
  assert.equal(excluded.status, "EXCLUDED_DIAGNOSTIC_BYTE_BOUND"); assert.equal(Object.hasOwn(excluded, "events"), false);
  const crowded = { ...dto, events: Array.from({ length: 5 }, (_, index) => ({ ...typed, id: `candidate-${index}` })) };
  assert.equal(chatTimingCandidateDetails(crowded, fixture).status, "EXCLUDED_DIAGNOSTIC_COUNT_BOUND");
});

test("a failed wait retains matching full synthetic API and legacy DOM before rejection without inventing qualification", async () => {
  const b = browser({ captureDetails: true }); const dto = page({ preview: true });
  dto.events[0].turn_id = fixture.targetMessageId;
  dto.events[0].metadata = { raw_type: "message", provider_phase: "final_answer", provider_turn_id: fixture.targetTurn };
  b.api.arm("failed-qualified"); b.click(); b.api.noteRequest(); b.adopt(dto); b.advance(3);
  await assert.rejects(b.api.wait("qualifiedAt", 30), /qualifiedAt timed out/);
  const saved = b.api.snapshot().candidate_diagnostic;
  assert.equal(saved.reason, "before_endpoint_deadline"); assert.equal(saved.before_deadline, true);
  assert.deepEqual(JSON.parse(JSON.stringify(saved.api.events)), dto.events);
  assert.equal(saved.dom.matching_body_count, 1); assert.equal(saved.dom.bodies[0].text, fixture.body);
  assert.ok(saved.dom.bodies[0].outer_html.includes(fixture.body)); assert.equal(saved.dom.bodies[0].row_key, "preview-id");
  assert.equal(saved.dom.bodies[0].committed_turn, null); assert.equal(saved.dom.candidate_qualified, false);
  b.adopt(page()); b.advance(3);
  const retained = b.api.snapshot().candidate_diagnostic;
  assert.equal(retained.api.page_fields.progress, "provisional"); assert.equal(retained.dom.bodies[0].committed_turn, null);
  b.api.dispose();
});

test("diagnostics bound full DOM bytes and leave the 128-entry evidence cap unchanged", () => {
  const body = "x".repeat(20_000); const b = browser({ body, captureDetails: true });
  b.api.arm("oversized-target"); b.click(); b.api.noteRequest(); b.adopt(page({ body })); b.advance(3);
  const snapshot = b.api.snapshot();
  assert.equal(snapshot.candidate_diagnostic.api.status, "EXCLUDED_DIAGNOSTIC_BYTE_BOUND");
  assert.equal(snapshot.candidate_diagnostic.dom.status, "EXCLUDED_DOM_BYTE_BOUND");
  assert.ok(JSON.stringify(snapshot).length < 16_384);
  assert.equal(snapshot.raw_request_count, 1); assert.equal(snapshot.calls.length, 1); b.api.dispose();
});

test("an endpoint resolving after its diagnostic timer uses the successful current candidate rather than stale failure evidence", async () => {
  const b = browser({ captureDetails: true }); b.api.arm("late-success"); b.click(); b.api.noteRequest();
  b.adopt(page({ preview: true })); b.advance(3);
  const wait = b.api.wait("qualifiedAt", 40);
  await new Promise((resolve) => setTimeout(resolve, 35));
  b.adopt(page()); b.advance(5); await wait;
  const diagnostic = b.api.snapshot().candidate_diagnostic;
  assert.equal(diagnostic.reason, "snapshot"); assert.equal(diagnostic.api.page_fields.progress, "ready");
  assert.equal(diagnostic.dom.bodies[0].committed_turn, "committed-turn"); b.api.dispose();
});

test("an invalid API scope still retains its full synthetic candidate and the decoder error", async () => {
  const b = browser({ surface: "desktop", captureDetails: true }); const dto = { ...page(), revision: null };
  dto.events[0].turn_id = fixture.targetMessageId;
  dto.events[0].metadata = { raw_type: "message", provider_phase: "final_answer", provider_turn_id: fixture.targetTurn };
  b.api.arm("invalid-api-scope"); b.click(); await b.deliver(dto);
  const snapshot = b.api.snapshot();
  assert.deepEqual(Array.from(snapshot.faults), ["invalid_page_scope"]);
  assert.equal(snapshot.raw_error_count, 1); assert.equal(snapshot.raw_receipt_count, 0);
  assert.equal(snapshot.candidate_diagnostic.api.page_fields.revision, null);
  assert.deepEqual(JSON.parse(JSON.stringify(snapshot.candidate_diagnostic.api.events)), dto.events);
  assert.equal(snapshot.candidate_diagnostic.dom.bodies[0].committed_turn, null); b.api.dispose();
});

test("the real Markdown paragraph text matches the full fixture while the wire receipt retains its raw body", () => {
  const source = syntheticCodexSource({ workspace: path.resolve("synthetic-workspace"), turns: 128, padding: 128 });
  const dom = new JSDOM(renderToStaticMarkup(React.createElement(Markdown, null, source.body)));
  const rendered = dom.window.document.body.textContent;
  assert.notEqual(rendered, source.body);
  assert.equal(rendered, source.body.split("\n\n").join("\n"));
  const b = browser({ body: source.body }); b.input.disabled = true;
  b.content.textContent = rendered;
  b.api.arm("paragraphs"); b.click(); b.api.noteRequest(); b.adopt(page({ body: source.body })); b.advance(3);
  assert.ok(b.api.snapshot().phase.readableAt);
  assert.equal(b.api.snapshot().receipts[0].targets.length, 1);
  assert.equal(b.api.prepareInteraction().kind, "wheel");
  b.wheelEvent(); b.scroll.scrollTop -= 4; b.advance(3);
  assert.equal(assertChatTimingSample(b.api.snapshot(), { requireTransition: false }).usability_probe, "wheel");
  b.api.dispose(); dom.window.close();
});

test("native observer uses the maintained subscription and rejects a hidden owning tab", async () => {
  const b = browser({ surface: "desktop" }); b.api.arm("cold"); b.click();
  b.adopt(page({ preview: true })); await b.deliver(page({ preview: true }));
  b.tab.isConnected = false; b.advance(3); assert.equal(b.api.snapshot().phase.readableAt, null);
  b.tab.isConnected = true; b.advance(3); assert.ok(b.api.snapshot().phase.readableAt);
  assert.equal(b.api.snapshot().calls.length, 1);
  assert.equal(b.api.snapshot().subscription.request_count, 1);
  assert.equal(b.api.dispose().subscription_active, false);
});

test("a long exact-duplicate poll stream retains raw counts and the two-frame witness without exhausting distinct evidence", async () => {
  const b = browser({ surface: "desktop" }); const dto = page({ preview: true });
  b.api.arm("long-poll"); b.click(); b.adopt(dto); b.advance(3);
  for (let index = 0; index < 2_000; index++) {
    await b.deliver(dto);
    if (index % 100 === 0) b.advance();
  }
  assert.equal(b.api.prepareInteraction().kind, "input");
  b.input.value = "probe"; b.document.activeElement = b.input; b.inputEvent(); b.advance(3);
  const state = b.api.snapshot();
  assert.equal(state.raw_request_count, 2_000); assert.equal(state.coalesced_request_count, 1_999);
  assert.equal(state.raw_receipt_count, 2_001); assert.equal(state.coalesced_receipt_count, 2_000);
  assert.equal(state.calls.length, 1); assert.equal(state.receipts.length, 1);
  assert.equal(state.calls[0].count, 2_000); assert.equal(state.receipts[0].count, 2_001);
  assert.equal(state.receipts[0].first_sequence, 1); assert.equal(state.receipts[0].last_sequence, 2_001);
  assert.ok(state.receipts[0].last_at > state.receipts[0].at);
  assert.equal(state.request_overflow_count, 0); assert.equal(state.receipt_overflow_count, 0);
  assert.equal(state.faults.length, 0);
  assert.equal(assertChatTimingSample(state, { requireQualified: false, requireTransition: false }).usability_probe, "input");
  b.api.dispose();
});

test("coalescing retains changed scope, head, progress, presentation and targets plus distinct errors and current ordering", async () => {
  const b = browser({ surface: "desktop" }); const dto = page(); b.api.arm("changes"); b.click();
  const variants = [dto, { ...dto, generation: "changed-generation" }, { ...dto, revision: "changed-head" },
    { ...dto, progress: "indexing" }, { ...dto, presentation: null },
    { ...dto, events: [{ ...dto.events[0], id: "changed-target" }] }];
  for (const value of variants) { await b.deliver(value); b.advance(); }
  await b.deliver(dto);
  await b.reject(new Error("changed source scope rejected")); await b.reject(new Error("provider read failed"));
  const state = b.api.snapshot();
  assert.equal(state.receipts.length, variants.length); assert.equal(state.coalesced_receipt_count, 1);
  assert.equal(state.receipts.at(-1).scope.generation, dto.generation);
  assert.equal(state.receipts.at(-1).scope.revision, dto.revision);
  assert.equal(state.receipts.at(-1).count, 2); assert.equal(state.receipts.at(-1).last_sequence, 7);
  assert.ok(state.receipts.some((value) => value.scope.generation === "changed-generation"));
  assert.ok(state.receipts.some((value) => value.scope.revision === "changed-head"));
  assert.ok(state.receipts.some((value) => value.progress === "indexing"));
  assert.ok(state.receipts.some((value) => value.presentation.version === null));
  assert.ok(state.receipts.some((value) => value.targets[0].id === "changed-target"));
  assert.deepEqual(Array.from(state.faults), ["changed source scope rejected", "provider read failed"]);
  assert.equal(state.raw_error_count, 2); assert.equal(state.error_overflow_count, 0);
  b.api.dispose();
});

test("distinct request and receipt evidence still fails at the unchanged 128-entry cap", () => {
  const b = browser(); const receipt = chatTimingReceipt(page(), fixture); b.api.arm("bounded"); b.click();
  for (let index = 0; index < 129; index++) {
    b.api.noteRequest({ method: "GET", path: "/chat", query: `?cursor=${index}` });
    b.api.receive({ ...receipt, scope: { ...receipt.scope, revision: `head-${index}` } });
  }
  const state = b.api.snapshot();
  assert.equal(state.calls.length, 128); assert.equal(state.receipts.length, 128);
  assert.equal(state.raw_request_count, 129); assert.equal(state.raw_receipt_count, 129);
  assert.equal(state.coalesced_request_count, 0); assert.equal(state.coalesced_receipt_count, 0);
  assert.equal(state.request_overflow_count, 1); assert.equal(state.receipt_overflow_count, 1);
  assert.deepEqual(Array.from(state.faults), ["timing request distinct evidence bound exceeded", "timing receipt distinct evidence bound exceeded"]);
  assert.throws(() => assertChatTimingSample(state), /observer_fault/); b.api.dispose();
});

test("coalescing a later duplicate cannot erase pre-click request evidence", () => {
  const b = browser(); b.api.noteRequest(); b.advance(); b.api.arm("prewarm"); b.click(); b.api.noteRequest();
  b.adopt(page()); b.advance(3); b.api.prepareInteraction();
  b.input.value = "probe"; b.document.activeElement = b.input; b.inputEvent(); b.advance(3);
  const state = b.api.snapshot();
  assert.equal(state.calls.length, 1); assert.equal(state.calls[0].count, 2);
  assert.ok(state.calls[0].at < state.phase.clickAt);
  assert.throws(() => assertChatTimingSample(state, { requireTransition: false }), /chat_prewarmed/); b.api.dispose();
});

test("trusted click and two-frame readable/usable endpoints precede an actual qualified transition", () => {
  const b = browser();
  b.api.arm("cold"); b.click(); b.api.noteRequest(); b.adopt(page({ preview: true }));
  b.advance(2); assert.equal(b.api.snapshot().phase.readableAt, null);
  b.advance(); assert.ok(b.api.snapshot().phase.readableAt);
  const plan = b.api.prepareInteraction(); assert.equal(plan.kind, "input");
  b.input.value = "probe"; b.document.activeElement = b.input;
  b.inputEvent(false); b.advance(3); assert.equal(b.api.snapshot().phase.usableAt, null);
  b.inputEvent(); b.advance(3); assert.ok(b.api.snapshot().phase.usableAt);
  b.adopt(page()); b.advance(3);
  const result = assertChatTimingSample(b.api.snapshot());
  assert.equal(result.transition, "OBSERVED"); assert.ok(result.usable_ms < 1_000);
  assert.ok(result.preview_to_qualified_ms > 0);
  assert.equal(b.api.dispose().disposed, true); assert.equal(b.listeners.size, 0);
});

test("synthetic click cannot start a timing sample", () => {
  const b = browser(); b.api.arm("cold"); b.click(false); b.adopt(page()); b.advance(4);
  assert.equal(b.api.snapshot().phase.clickAt, null);
  assert.equal(b.api.snapshot().phase.readableAt, null); b.api.dispose();
});

test("a replaced equal-text body cannot complete the original two-frame witness", () => {
  const b = browser(); b.api.arm("cold"); b.click(); b.api.noteRequest(); b.adopt(page({ preview: true }));
  b.advance(2); b.content.isConnected = false; b.advance();
  assert.equal(b.api.snapshot().phase.readableAt, null); b.api.dispose();
});

test("Remote renderer DOM and consumer-scope admission remain separately reported", () => {
  const b = browser(); b.api.arm("cold"); b.click(); b.api.noteRequest(); b.adopt(page(), false); b.advance(3);
  const state = b.api.snapshot();
  assert.ok(state.phase.readableAt); assert.equal(state.phase.readable.admitted, false);
  assert.ok(state.phase.qualifiedAt);
  assert.throws(() => assertChatTimingSample(state), /missing_/); b.api.dispose();
});

test("a short readonly transcript has no input or scroll probe even with a disclosure", () => {
  const b = browser({ disclosure: true });
  b.input.disabled = true; b.scroll.scrollHeight = b.scroll.clientHeight;
  b.api.arm("qualified"); b.click(); b.api.noteRequest(); b.adopt(page()); b.advance(3);
  assert.equal(b.api.prepareInteraction(), null);
  b.workButton.attributes["aria-expanded"] = "true"; b.advance(3);
  assert.equal(b.api.snapshot().phase.usableAt, null);
  b.api.dispose();
});

function acceptedSnapshot() {
  return { faults: [], disposed: false, calls: [{ at: 11 }], receipts: [{}], phases: [{ clickAt: 10 }],
    phase: { clickAt: 10, readableAt: 30, usableAt: 60, previewAt: 30, qualifiedAt: 120, trustedInteractionAt: 40,
      interaction: "wheel", readable: { admitted: true },
      frames: { readableAt: [20, 30], usableAt: [50, 60], qualifiedAt: [110, 120] } } };
}

test("API-only, empty ledgers, prewarm and canonical-first samples cannot pass the cold qualification gate", () => {
  for (const [mutate, code] of [
    [(state) => { state.phase.clickAt = null; }, "missing_trusted_click"],
    [(state) => { state.calls = []; }, "missing_real_page_observation"],
    [(state) => { state.calls[0].at = 0; }, "chat_prewarmed"],
    [(state) => { state.phase.previewAt = null; }, "preview_to_qualified_unobserved"],
    [(state) => { state.phase.frames.usableAt = []; }, "missing_frames_usableAt"],
    [(state) => { state.phase.trustedInteractionAt = null; }, "missing_trusted_interaction"],
    [(state) => { state.phase.readable.admitted = false; }, "missing_admitted_scope_marker"],
  ]) {
    const state = acceptedSnapshot(); mutate(state);
    assert.throws(() => assertChatTimingSample(state), { code });
  }
  const canonicalFirst = acceptedSnapshot(); canonicalFirst.phase.previewAt = null;
  assert.equal(assertChatTimingSample(canonicalFirst, { requireTransition: false }).transition, "CANONICAL_FIRST_NO_TRANSITION_SAMPLE");
});

test("exactly one second fails the strict click-to-usable budget", () => {
  const state = acceptedSnapshot(); state.phase.usableAt = 1_010; state.phase.frames.usableAt = [1_000, 1_010];
  assert.throws(() => assertChatTimingSample(state), { code: "chat_open_budget_exceeded" });
});

test("pending-source UI-only latency stays explicitly unqualified without invented transition metrics", () => {
  const state = acceptedSnapshot();
  state.phase.qualifiedAt = null; state.phase.previewAt = null;
  state.phase.readable = { admitted: false, real_response_observed: true, committed_turn: null };
  const options = { requireQualified: false, requireTransition: false, requireAdmitted: false };
  const result = assertChatTimingSample(state, options);
  assert.equal(result.evidence_level, "UI_ONLY_ADMISSION_UNPROVEN");
  assert.equal(result.renderer_at_readable, "LEGACY_OR_RECENT_PREVIEW");
  assert.equal(result.qualified_ms, null); assert.equal(result.preview_to_qualified_ms, null);
  assert.equal(result.transition, "UNOBSERVED");
  state.phase.readable.real_response_observed = false;
  assert.throws(() => assertChatTimingSample(state, options), { code: "missing_real_body_response" });
});

test("disposal invalidates outstanding endpoint waits and detaches trusted event listeners", async () => {
  const b = browser(); b.api.arm("cold");
  const wait = b.api.wait("readableAt", 100); b.api.dispose(); b.advance();
  await assert.rejects(wait, /invalidated/); assert.equal(b.listeners.size, 0);
});

test("timeout retires its own wait instead of continuing an unbounded animation loop", async () => {
  const b = browser(); b.api.arm("cold");
  await assert.rejects(b.api.wait("readableAt", 1), /timed out/);
  b.advance(3); assert.equal(b.api.snapshot().phase.readableAt, null); b.api.dispose();
});

function tempHome(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-e2e-native-chat-timing-unit-"));
  t.after(() => {
    const resolved = fs.realpathSync(root);
    assert.equal(path.dirname(resolved), fs.realpathSync(os.tmpdir()));
    assert.ok(path.basename(resolved).startsWith("wardian-e2e-native-chat-timing-unit-"));
    fs.rmSync(resolved, { recursive: true, force: true });
  });
  const harness = { isolatedHome: root, runId: randomUUID() };
  acquireHomeLock({ home: root, runId: harness.runId });
  return harness;
}

test("stopped-source staging uses the supported restore layout and never manufactures capture state", (t) => {
  const harness = tempHome(t);
  const staged = stageSyntheticStoppedSource(harness, { turns: 128, padding: 128 });
  assert.ok(staged.sourcePath.includes(`${path.sep}habitat${path.sep}.codex${path.sep}sessions${path.sep}`));
  assert.equal(verifySyntheticSource(staged).successful_output, true);
  const configs = JSON.parse(fs.readFileSync(path.join(harness.isolatedHome, "settings", "state.json")));
  assert.equal(configs[0].provider, "codex"); assert.equal(configs[0].is_off, true);
  assert.equal(configs[0].resume_session, staged.nativeSessionId);
  assert.equal(fs.existsSync(staged.capturePath), false);
  assert.throws(() => pendingCaptureWitness(staged), { code: "missing_stopped_source_capability" });
  assert.throws(() => stageSyntheticStoppedSource(harness, { turns: 128, padding: 128 }), /fixture_must_precede_startup/);
  fs.appendFileSync(staged.sourcePath, "{}\n");
  assert.throws(() => verifySyntheticSource(staged), /source_size_changed/);
});

test("pending metadata is a labelled witness and cannot be replaced by complete or foreign-source metadata", (t) => {
  const harness = tempHome(t); const staged = stageSyntheticStoppedSource(harness, { turns: 128, padding: 128 });
  const capture = { path: staged.sourcePath, provider_source_key: staged.sourceKey, status: "pending", committed_offset: 100,
    native_identity: {}, continuity_anchor: {}, normalizer: {} };
  // App-free negative-control metadata only. Native staging never writes this synthetic capture state.
  const write = (source) => fs.writeFileSync(staged.capturePath, JSON.stringify({ provider_log_sources: [source] }));
  write(capture); assert.equal(pendingCaptureWitness(staged).committed_offset, 100);
  write({ ...capture, path: path.toNamespacedPath(staged.sourcePath) });
  assert.equal(pendingCaptureWitness(staged).committed_offset, 100);
  write({ ...capture, status: "complete" }); assert.throws(() => pendingCaptureWitness(staged), /capture_not_pending_at_open/);
  write({ ...capture, provider_source_key: "foreign" }); assert.throws(() => pendingCaptureWitness(staged), /missing_owned_capture/);
});

test("failed measurements persist partial endpoints and never publish successful metrics", async (t) => {
  const harness = tempHome(t); const file = path.join(harness.isolatedHome, "result.json"); const report = { cases: [] };
  const original = Object.assign(new Error("native interaction failed"), { code: "native_input_failed" });
  await assert.rejects(recordChatTimingCase(report, "cold", async () => { throw original; },
    async () => ({ phase: { clickAt: 10, readableAt: 20, usableAt: null } }), () => writeChatTimingReport(file, report)),
  (error) => error === original);
  const retained = JSON.parse(fs.readFileSync(file));
  assert.equal(retained.cases[0].status, "FAILED"); assert.equal(retained.cases[0].metrics, null);
  assert.equal(retained.cases[0].observation.phase.readableAt, 20);
  assert.equal(retained.cases[0].error.code, "native_input_failed");
});

test("report write failure retains the original measurement error in an aggregate", async () => {
  const original = new Error("original input failure"); const write = new Error("report write failed");
  await assert.rejects(recordChatTimingCase({ cases: [] }, "cold", async () => { throw original; },
    async () => ({ partial: true }), async () => { throw write; }),
  (error) => error instanceof AggregateError && error.cause === original && error.errors[0] === original && error.errors[1] === write);
});

test("original operation, observation and report failures all survive with the primary cause", async () => {
  const original = new Error("original interaction failure"); const observation = new Error("snapshot failure"); const write = new Error("report failure");
  const report = { cases: [] }; let saveAttempted = false;
  await assert.rejects(recordChatTimingCase(report, "cold", async () => { throw original; },
    async () => { throw observation; }, async () => { saveAttempted = true; throw write; }),
  (error) => error instanceof AggregateError && error.cause === original && error.errors.length === 3
    && error.errors[0] === original && error.errors[1] === observation && error.errors[2] === write);
  assert.equal(saveAttempted, true); assert.equal(report.cases[0].status, "FAILED"); assert.equal(report.cases[0].metrics, null);
  assert.equal(report.cases[0].observation_error, observation.message); assert.equal(report.cases[0].report_error, write.message);
});

test("observation and report failures retain both causes after a successful operation", async () => {
  const observation = new Error("snapshot failure"); const write = new Error("report failure"); const report = { cases: [] };
  await assert.rejects(recordChatTimingCase(report, "cold", async () => ({ usable_ms: 40 }),
    async () => { throw observation; }, async () => { throw write; }),
  (error) => error instanceof AggregateError && error.cause === observation && error.errors[0] === observation && error.errors[1] === write);
  assert.equal(report.cases[0].status, "FAILED"); assert.equal(report.cases[0].metrics, null);
});

test("a sole report failure retains its identity and invalidates successful metrics", async () => {
  const write = new Error("report failure"); const report = { cases: [] };
  await assert.rejects(recordChatTimingCase(report, "cold", async () => ({ usable_ms: 40 }),
    async () => ({ partial: true }), async () => { throw write; }), (error) => error === write);
  assert.equal(report.cases[0].status, "FAILED"); assert.equal(report.cases[0].metrics, null);
});

test("lost final observation clears an otherwise successful metric before persisting", async () => {
  const report = { cases: [] }; let retained;
  await assert.rejects(recordChatTimingCase(report, "cold", async () => ({ usable_ms: 40 }),
    async () => { throw new Error("observation lost"); }, async () => { retained = structuredClone(report); }), /observation lost/);
  assert.equal(retained.cases[0].status, "FAILED"); assert.equal(retained.cases[0].metrics, null);
});

test("compiled seeder preserves child failure and rejects zero matched tests or missing receipts", (t) => {
  for (const [result, code] of [
    [{ status: 17, stdout: "", stderr: "owned child failure" }, "native_seed_failed"],
    [{ status: null, error: Object.assign(new Error("owned setup timeout"), { code: "ETIMEDOUT" }), stdout: "running 1 test" }, "native_seed_failed"],
    [{ status: 0, stdout: "running 0 tests\ntest result: ok. 0 passed; 0 failed;" }, "native_seed_test_not_run"],
    [{ status: 0, stdout: "state::conversation_archive::chat_read::tests::seed_native_preview_timing_fixture ... ignored\ntest result: ok. 0 passed; 0 failed; 1 ignored;" }, "native_seed_test_not_run"],
    [{ status: 0, stdout: "state::conversation_archive::chat_read::tests::seed_native_preview_timing_fixture\ntest result: ok. 1 passed;" }, "native_seed_receipt_missing"],
  ]) {
    const harness = tempHome(t); const staged = stageSyntheticStoppedSource(harness, { turns: 128, padding: 128 });
    let calls = 0;
    assert.throws(() => seedNativePreviewSource({ harness, fixture: staged, executable: process.execPath,
      run(executable, args, options) {
        calls++; assert.equal(options.timeout, 180_000);
        assert.equal(path.dirname(executable), path.join(harness.isolatedHome, ".frozen-chat-seeder"));
        assert.deepEqual(args, ["state::conversation_archive::chat_read::tests::seed_native_preview_timing_fixture", "--exact", "--test-threads=1"]);
        const request = JSON.parse(fs.readFileSync(options.env.WARDIAN_CHAT_PREVIEW_SEED_REQUEST));
        assert.equal(request.home, harness.isolatedHome); assert.equal(request.source_path, staged.sourcePath);
        assert.equal(request.run_id, harness.runId); assert.equal(request.mode, "pending");
        assert.equal(options.env.WARDIAN_HOME, harness.isolatedHome);
        return result;
      } }), (error) => error.code === code && (!result.error || error.cause === result.error));
    assert.equal(calls, 1, "A setup failure must never retry");
  }
});

test("a pre-existing seed result is rejected before spawning rather than accepted as a fresh receipt", (t) => {
  const harness = tempHome(t); const staged = stageSyntheticStoppedSource(harness, { turns: 128, padding: 128 });
  const stale = path.join(harness.isolatedHome, "artifacts", "chat-preview-seed", "pending.request.json.result.json");
  fs.mkdirSync(path.dirname(stale), { recursive: true }); fs.writeFileSync(stale, JSON.stringify({ stale: true }));
  let spawned = false;
  assert.throws(() => seedNativePreviewSource({ harness, fixture: staged, executable: process.execPath,
    run() { spawned = true; throw new Error("must not spawn"); } }), { code: "native_seed_receipt_already_exists" });
  assert.equal(spawned, false);
});

test("compiled v2 seed receipt binds mode, terminal proof count, source extent and persisted Head bytes", (t) => {
  for (const [mode, fixtureVersion, completedTurns, error] of [
    ["pending", 2, 0, null], ["qualified", 2, 1, null],
    ["qualified", 1, 1, "unverified_seed_fixture_version"],
    ["qualified", 2, 0, "qualified_seed_has_no_completed_turns"],
    ["qualified", 2, -1, "unverified_seed_fixture_version"],
    ["qualified", 2, 0.5, "unverified_seed_fixture_version"],
  ]) {
    const harness = tempHome(t); const staged = stageSyntheticStoppedSource(harness, { turns: 128, padding: 128 });
    const seed = () => seedNativePreviewSource({ harness, fixture: staged, executable: process.execPath, mode,
      run(_executable, _args, options) {
        // App-free receipt negative/positive controls; this does not claim backend acquisition or UI timing.
        const requestFile = options.env.WARDIAN_CHAT_PREVIEW_SEED_REQUEST;
        const headBytes = JSON.stringify({ receipt_contract_fixture: true });
        const headHash = createHash("sha256").update(headBytes).digest("hex");
        const agentRoot = path.join(harness.isolatedHome, "agents", staged.sessionId);
        fs.mkdirSync(path.join(agentRoot, "chat-read-objects"));
        fs.writeFileSync(path.join(agentRoot, "chat-read-head.json"), JSON.stringify(headHash) + "\n");
        fs.writeFileSync(path.join(agentRoot, "chat-read-objects", headHash), headBytes);
        fs.writeFileSync(`${requestFile}.result.json`, JSON.stringify({ fixture_version: fixtureVersion, completed_turns: completedTurns,
          session_id: staged.sessionId,
          native_session_id: staged.nativeSessionId, source_bytes: staged.sourceBytes,
          capture: { status: mode === "pending" ? "pending" : "complete", committed_offset: mode === "pending" ? 100 : staged.sourceBytes },
          head: { conversation_id: "conversation", generation: "generation", sha256: headHash } }));
        return { status: 0, stdout: "state::conversation_archive::chat_read::tests::seed_native_preview_timing_fixture\ntest result: ok. 1 passed;" };
      } });
    if (error) { assert.throws(seed, { code: error }); continue; }
    const result = seed();
    assert.equal(result.mode, mode); assert.equal(result.capture.status, mode === "pending" ? "pending" : "complete");
    assert.equal(result.fixture_version, 2); assert.equal(result.completed_turns, completedTurns);
    assert.match(result.executable_sha256, /^[a-f0-9]{64}$/);
  }
});

function capture(offset, at) {
  return { status: "pending", committed_offset: offset, source_bytes: 1_000, source_key: "codex:session:owned",
    native_identity_sha256: "identity", dev: "device", ino: "inode", policy_generation: 1, provider: "codex", observed_at_ms: at };
}

test("pending-at-click bracket rejects completion, cursor rewind and source/policy identity changes", () => {
  const before = capture(100, 10); const after = capture(200, 20);
  assert.equal(assertPendingCaptureBracket(before, after).status, "PENDING_BRACKETED_TRUSTED_CLICK");
  assert.throws(() => assertPendingCaptureBracket(before, { ...after, status: "complete", committed_offset: 1_000 }), { code: "capture_completed_during_click" });
  assert.throws(() => assertPendingCaptureBracket(before, { ...after, committed_offset: 90 }), { code: "capture_cursor_not_monotonic" });
  for (const key of ["native_identity_sha256", "ino", "source_key", "policy_generation"]) {
    assert.throws(() => assertPendingCaptureBracket(before, { ...after, [key]: "changed" }), { code: "capture_identity_changed_during_click" });
  }
});

test("pending click wrapper reads after joined trusted input before returning to any body wait", async () => {
  const order = []; const record = {}; let reads = 0;
  await trustedClickWithPendingCapture({}, async () => {
    order.push("click-start"); await Promise.resolve(); order.push("click-joined");
  }, record, () => { order.push(reads ? "after" : "before"); return capture(100 + reads++ * 10, reads * 10); });
  assert.deepEqual(order, ["before", "click-start", "click-joined", "after"]);
  assert.equal(record.after.committed_offset, 110); assert.ok(record.proof);
});

test("pending click completion retains both witnesses and publishes no pending proof", async () => {
  const record = {}; let reads = 0;
  await assert.rejects(trustedClickWithPendingCapture({}, async () => {}, record,
    () => reads++ ? { ...capture(1_000, 20), status: "complete" } : capture(100, 10)), { code: "capture_completed_during_click" });
  assert.equal(record.before.status, "pending"); assert.equal(record.after.status, "complete"); assert.equal(record.proof, undefined);
});

test("an exactly associated desktop busy rejection recovers only from a later request and cleared alert", async () => {
  const b = browser({ surface: "desktop" }); const dto = presentedPreview();
  b.api.arm("busy-recovery"); b.click(); b.adopt(dto); b.advance(3);
  const first = b.api.snapshot().phase.readableAt;
  await b.reject(new Error("chat source temporarily busy")); b.transcriptAlert.hidden = false;
  assert.equal(b.api.snapshot().raw_error_count, 1);
  assert.deepEqual(Array.from(b.api.snapshot().faults), [], "only the exact associated busy error is nonfatal");
  b.advance(2); assert.equal(b.api.prepareInteraction(), null);
  await b.deliver(dto); b.advance(3);
  assert.equal(b.api.prepareInteraction(), null, "a visible direct-child transcript alert blocks recovery");
  b.transcriptAlert.hidden = true; b.advance(3);
  assert.equal(b.api.snapshot().busy_retries[0].recovered_at !== null, true);
  const plan = b.api.prepareInteraction(); assert.equal(plan.kind, "input");
  b.input.value = "probe"; b.document.activeElement = b.input; b.inputEvent(); b.advance(3);
  const result = b.api.snapshot(); assert.ok(result.phase.usableAt);
  assert.equal(result.phase.readableAt, first); assert.equal(result.raw_error_count, 1);
  assert.equal(assertChatTimingSample(result, { requireQualified: false, requireTransition: false }).usability_probe, "input");
  b.api.dispose();
});

test("busy recovery rejects old and pre-rejection dispatched replies, controls and a persisting alert", async () => {
  const b = browser({ surface: "desktop" }); const dto = presentedPreview();
  b.api.arm("busy-order"); b.click(); b.adopt(dto); b.advance(3);
  let rejectBusy; let resolveOld;
  const busy = b.observe(new Promise((_, reject) => { rejectBusy = reject; }));
  const old = b.observe(new Promise((resolve) => { resolveOld = resolve; }));
  rejectBusy("chat source temporarily busy"); await busy; b.advance(1);
  resolveOld(dto); await old; b.advance(4);
  assert.equal(b.api.snapshot().busy_retries[0].recovered_at, null);
  assert.equal(b.api.prepareInteraction(), null, "a successful request dispatched before rejection cannot recover");
  await b.deliver(unscopedReset()); b.advance(4);
  assert.equal(b.api.snapshot().busy_retries[0].recovered_at, null);
  assert.throws(() => assertChatTimingSample(b.api.snapshot(), { requireQualified: false }), { code: "unresolved_chat_retry" });
  b.advance(1); b.transcriptAlert.hidden = false; await b.deliver(dto); b.advance(4);
  await assert.rejects(b.api.wait("usableAt", 2), /usableAt timed out/);
  assert.equal(b.api.snapshot().busy_retries[0].recovered_at, null);
  b.transcriptAlert.hidden = true; b.advance(2); b.transcriptAlert.hidden = false; b.advance();
  assert.equal(b.api.snapshot().busy_retries[0].recovered_at, null, "alert absence is required at both recovery frames");
  b.transcriptAlert.hidden = true; b.advance(3);
  assert.ok(b.api.snapshot().busy_retries[0].recovered_at); b.api.dispose();
});

test("busy exceptions preserve the original promise and dispatch phase; unknown association and other errors stay fatal", async () => {
  const b = browser({ surface: "desktop" }); b.api.arm("cold-dispatch"); b.click();
  let reject; const original = new Promise((_, no) => { reject = no; }); const observation = b.observe(original);
  b.api.arm("warm-arrival"); b.click(); reject(new Error("chat source temporarily busy")); await observation;
  await assert.rejects(original, /chat source temporarily busy/);
  const entry = b.api.snapshot().busy_retries[0]; assert.equal(entry.phase_label, "cold-dispatch");
  assert.equal(entry.phase_index, 0); assert.equal(entry.operation, "load_agent_chat_page");
  assert.equal(b.api.snapshot().phase.label, "warm-arrival"); assert.equal(b.api.snapshot().raw_error_count, 1);
  b.api.dispose();
  for (const [message, args] of [["chat source temporarily busy", { sessionId: "foreign" }],
    ["chat source temporarily busy!", { sessionId: fixture.sessionId }], ["other failure", { sessionId: fixture.sessionId }]]) {
    const bad = browser({ surface: "desktop" }); bad.api.arm("fatal"); bad.click();
    await bad.observe(Promise.reject(new Error(message)), args);
    assert.equal(bad.api.snapshot().faults.length, 1); assert.equal(bad.api.snapshot().busy_retries.length, 0); bad.api.dispose();
  }
});

test("busy evidence is content-free and bounded; historical endpoints cannot bypass an unresolved retry", async () => {
  const b = browser({ surface: "desktop" }); b.api.arm("busy-cap"); b.click(); b.adopt(page()); b.advance(3);
  b.api.prepareInteraction(); b.input.value = "probe"; b.document.activeElement = b.input; b.inputEvent(); b.advance(3);
  const usable = b.api.snapshot().phase.usableAt; assert.ok(usable);
  await b.reject(new Error("chat source temporarily busy"));
  assert.equal(b.api.snapshot().phase.usableAt, usable); assert.equal(b.api.prepareInteraction(), null);
  await assert.rejects(b.api.wait("qualifiedAt", 2), /qualifiedAt timed out/);
  assert.throws(() => assertChatTimingSample(b.api.snapshot()), { code: "unresolved_chat_retry" });
  for (let index = 1; index < 9; index++) await b.reject(new Error("chat source temporarily busy"));
  const state = b.api.snapshot(); assert.equal(state.raw_error_count, 9); assert.equal(state.busy_retries.length, 8);
  assert.equal(state.busy_retry_overflow_count, 1); assert.ok(state.busy_retry_overflow_bytes > 0);
  assert.ok(state.busy_retry_bytes <= 8_192); assert.equal(state.faults.length, 1);
  assert.equal(JSON.stringify(state.busy_retries).includes(fixture.sessionId), false); b.api.dispose();
});

test("busy after healthy first endpoints preserves metrics but requires fresh current health and canonical proof", async () => {
  const b = browser({ surface: "desktop" }); b.api.arm("healthy-before-busy"); b.click(); b.api.noteRequest(); b.adopt(page()); b.advance(3);
  b.api.prepareInteraction(); b.input.value = "probe"; b.document.activeElement = b.input; b.inputEvent(); b.advance(3);
  const initial = b.api.snapshot(); const first = assertChatTimingSample(initial, { requireTransition: false });
  await b.reject(new Error("chat source temporarily busy")); b.advance(400);
  assert.equal(b.api.snapshot().phase.usableAt, initial.phase.usableAt);
  await assert.rejects(b.api.wait("qualifiedAt", 2), /qualifiedAt timed out/);
  assert.throws(() => assertChatTimingSample(b.api.snapshot(), { requireTransition: false }), { code: "unresolved_chat_retry" });
  const provisional = presentedPreview(); b.adopt(provisional); await b.deliver(provisional); b.advance(3);
  assert.ok(b.api.snapshot().busy_retries[0].recovered_at);
  assert.throws(() => assertChatTimingSample(b.api.snapshot(), { requireTransition: false }), { code: "missing_current_retry_qualification" });
  b.adopt(page()); await b.deliver(page()); b.advance(3);
  const state = b.api.snapshot(); const result = assertChatTimingSample(state, { requireTransition: false });
  assert.equal(result.usable_ms, first.usable_ms); assert.equal(result.qualified_ms, first.qualified_ms);
  assert.equal(state.phase.usableAt, initial.phase.usableAt); assert.equal(state.phase.qualifiedAt, initial.phase.qualifiedAt);
  assert.ok(state.phase.qualified_after_retry.at > initial.phase.qualifiedAt);
  assert.equal(state.phase.qualified_after_retry.frames.length, 2);
  assert.equal(state.phase.probe.attempts.length, 1, "a healthy historical first witness does not require a late extra input");
  b.api.dispose();
});

test("oversized busy metadata fails closed without retaining a record beyond its byte cap", async () => {
  const b = browser({ surface: "desktop" }); b.api.arm("bounded-label".repeat(800)); b.click();
  await b.reject(new Error("chat source temporarily busy"));
  const state = b.api.snapshot(); assert.equal(state.busy_retries.length, 0);
  assert.equal(state.busy_retry_bytes, 0); assert.equal(state.busy_retry_overflow_count, 1);
  assert.ok(state.busy_retry_overflow_bytes > 8_192); assert.equal(state.fatal_error_count, 1); b.api.dispose();
});

test("node replacement and target retraction cannot reuse a dispatched old probe", () => {
  const b = browser(); b.input.disabled = true; const dto = presentedPreview();
  b.api.arm("dom-replacement"); b.click(); b.adopt(dto); b.advance(3);
  const first = b.api.prepareInteraction(); assert.equal(b.api.isInteractionCurrent(first.attempt), true);
  const replacement = { ...b.content }; b.content.isConnected = false;
  b.scroll.querySelectorAll = (selector) => selector === ':scope > [role="alert"]' ? [] : [replacement];
  b.wheelEvent(); b.scroll.scrollTop -= 4; b.advance(3);
  assert.equal(b.api.snapshot().phase.usableAt, null);
  const second = b.api.prepareInteraction(); assert.equal(second.attempt, 2);
  const retracted = structuredClone(dto); retracted.events = [];
  b.api.receive(chatTimingReceipt(retracted, fixture)); b.advance(3);
  assert.equal(b.api.isInteractionCurrent(second.attempt), false); assert.equal(b.api.prepareInteraction(), null);
  assert.equal(b.api.snapshot().phase.usableAt, null); b.api.dispose();
});

test("a replaced probe subject requires fresh paint and a new trusted wheel without resetting clocks", () => {
  const b = browser(); b.input.disabled = true; b.scroll.scrollTop = 0;
  const dto = presentedPreview(); b.api.arm("probe-replacement"); b.click(); b.api.noteRequest(); b.adopt(dto); b.advance(3);
  const first = b.api.snapshot().phase;
  const plan = b.api.prepareInteraction(); assert.equal(plan.kind, "wheel");
  const deadline = b.api.snapshot().phase.probe.deadline_at;
  b.wheelEvent(true, plan.delta); b.scroll.scrollTop += plan.delta; b.advance(2);
  dto.revision = "replacement-after-trusted-wheel"; b.adopt(dto); b.advance(3);
  assert.equal(b.api.snapshot().phase.usableAt, null, "old input must not credit the replacement");
  const replacement = b.api.prepareInteraction();
  assert.equal(replacement?.kind, "wheel", "a fresh admitted subject needs a fresh safe probe");
  assert.equal(b.api.snapshot().phase.readableAt, first.readableAt);
  assert.equal(b.api.snapshot().phase.clickAt, first.clickAt);
  b.advance(3); assert.equal(b.api.snapshot().phase.usableAt, null);
  b.wheelEvent(); b.scroll.scrollTop += replacement.delta; b.advance(3);
  const result = b.api.snapshot().phase;
  assert.ok(result.usableAt); assert.equal(result.usable_subject.response_scope.revision, dto.revision);
  assert.equal(result.clickAt, first.clickAt); assert.equal(result.readableAt, first.readableAt);
  assert.equal(result.probe.deadline_at, deadline); assert.equal(result.probe.attempts.length, 2);
  assert.equal(result.probe.attempts[0].event_delta_y, plan.delta);
  assert.equal(result.probe.attempts[0].invalidation, "subject_replaced");
  assert.equal(result.probe.attempts[1].last_check.same_subject, true);
  assert.equal(result.probe.attempts[1].last_check.before !== result.probe.attempts[1].last_check.scroll_top, true);
  assert.ok(result.probe.evidence_bytes <= 4_096); assert.equal(result.probe.evidence_overflow_count, 0);
  b.api.dispose();
});

test("probe attempts cap at three and unchanged nonmovement cannot retry or renew the deadline", () => {
  const b = browser(); b.input.disabled = true; b.scroll.scrollTop = 0;
  const dto = presentedPreview(); b.api.arm("three-probes"); b.click(); b.adopt(dto); b.advance(3);
  let deadline;
  for (let index = 0; index < 3; index++) {
    const plan = b.api.prepareInteraction(); assert.equal(plan.attempt, index + 1);
    deadline ??= b.api.snapshot().phase.probe.deadline_at;
    b.wheelEvent(true, plan.delta); b.scroll.scrollTop += plan.delta;
    dto.revision = `next-subject-${index}`; b.adopt(dto); b.advance(3);
    assert.equal(b.api.snapshot().phase.usableAt, null);
    assert.equal(b.api.snapshot().phase.probe.deadline_at, deadline);
  }
  assert.equal(b.api.prepareInteraction().code, "probe_attempts_exhausted");
  assert.equal(b.api.snapshot().phase.probe.attempts.length, 3); b.api.dispose();
  const stable = browser(); stable.input.disabled = true;
  stable.api.arm("no-movement"); stable.click(); stable.adopt(page()); stable.advance(3);
  stable.api.prepareInteraction(); stable.wheelEvent(); stable.advance(4);
  assert.equal(stable.api.prepareInteraction(), null); assert.equal(stable.api.snapshot().phase.probe.attempts.length, 1);
  stable.advance(320); assert.equal(stable.api.prepareInteraction().code, "probe_deadline_exceeded");
  assert.equal(stable.api.snapshot().phase.usableAt, null); stable.api.dispose();
});

test("probe driver shares one absolute deadline, rejects repeated attempts and preserves cleanup errors", async () => {
  let clock = 0; const remaining = []; const released = []; let index = 0;
  await driveChatPreviewProbe({ now: () => clock,
    next: async (left) => { remaining.push(left); clock += 50; return ++index < 3 ? { kind: "wheel", attempt: index } : { kind: "complete" }; },
    isCurrent: async () => true, act: async () => { clock += 500; }, release: async (plan) => released.push(plan.kind) });
  assert.deepEqual(remaining, [5000, 4450, 3900]); assert.equal(released.length, 3);
  await assert.rejects(driveChatPreviewProbe({ now: () => clock,
    next: async () => ({ kind: "wheel", attempt: 1 }), isCurrent: async () => true,
    act: async () => { clock += 5001; } }), { code: "probe_deadline_exceeded" });
  await assert.rejects(driveChatPreviewProbe({ next: async () => ({ kind: "wheel", attempt: 1 }),
    isCurrent: async () => true, act: async () => {} }), { code: "invalid_or_repeated_probe_attempt" });
  await assert.rejects(driveChatPreviewProbe({ next: async () => ({ kind: "failed", code: "probe_attempts_exhausted" }),
    release: async () => { throw new Error("release-failed"); } }), (error) => error instanceof AggregateError
      && error.cause.code === "probe_attempts_exhausted" && error.errors[1].message === "release-failed");
});

test("readonly overflowing output requires a trusted wheel and actual two-frame scroll change", () => {
  const b = browser(); b.input.disabled = true;
  b.api.arm("overflow"); b.click(); b.api.noteRequest(); b.adopt(page()); b.advance(3);
  assert.equal(b.api.prepareInteraction().kind, "wheel");
  b.wheelEvent(false); b.scroll.scrollTop -= 4; b.advance(3); assert.equal(b.api.snapshot().phase.usableAt, null);
  b.scroll.scrollTop += 4; b.wheelEvent(); b.advance(3); assert.equal(b.api.snapshot().phase.usableAt, null);
  b.scroll.scrollTop -= 4; b.advance(3);
  assert.equal(assertChatTimingSample(b.api.snapshot(), { requireTransition: false }).usability_probe, "wheel"); b.api.dispose();
});

test("exact eight-pixel Remote overflow permits a wheel probe but requires trusted movement and two frames", () => {
  const b = browser(); b.input.disabled = true;
  b.scroll.scrollHeight = 666; b.scroll.clientHeight = 658; b.scroll.scrollTop = 0;
  b.api.arm("remote-warm-eight-pixels"); b.click(); b.api.noteRequest(); b.adopt(page()); b.advance(3);
  const readable = b.api.snapshot().phase.readable;
  const plan = b.api.prepareInteraction();
  assert.equal(plan?.kind, "wheel", "the retained 666/658 geometry has positive actual overflow");
  assert.equal(plan.node, b.scroll); assert.equal(plan.delta, 4);
  b.wheelEvent(false); b.scroll.scrollTop = 4; b.advance(3);
  assert.equal(b.api.snapshot().phase.usableAt, null);
  assert.equal(b.api.snapshot().phase.trustedInteractionAt, null);
  b.scroll.scrollTop = 0; b.wheelEvent(); b.advance(3);
  assert.ok(b.api.snapshot().phase.trustedInteractionAt);
  assert.equal(b.api.snapshot().phase.usableAt, null, "a trusted wheel without movement is insufficient");
  b.scroll.scrollTop = 4; b.advance(2);
  assert.equal(b.api.snapshot().phase.usableAt, null, "both post-movement frame checks must finish");
  b.advance();
  const snapshot = b.api.snapshot();
  assert.equal(snapshot.phase.frames.usableAt.length, 2);
  assert.equal(snapshot.phase.usable_subject.row_key, readable.row_key);
  assert.equal(snapshot.phase.usable_subject.committed_turn, readable.committed_turn);
  assert.equal(assertChatTimingSample(snapshot, { requireTransition: false }).usability_probe, "wheel"); b.api.dispose();
});

test("zero overflow declines a readonly wheel probe and cannot mark output usable", () => {
  const b = browser(); b.input.disabled = true;
  b.scroll.scrollHeight = 658; b.scroll.clientHeight = 658; b.scroll.scrollTop = 0;
  b.api.arm("zero-overflow"); b.click(); b.api.noteRequest(); b.adopt(page()); b.advance(3);
  assert.ok(b.api.snapshot().phase.readableAt);
  assert.equal(b.api.prepareInteraction(), null);
  b.wheelEvent(); b.advance(3);
  assert.equal(b.api.snapshot().phase.trustedInteractionAt, null);
  assert.equal(b.api.snapshot().phase.usableAt, null); b.api.dispose();
});

test("normal desktop Chat activation keeps the original trusted Roster clock and permits retained warm mode", () => {
  const b = browser({ surface: "desktop", modeNeedsToggle: true }); b.api.arm("cold"); b.click();
  const clickAt = b.api.snapshot().phase.clickAt;
  b.advance(4); assert.equal(b.api.modeControl().needsToggle, true); b.normalModeToggle();
  assert.equal(b.api.snapshot().phase.clickAt, clickAt); assert.equal(b.api.modeControl().needsToggle, false);
  b.adopt(page()); b.advance(3); assert.ok(b.api.snapshot().phase.readableAt > clickAt);
  b.api.arm("warm"); b.click(); assert.equal(b.api.modeControl().needsToggle, false);
  b.api.dispose();
});

test("usable subject may bind a later real render before the first probe without resetting the click/readable clocks", () => {
  const b = browser(); b.api.arm("transition"); b.click(); b.api.noteRequest(); b.adopt(page({ preview: true })); b.advance(3);
  const original = b.api.snapshot().phase;
  b.adopt(page()); assert.equal(b.api.prepareInteraction(), null);
  b.advance(3); assert.equal(b.api.prepareInteraction().kind, "input");
  b.input.value = "probe"; b.document.activeElement = b.input; b.inputEvent(); b.advance(3);
  const final = b.api.snapshot();
  assert.equal(final.phase.clickAt, original.clickAt); assert.equal(final.phase.readableAt, original.readableAt);
  assert.equal(final.phase.probe_uses_later_render, true);
  assert.equal(final.phase.readable.row_key, "preview-id"); assert.equal(final.phase.usable_subject.row_key, "committed-turn");
  assert.equal(assertChatTimingSample(final).transition, "OBSERVED"); b.api.dispose();
});
