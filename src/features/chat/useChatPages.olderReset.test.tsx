// SYNTHETIC_TRANSPORT_FRONTEND_UNIT_ONLY: mock loader transport; no producer, backend, or native proof.
import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AgentChatEvent, AgentChatPage } from "../../types";
import { useChatPages, type ChatPageLoader } from "./useChatPages";

const SESSION = "synthetic-agent";
const EPOCH = "synthetic-source-epoch";
const ADMISSION = "synthetic-admission";

function page(overrides: Partial<AgentChatPage> = {}): AgentChatPage {
  return {
    session_id: SESSION, conversation_id: "synthetic-conversation", generation: "synthetic-generation",
    source_epoch: EPOCH, revision: "revision-0", events: [], next_before: null, unchanged: false,
    reset: false, progress: "ready", aliases: [], removed_ids: [], detail: null, bytes_read: 0,
    records_decoded: 0, ...overrides,
  };
}

function sourceEvent(label: string, provisional = false, sequence = 1): AgentChatEvent {
  return {
    id: "header-" + label, session_id: SESSION, provider: "codex", kind: "message", role: "user",
    text: "synthetic row " + label, title: null, status: null, turn_id: null, source: null,
    command: null, exit_code: null, path: null, language: null, created_at: null, sequence,
    metadata: {
      chat_source_ref: "source:" + label, chat_source_epoch: EPOCH, chat_source_admission: ADMISSION,
      ...(provisional ? { chat_provisional: true } : {}),
    },
  };
}

function withoutProvisionalAdmission(event: AgentChatEvent): AgentChatEvent {
  const metadata = { ...event.metadata };
  delete metadata.chat_provisional;
  delete metadata.chat_source_admission;
  return { ...event, metadata };
}

function syntheticRenewal(request: Parameters<ChatPageLoader>[0], cohort: AgentChatEvent[]): AgentChatPage {
  const bySource = new Map(cohort.map((event) => [event.metadata.chat_source_ref as string, event]));
  const ids = request.revalidateIds ?? [];
  const events = ids.map((id) => {
    const row = bySource.get(id);
    if (!row) throw new Error("Synthetic transport has no row for requested source ID: " + id);
    return row;
  });
  return page({
    revision: request.revision ?? "revision-0", progress: "ready", events, revalidated_ids: [...ids],
    bytes_read: events.length * 64, records_decoded: events.length,
  });
}

function snapshot(state: ReturnType<typeof useChatPages>) {
  return {
    eventIds: state.events.map((event) => event.id).sort(),
    displaySlots: state.events.map((event) => ({
      eventId: event.id,
      displayKey: typeof event.metadata.chat_display_key === "string" ? event.metadata.chat_display_key : null,
    })).sort((left, right) => left.eventId.localeCompare(right.eventId)),
    cursor: state.page?.next_before ?? null,
    browsingOlder: state.browsingOlder,
    loadingOlder: state.loadingOlder,
    revision: state.page?.revision ?? null,
    error: state.error,
  };
}

