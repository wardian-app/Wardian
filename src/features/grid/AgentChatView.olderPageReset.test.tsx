// SYNTHETIC_IPC_UNIT_FIXTURE: actual view, hook, and renewal validator with mock IPC transport only.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Image } from "@tauri-apps/api/image";
import { open } from "@tauri-apps/plugin-dialog";
import { readImage, writeImage, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatEvent, AgentChatPage } from "../../types";
import { chatPageInvokeFixture } from "../../test/chatPageTestFixture";
import { AgentChatView } from "./AgentChatView";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  readImage: vi.fn(), writeImage: vi.fn(), writeText: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/image", () => ({ Image: { fromPath: vi.fn() } }));

const invokeMock = chatPageInvokeFixture(vi.mocked(invoke));
const SESSION = "agent-1";
const EPOCH = "older-reset-dom-epoch";
const ADMISSION = "older-reset-dom-admission";

function page(overrides: Partial<AgentChatPage> = {}): AgentChatPage {
  return {
    session_id: SESSION, conversation_id: "older-reset-dom-conversation",
    generation: "older-reset-dom-generation", source_epoch: EPOCH,
    revision: "revision-0", events: [], next_before: null, unchanged: false,
    reset: false, older_page: false, progress: "ready", aliases: [], removed_ids: [],
    detail: null, bytes_read: 0, records_decoded: 0, ...overrides,
  };
}

function row(label: string, sequence: number, provisional = false): AgentChatEvent {
  return {
    id: `header-${label}`, session_id: SESSION, provider: "codex", kind: "message", role: "user",
    text: label === "A" ? "Row A" : `Row ${label}`, title: null, status: null, turn_id: null,
    source: null, command: null, exit_code: null, path: null, language: null, created_at: null, sequence,
    metadata: {
      chat_source_ref: `source:${label}`, chat_source_epoch: EPOCH, chat_source_admission: ADMISSION,
      ...(provisional ? { chat_provisional: true } : {}),
    },
  };
}

function transcriptButton(transcript: HTMLOListElement, label: string): HTMLButtonElement | null {
  return Array.from(transcript.querySelectorAll<HTMLButtonElement>(":scope > li > button"))
    .find((button) => button.textContent?.trim() === label) ?? null;
}
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    && !(value instanceof ArrayBuffer);
}

