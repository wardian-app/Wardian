import { describe, expect, it } from "vitest";
import { captureChatScrollAnchor, restoreChatScrollAnchor } from "./chatScrollAnchor";

describe("bounded Chat scroll anchor", () => {
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
