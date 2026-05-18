import { readFileSync } from "node:fs";
import { cwd } from "node:process";
import { describe, expect, it } from "vitest";

function cssBlock(selector: string) {
  const css = readFileSync(`${cwd()}/src/styles/App.css`, "utf8");
  const escapedSelector = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = css.match(new RegExp(`${escapedSelector}\\s*\\{(?<body>[^}]*)\\}`));
  return match?.groups?.body ?? "";
}

describe("titlebar chrome", () => {
  it("does not draw vertical column seams in the app header", () => {
    expect(cssBlock(".titlebar-left")).not.toContain("border-right");
    expect(cssBlock(".titlebar-right")).not.toContain("border-left");
    expect(cssBlock('.titlebar-right:not([style*="--titlebar-right-width: 0px"])')).not.toContain("border-left");
  });
});
