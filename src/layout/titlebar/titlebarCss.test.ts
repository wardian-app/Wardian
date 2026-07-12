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

  it("does not let an implicit right minimum cover Dockview pane actions", () => {
    expect(cssBlock(".titlebar-right")).not.toContain("min-width");
    const workbenchCss = readFileSync(`${cwd()}/src/layout/workbench/workbench.css`, "utf8");
    expect(workbenchCss).toContain('[data-right-collapsed="true"]');
    expect(workbenchCss).toContain('[data-window-controls-clearance="true"]');
    expect(workbenchCss).toContain("padding-right: var(--wardian-collapsed-right-chrome-width)");
  });
});
