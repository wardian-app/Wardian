import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { committedChatFixture } from "../../test/committedChatFixture";
import { useChatPages, type ChatPageLoader } from "./useChatPages";
import type { AgentChatPage } from "../../types";

describe("canonical step detail reader", () => {
  it("uses the bounded presentation detail while retaining the legacy detail API", async () => {
    const { page, presentation, detail } = committedChatFixture();
    const loader = vi.fn<ChatPageLoader>().mockImplementation(async ({ detailRef }) => detailRef
      ? { ...page, events: [], detail: { event_id: "read", text: "legacy", next: null, complete: true },
        presentation: { version: 1, payload: { ...presentation, detail } } } : page);
    const view = renderHook(() => useChatPages(page.session_id, loader, 60_000, 0));
    await waitFor(() => expect(view.result.current.page).toEqual(page));
    await act(async () => { expect(await view.result.current.loadStepDetail("step:read")).toEqual(detail); });
    await act(async () => { expect(await view.result.current.loadDetail("legacy:read")).toMatchObject({ text: "legacy" }); });
    expect(view.result.current.events).toHaveLength(5);
    view.unmount();
  });

  it("rejects a changed revision, unsupported detail, and a retired conversation", async () => {
    const { page, presentation, detail } = committedChatFixture();
    let reply: AgentChatPage = { ...page, events: [], revision: "changed", presentation: { version: 1, payload: { ...presentation, detail } } };
    let resolve: ((page: AgentChatPage) => void) | null = null;
    const loader = vi.fn<ChatPageLoader>().mockImplementation(async ({ detailRef }) => detailRef === "waiting"
      ? new Promise<AgentChatPage>((done) => { resolve = done; }) : detailRef ? reply : page);
    const view = renderHook(() => useChatPages(page.session_id, loader, 60_000, 0));
    await waitFor(() => expect(view.result.current.page).toEqual(page));
    await act(async () => { await expect(view.result.current.loadStepDetail("step:read")).rejects.toThrow(); });
    reply = { ...reply, revision: page.revision, presentation: { version: 2, payload: null } };
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    await act(async () => { await expect(view.result.current.loadStepDetail("step:read")).rejects.toThrow(); });
    warning.mockRestore();
    let pending!: Promise<unknown>;
    await act(async () => { pending = view.result.current.loadStepDetail("waiting").catch((error) => error); });
    await act(async () => { view.result.current.reset(); });
    await act(async () => { resolve!({ ...page, presentation: { version: 1, payload: { ...presentation, detail } } }); });
    expect(await pending).toBeInstanceOf(Error);
    view.unmount();
  });
});
