import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { captureChatScrollAnchor, restoreChatScrollAnchor, useChatHandoffAnchor } from "./chatScrollAnchor";

describe("bounded Chat scroll anchor", () => {
  it.each(["prompt", "work"])("restores the exact %s event when legacy row roots become one shared turn", (kind) => {
    const scroll = document.createElement("div");
    scroll.scrollTop = 100;
    Object.defineProperty(scroll, "scrollHeight", { value: 1000 });
    Object.defineProperty(scroll, "clientHeight", { value: 200 });
    scroll.getBoundingClientRect = () => new DOMRect(0, 0, 200, 200);
    const legacy = document.createElement("div");
    legacy.dataset.chatRowKey = kind === "prompt" ? "exact-prompt" : "old-work-group";
    legacy.getBoundingClientRect = () => new DOMRect(0, 10, 200, 50);
    if (kind === "work") {
      const summary = document.createElement("button");
      summary.dataset.chatAnchorKeys = JSON.stringify(["exact-call", "exact-result"]);
      summary.getBoundingClientRect = legacy.getBoundingClientRect;
      legacy.append(summary);
    }
    scroll.append(legacy);
    const view = renderHook(({ rows }) => useChatHandoffAnchor({ current: scroll }, "same-admission", rows), { initialProps: { rows: "legacy" } });
    const shared = document.createElement("section");
    shared.dataset.chatRowKey = "canonical-turn";
    shared.getBoundingClientRect = () => new DOMRect(0, -80, 200, 300);
    const anchor = document.createElement("div");
    if (kind === "prompt") anchor.dataset.chatAnchorKey = "exact-prompt";
    else anchor.dataset.chatAnchorKeys = JSON.stringify(["exact-call"]);
    anchor.getBoundingClientRect = () => new DOMRect(0, 70, 200, 44);
    shared.append(anchor);
    act(() => { scroll.replaceChildren(shared); view.rerender({ rows: "shared" }); });
    expect(scroll.scrollTop).toBe(160);
    view.unmount();
  });

  it("chooses the visible prompt ahead of later work anchors, and ignores collapsed zero-height anchors", () => {
    const scroll = document.createElement("div");
    scroll.getBoundingClientRect = () => new DOMRect(0, 0, 200, 200);
    const prompt = document.createElement("div");
    prompt.dataset.chatRowKey = "visible-prompt";
    prompt.getBoundingClientRect = () => new DOMRect(0, 5, 200, 35);
    const work = document.createElement("div");
    work.dataset.chatAnchorKeys = JSON.stringify(["later-work"]);
    work.getBoundingClientRect = () => new DOMRect(0, 50, 200, 40);
    scroll.append(prompt, work);
    expect(captureChatScrollAnchor(scroll).eventKeys).toEqual(["visible-prompt"]);
    prompt.getBoundingClientRect = () => new DOMRect(0, -100, 200, 35);
    expect(captureChatScrollAnchor(scroll).eventKeys).toEqual(["later-work"]);
    work.getBoundingClientRect = () => new DOMRect(0, 50, 0, 0);
    expect(captureChatScrollAnchor(scroll).eventKeys).toEqual([]);
  });

  it("keeps the visible row fixed when older prepend evicts equal-height newer rows", () => {
    const scroll = document.createElement("div");
    const row = document.createElement("div");
    row.dataset.chatRowKey = "stable-observation-slot";
    scroll.append(row);
    scroll.scrollTop = 100;
    Object.defineProperty(scroll, "scrollHeight", { value: 1000 });
    scroll.getBoundingClientRect = () => new DOMRect(0, 0, 200, 200);
    row.getBoundingClientRect = () => new DOMRect(0, 10, 200, 50);
    const anchor = captureChatScrollAnchor(scroll);
    row.getBoundingClientRect = () => new DOMRect(0, 90, 200, 50);
    restoreChatScrollAnchor(scroll, anchor);
    expect(scroll.scrollTop).toBe(180);
    expect(scroll.scrollHeight).toBe(1000);
  });
});