function expectedSnapshot(events: AgentChatEvent[], cursor: string | null, browsingOlder: boolean, revision: string) {
  return {
    eventIds: events.map((event) => event.id).sort(),
    displaySlots: events.map((event) => ({ eventId: event.id, displayKey: event.id }))
      .sort((left, right) => left.eventId.localeCompare(right.eventId)),
    cursor, browsingOlder, loadingOlder: false, revision, error: null,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function signal() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

async function waitForRevision(
  hook: { result: { current: ReturnType<typeof useChatPages> } },
  revision: string,
) {
  await waitFor(() => expect(hook.result.current.page?.revision).toBe(revision));
}

describe("SYNTHETIC_TRANSPORT_FRONTEND_UNIT_ONLY: compatible older reset pin lifecycle", () => {
  it("retains the 83-row cohort, cursor, and pin across a deferred older reset and later same-source reset", async () => {
    const provisional = ["A", "B", "C"].map((label, index) => sourceEvent(label, true, index + 1));
    const recent = Array.from({ length: 80 }, (_, index) =>
      sourceEvent("recent-" + String(index + 1).padStart(3, "0"), true, index + 4));
    const cohort = [...provisional, ...recent];
    const initial = page({ progress: "provisional", events: provisional, next_before: "older-cursor" });
    const recentPage = page({
      revision: "recent-revision", progress: "provisional", events: recent, next_before: "older-cursor",
    });
    const olderReset = page({
      revision: "older-reset-revision", reset: true, progress: "provisional", events: recent,
      next_before: "older-reset-page-cursor",
    });
    const laterReset = page({
      revision: "later-reset-revision", reset: true, progress: "ready",
      events: recent.map(withoutProvisionalAdmission), next_before: "later-reset-page-cursor",
    });
    const olderResponse = deferred<AgentChatPage>();
    const olderRequested = signal();
    const laterResponse = deferred<AgentChatPage>();
    const laterRequested = signal();
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      if (request.revalidateIds !== undefined) return syntheticRenewal(request, cohort);
      if (request.cursor) { olderRequested.resolve(); return olderResponse.promise; }
      if (request.revision === undefined) return initial;
      if (request.revision === initial.revision) return recentPage;
      if (request.revision === olderReset.revision) {
        laterRequested.resolve();
        return laterResponse.promise;
      }
      throw new Error("Unexpected synthetic page request");
    });
    const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });

    await waitForRevision(hook, initial.revision);
    hook.rerender({ reload: 1 });
    await waitForRevision(hook, recentPage.revision);
    const beforeOlder = snapshot(hook.result.current);
    await act(async () => {
      const olderRead = hook.result.current.loadOlder();
      await olderRequested.promise;
      olderResponse.resolve(olderReset);
      await olderRead;
    });
    const afterOlder = snapshot(hook.result.current);

    hook.rerender({ reload: 2 });
    await laterRequested.promise;
    await act(async () => {
      laterResponse.resolve(laterReset);
      await Promise.resolve();
    });
    await waitForRevision(hook, laterReset.revision);
    await waitFor(() => expect(hook.result.current.error).toBeNull());
    const afterLaterReset = snapshot(hook.result.current);
    const expectedIds = cohort.map((event) => event.metadata.chat_source_ref as string).sort();
    const renewalSummary = (revision: string) => {
      const requests = loader.mock.calls.map(([request]) => request)
        .filter((request) => request.revision === revision && request.revalidateIds !== undefined);
      return {
        batchSizes: requests.map((request) => request.revalidateIds?.length ?? 0),
        sourceIds: [...new Set(requests.flatMap((request) => request.revalidateIds ?? []))].sort(),
      };
    };
    const expectedBatches = [...Array(10).fill(8), 3];
    expect({ beforeOlder, afterOlder, afterLaterReset }).toEqual({
      beforeOlder: expectedSnapshot(cohort, "older-cursor", false, recentPage.revision),
      afterOlder: expectedSnapshot(cohort, "older-cursor", true, olderReset.revision),
      afterLaterReset: expectedSnapshot(cohort, "older-cursor", true, laterReset.revision),
    });
    expect(renewalSummary(olderReset.revision)).toEqual({ batchSizes: expectedBatches, sourceIds: expectedIds });
    expect(renewalSummary(laterReset.revision)).toEqual({ batchSizes: expectedBatches, sourceIds: expectedIds });
    hook.unmount();
  });

  it("preserves an ordinary older page and its cursor through a subsequent same-scope reset", async () => {
    const recent = [sourceEvent("control-recent-1"), sourceEvent("control-recent-2", false, 2)];
    const older = sourceEvent("control-older", false, 1);
    const cohort = [...recent, older];
    const initial = page({ events: recent, next_before: "control-before-1" });
    const olderPage = page({ revision: "control-older-revision", events: [older], next_before: "control-before-2" });
    const resetRows = [...recent, ...Array.from({ length: 78 }, (_, index) =>
      sourceEvent("control-reset-" + String(index + 1), false, index + 3))];
    const resetPage = page({
      revision: "control-reset-revision", reset: true, events: resetRows, next_before: "reset-before",
    });
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      if (request.revalidateIds !== undefined) return syntheticRenewal(request, cohort);
      if (request.cursor) return olderPage;
      if (request.revision === undefined) return initial;
      if (request.revision === olderPage.revision) return resetPage;
      throw new Error("Unexpected synthetic page request");
    });
    const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });
    await waitForRevision(hook, initial.revision);
    await act(async () => { await hook.result.current.loadOlder(); });
    hook.rerender({ reload: 1 });
    await waitForRevision(hook, resetPage.revision);
    expect(snapshot(hook.result.current)).toEqual(
      expectedSnapshot(cohort, "control-before-2", true, resetPage.revision),
    );
    hook.unmount();
  });

  it("retires the old window and pin when an older reset changes generation", async () => {
    const initial = page({
      events: [sourceEvent("changed-old-1"), sourceEvent("changed-old-2", false, 2)],
      next_before: "changed-before",
    });
    const replacement = page({
      generation: "replacement-generation", revision: "replacement-revision", reset: true,
      events: [sourceEvent("replacement")], next_before: "replacement-before",
    });
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      if (request.cursor) return replacement;
      if (request.revision === undefined) return initial;
      throw new Error("Unexpected synthetic page request");
    });
    const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });
    await waitForRevision(hook, initial.revision);
    await act(async () => { await hook.result.current.loadOlder(); });
    expect(snapshot(hook.result.current)).toEqual(
      expectedSnapshot(replacement.events, "replacement-before", false, replacement.revision),
    );
    hook.unmount();
  });

  it("keeps same-admission provisional rows on a recent reset, then evicts them on a no-admission reset", async () => {
    const provisional = ["A", "B", "C"].map((label, index) => sourceEvent(label, true, index + 1));
    const recent = Array.from({ length: 80 }, (_, index) =>
      sourceEvent("recent-only-" + String(index + 1).padStart(3, "0"), true, index + 4));
    const initial = page({ progress: "provisional", events: provisional, next_before: "recent-only-before" });
    const ordinaryRecent = page({
      revision: "recent-only-page", progress: "provisional", events: recent, next_before: "recent-only-before",
    });
    const resetRecent = page({
      revision: "recent-only-reset", reset: true, progress: "provisional",
      events: recent, next_before: "reset-recent-before",
    });
    const noAdmissionReset = page({
      revision: "recent-only-canonical-reset", reset: true, progress: "ready",
      events: recent.map(withoutProvisionalAdmission), next_before: "canonical-recent-before",
    });
    const loader = vi.fn<ChatPageLoader>(async (request) => {
      if (request.revalidateIds !== undefined) return syntheticRenewal(request, [...provisional, ...recent]);
      if (request.revision === undefined) return initial;
      if (request.revision === initial.revision) return ordinaryRecent;
      if (request.revision === ordinaryRecent.revision) return resetRecent;
      if (request.revision === resetRecent.revision) return noAdmissionReset;
      throw new Error("Unexpected synthetic page request");
    });
    const hook = renderHook(({ reload }) => useChatPages(SESSION, loader, 60_000, reload), {
      initialProps: { reload: 0 },
    });
    await waitForRevision(hook, initial.revision);
    hook.rerender({ reload: 1 });
    await waitForRevision(hook, ordinaryRecent.revision);
    hook.rerender({ reload: 2 });
    await waitForRevision(hook, resetRecent.revision);
    expect(snapshot(hook.result.current)).toEqual(
      expectedSnapshot([...provisional, ...recent], "reset-recent-before", false, resetRecent.revision),
    );
    hook.rerender({ reload: 3 });
    await waitForRevision(hook, noAdmissionReset.revision);
    expect(snapshot(hook.result.current)).toEqual(
      expectedSnapshot(recent, "canonical-recent-before", false, noAdmissionReset.revision),
    );
    hook.unmount();
  });
});
