import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentChatPage } from "../../types";
import { chatPageObservation, createChatPageObservation, installChatPageObservation } from "./chatPageObservation";
import { loadChatPage } from "./chatPageLoader";

vi.unmock("@tauri-apps/api/core");

const page: AgentChatPage = {
  session_id: "owner", conversation_id: "conversation", generation: "generation", source_epoch: null,
  revision: "revision", events: [], next_before: null, unchanged: false, reset: false, progress: "ready",
  aliases: [], removed_ids: [], detail: null, bytes_read: 0, records_decoded: 0,
};
const disposers: Array<() => void> = [];
afterEach(() => { disposers.splice(0).forEach((dispose) => dispose()); vi.unstubAllGlobals(); });

describe("passive Chat page observation", () => {
  it("does not touch a Promise without a matching subscriber or retain a backlog", () => {
    const bridge = createChatPageObservation();
    const result = Promise.resolve(page);
    const then = vi.spyOn(result, "then");
    const args = { sessionId: "owner", recentPreview: true };
    bridge.notifyRequest(args, result);
    const onRequest = vi.fn();
    const subscription = bridge.facade.subscribe({ version: 1, owner: "other", onRequest });
    expect(subscription.active).toBe(true);
    bridge.notifyRequest(args, result);
    expect(then).not.toHaveBeenCalled();
    expect(onRequest).not.toHaveBeenCalled();
    expect(Object.keys(bridge.facade)).toEqual(["version", "subscribe"]);
  });

  it("acknowledges one versioned owner and incarnation, rejects bad/duplicate activation, and isolates faults", () => {
    const bridge = createChatPageObservation();
    const onRequest = vi.fn(() => { throw new Error("subscriber failure"); });
    expect(bridge.facade.subscribe({ version: 2, owner: "owner", onRequest })).toEqual({ active: false, reason: "incompatible_version" });
    expect(bridge.facade.subscribe({ version: 1, owner: "", onRequest })).toEqual({ active: false, reason: "invalid_subscription" });
    const subscription = bridge.facade.subscribe({ version: 1, owner: "owner", onRequest });
    if (!subscription.active) throw new Error("activation failed");
    expect(subscription).toMatchObject({ version: 1, owner: "owner", active: true, token: 1 });
    expect(bridge.facade.subscribe({ version: 1, owner: "owner", onRequest })).toEqual({ active: false, reason: "subscription_active" });
    const result = Promise.resolve(page);
    const args = { sessionId: "owner", recentPreview: true };
    expect(() => bridge.notifyRequest(args, result)).not.toThrow();
    expect(onRequest.mock.calls[0]).toEqual([args, result]);
    expect(subscription.status()).toEqual({ active: true, fault_count: 1, request_count: 1 });
    subscription.unsubscribe();
    const next = bridge.facade.subscribe({ version: 1, owner: "owner", onRequest: vi.fn() });
    if (!next.active) throw new Error("replacement activation failed");
    subscription.unsubscribe();
    expect(next.token).toBe(2);
    expect(next.status().active).toBe(true);
    expect(subscription.status().active).toBe(false);
    next.unsubscribe();
    bridge.notifyRequest(args, result);
    expect(onRequest).toHaveBeenCalledTimes(1);
  });

  it("isolates asynchronous subscriber faults and retires late failure callbacks", async () => {
    const bridge = createChatPageObservation();
    let reject!: (reason: unknown) => void;
    const pending = new Promise<void>((_resolve, no) => { reject = no; });
    const active = bridge.facade.subscribe({ version: 1, owner: "owner", onRequest: () => pending });
    if (!active.active) throw new Error("activation failed");
    bridge.notifyRequest({ sessionId: "owner", recentPreview: true }, Promise.resolve(page));
    active.unsubscribe();
    reject(new Error("late subscriber rejection"));
    await Promise.resolve();
    expect(active.status()).toEqual({ active: false, fault_count: 0, request_count: 1 });
    const next = bridge.facade.subscribe({ version: 1, owner: "owner", onRequest: async () => { throw new Error("active subscriber rejection"); } });
    if (!next.active) throw new Error("activation failed");
    bridge.notifyRequest({ sessionId: "owner", recentPreview: true }, Promise.resolve(page));
    await Promise.resolve();
    expect(next.status().fault_count).toBe(1);
    next.unsubscribe();
  });

  it.each(["pending", "success", "rejection", "synchronous failure"])("preserves the actual SDK Promise, arguments and %s outcome under immutable Tauri", async (outcome) => {
    const error = new Error("original failure");
    let resolve!: (value: AgentChatPage) => void;
    let reject!: (reason: unknown) => void;
    const inner = new Promise<AgentChatPage>((yes, no) => { resolve = yes; reject = no; });
    const original = vi.fn((_command: string, _args: unknown) => { if (outcome === "synchronous failure") throw error; return inner; });
    const internals = {};
    Object.defineProperty(internals, "invoke", { value: original, writable: false, configurable: false });
    const nativeWindow = {} as Window;
    Object.defineProperty(nativeWindow, "__TAURI_INTERNALS__", { value: internals, writable: false, configurable: false });
    vi.stubGlobal("window", nativeWindow);
    installChatPageObservation(nativeWindow);
    expect(nativeWindow.__wardianChatPageObservation).toBe(chatPageObservation.facade);
    const onRequest = vi.fn();
    const subscription = chatPageObservation.facade.subscribe({ version: 1, owner: "owner", onRequest });
    if (!subscription.active) throw new Error("activation failed");
    disposers.push(subscription.unsubscribe);
    const revalidateIds = ["id"];
    const request = { sessionId: "owner", cursor: "cursor", revision: "revision", detailRef: "detail", revalidateIds };
    const result = loadChatPage(request);
    expect(original).toHaveBeenCalledTimes(1);
    const [args, observed] = onRequest.mock.calls[0];
    expect(observed).toBe(result);
    expect(observed).not.toBe(inner); // The SDK is async; its returned Promise is the product boundary.
    expect(original.mock.calls[0][0]).toBe("load_agent_chat_page");
    expect(original.mock.calls[0][1]).toBe(args);
    expect(args.revalidateIds).toBe(revalidateIds);
    expect(args).toEqual({ ...request, recentPreview: false });
    expect(Object.isFrozen(args)).toBe(false);
    expect(Object.getOwnPropertyDescriptor(internals, "invoke")).toMatchObject({ value: original, writable: false, configurable: false });
    if (outcome === "pending") {
      const settled = vi.fn();
      void result.then(settled, settled);
      subscription.unsubscribe();
      await Promise.resolve();
      expect(settled).not.toHaveBeenCalled();
      resolve(page);
      expect(await result).toBe(page);
    } else if (outcome === "success") {
      resolve(page);
      expect(await result).toBe(page);
    } else {
      const assertion = expect(result).rejects.toBe(error);
      if (outcome === "rejection") reject(error);
      await assertion;
    }
  });
});