describe("SYNTHETIC_IPC_UNIT_FIXTURE accepted older reset in the transcript", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    vi.mocked(listen).mockReset();
    vi.mocked(listen).mockResolvedValue(() => {});
    vi.mocked(Image.fromPath).mockReset();
    vi.mocked(open).mockReset();
    vi.mocked(readImage).mockReset();
    vi.mocked(writeImage).mockReset();
    vi.mocked(writeText).mockReset();
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("follows reset and continuation cursors through terminal DOM paging", async () => {
    vi.useFakeTimers();
    const provisional = ["A", "B", "C"].map((label, index) => row(label, index + 1, true));
    const recent = Array.from({ length: 80 }, (_, index) =>
      row("recent-" + String(index + 1).padStart(3, "0"), index + 4, true));
    const firstOlder = Array.from({ length: 80 }, (_, index) =>
      row("reset-older-" + String(index + 1).padStart(3, "0"), index - 80));
    const secondOlder = Array.from({ length: 80 }, (_, index) =>
      row("follow-older-" + String(index + 1).padStart(3, "0"), index - 160));
    const allRows = [...provisional, ...recent, ...firstOlder, ...secondOlder];
    const sourceRows = new Map(allRows.map((event) => [event.metadata.chat_source_ref as string, event]));
    const initial = page({ revision: "dom-initial", progress: "provisional", events: provisional, next_before: "C0" });
    const recentPage = page({ revision: "dom-recent", progress: "provisional", events: recent, next_before: "C0" });
    const olderReset = page({
      revision: "dom-older-reset", reset: true, older_page: true,
      progress: "provisional", events: firstOlder, next_before: "C1",
    });
    const continuation = page({
      revision: "dom-older-continuation", older_page: true,
      progress: "ready", events: secondOlder, next_before: "C2",
    });
    const exhausted = page({ revision: "dom-older-exhausted", progress: "ready", next_before: null });
    const pageRequests: Record<string, unknown>[] = [];

    invokeMock.mockImplementation((command, args) => {
      if (command !== "load_agent_chat_page") return Promise.resolve(undefined);
      const request = isRecord(args) ? args : {};
      pageRequests.push(request);
      const rawIds = request.revalidateIds;
      if (Array.isArray(rawIds)) {
        const ids = rawIds.filter((id): id is string => typeof id === "string");
        const events = ids.map((id) => {
          const sourceRow = sourceRows.get(id);
          if (!sourceRow) throw new Error("Synthetic transport has no row for " + id);
          return sourceRow;
        });
        const revision = typeof request.revision === "string" ? request.revision : "";
        return Promise.resolve(page({
          revision, progress: "ready", events, revalidated_ids: [...ids],
          bytes_read: events.length * 64, records_decoded: events.length,
        }));
      }
      if (request.cursor === "C0") return Promise.resolve(olderReset);
      if (request.cursor === "C1") return Promise.resolve(continuation);
      if (request.cursor === "C2") return Promise.resolve(exhausted);
      if (request.revision === undefined) return Promise.resolve(initial);
      if (request.revision === initial.revision) return Promise.resolve(recentPage);
      throw new Error("Unexpected synthetic page request");
    });

    const view = render(
      <AgentChatView sessionId={SESSION} refreshIntervalMs={60_000} readOnly deferModelDiscovery />,
    );
    try {
      await act(async () => {});
      const transcript = view.container.querySelector<HTMLOListElement>('[data-testid="agent-chat-transcript"]');
      expect(transcript).not.toBeNull();
      if (!transcript) throw new Error("The transcript list was not rendered");
      const rowA = transcript.querySelector<HTMLElement>('[data-chat-row-key="header-A"]');
      expect(rowA).not.toBeNull();
      if (!rowA) throw new Error("The retained source row has no transcript key");
      const rowAText = rowA.textContent;

      await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
      expect(transcript.querySelector('[data-chat-row-key="header-recent-001"]')).not.toBeNull();
      expect(transcript.querySelectorAll("[data-chat-row-key]").length).toBe(83);
      const firstOlderButton = transcriptButton(transcript, "Load older transcript");
      expect(firstOlderButton).not.toBeNull();
      if (!firstOlderButton) throw new Error("The first Load older transcript control was not rendered");
      expect(firstOlderButton).toBeEnabled();

      await act(async () => {
        fireEvent.click(firstOlderButton);
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(transcript.querySelector('[data-chat-row-key="header-reset-older-001"]')).not.toBeNull();
      expect(transcript.querySelectorAll("[data-chat-row-key]").length).toBe(163);
      expect(transcript.querySelector('[data-chat-row-key="header-A"]')).toBe(rowA);
      expect(rowA.textContent).toBe(rowAText);
      const jumpToLatestButton = transcriptButton(transcript, "Jump to latest");
      expect(jumpToLatestButton).not.toBeNull();
      if (!jumpToLatestButton) throw new Error("The Jump to latest control was not rendered");
      const loadOlderButton = transcriptButton(transcript, "Load older transcript");
      expect(loadOlderButton).not.toBeNull();
      if (!loadOlderButton) throw new Error("The reset Load older transcript control was not rendered");
      expect(jumpToLatestButton).toBeEnabled();
      expect(loadOlderButton).toBeEnabled();

      await act(async () => {
        fireEvent.click(loadOlderButton);
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(transcript.querySelector('[data-chat-row-key="header-follow-older-001"]')).not.toBeNull();
      expect(transcript.querySelectorAll("[data-chat-row-key]").length).toBe(243);
      expect(transcript.querySelector('[data-chat-row-key="header-A"]')).toBe(rowA);
      expect(rowA.textContent).toBe(rowAText);
      expect(pageRequests.filter((request) => typeof request.cursor === "string").map((request) => request.cursor))
        .toEqual(["C0", "C1"]);
      expect(jumpToLatestButton.isConnected).toBe(true);
      expect(jumpToLatestButton).toBeEnabled();
      expect(loadOlderButton.isConnected).toBe(true);
      expect(loadOlderButton).toBeEnabled();

      await act(async () => {
        fireEvent.click(loadOlderButton);
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(transcript.querySelector('[data-chat-row-key="header-follow-older-001"]')).not.toBeNull();
      expect(transcript.querySelectorAll("[data-chat-row-key]").length).toBe(243);
      expect(transcript.querySelector('[data-chat-row-key="header-A"]')).toBe(rowA);
      expect(rowA.textContent).toBe(rowAText);
      expect(pageRequests.filter((request) => typeof request.cursor === "string").map((request) => request.cursor))
        .toEqual(["C0", "C1", "C2"]);
      expect(loadOlderButton.isConnected).toBe(false);
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });
});
