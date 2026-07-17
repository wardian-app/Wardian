import { describe, expect, it, vi } from "vitest";

import {
  fileResourceUrlConversion,
  fileResourceUrlForWebview,
} from "./resourceTicketUrl.mjs";

describe("fileResourceUrlForWebview", () => {
  it("decodes a Wardian ticket path and converts it exactly once", () => {
    const convert = vi.fn((path: string, protocol: string) => (
      `http://${protocol}.localhost/${encodeURIComponent(path)}`
    ));

    expect(fileResourceUrlConversion(
      "wardian-resource://localhost/ticket%20with%20spaces",
    )).toEqual({ path: "ticket with spaces", protocol: "wardian-resource" });
    expect(fileResourceUrlForWebview(
      "wardian-resource://localhost/ticket%20with%20spaces",
      convert,
    )).toBe("http://wardian-resource.localhost/ticket%20with%20spaces");
    expect(convert).toHaveBeenCalledOnce();
    expect(convert).toHaveBeenCalledWith("ticket with spaces", "wardian-resource");
  });

  it.each([
    "https://example.test/ticket",
    "http://wardian-resource.localhost/already-converted",
    "blob:https://example.test/id",
    "data:image/png;base64,AA==",
    "not a URL",
    "wardian-resource://other-host/ticket",
    "wardian-resource://localhost/%E0%A4%A",
  ])("preserves a non-convertible renderer URL: %s", (url) => {
    const convert = vi.fn();
    expect(fileResourceUrlForWebview(url, convert)).toBe(url);
    expect(convert).not.toHaveBeenCalled();
  });
});
