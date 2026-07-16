import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { AppShell } from "./AppShell";

describe("AppShell", () => {
  it("locks persistent content without disabling window chrome during reset", () => {
    render(
      <AppShell
        contentBusy
        titlebar={<button type="button">Close window</button>}
        leftRail={<button type="button">Rail action</button>}
        leftPane={<div>Left pane</div>}
        mainContent={<div>Workbench</div>}
        roster={<div>Roster</div>}
      />,
    );

    expect(screen.getByTestId("app-shell-content")).toHaveAttribute("inert", "");
    expect(screen.getByTestId("app-shell-content")).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("button", { name: "Close window" })).toBeEnabled();
  });

  it("raises the workbench into the center segment of the top chrome", () => {
    render(
      <AppShell
        titlebar={<div>Window chrome</div>}
        leftRail={<div>Rail</div>}
        leftPane={<div>Left pane</div>}
        mainContent={<div>Workbench</div>}
        roster={<div>Roster</div>}
      />,
    );

    expect(screen.getByTestId("app-shell-titlebar")).toHaveClass("app-shell-titlebar");
    expect(screen.getByTestId("app-shell-content")).toHaveClass("app-shell-content");
    expect(screen.getByTestId("app-shell-main")).toHaveClass("app-shell-workbench");
  });
});
