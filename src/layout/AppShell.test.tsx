import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { AppShell, useAppShellWorkbenchNavigation } from "./AppShell";
import type { WorkbenchNavigationService } from "../features/workbench/navigationService";

const navigation = {} as WorkbenchNavigationService;

function NavigationProbe() {
  return <output>{useAppShellWorkbenchNavigation() === navigation ? "navigation ready" : "missing"}</output>;
}

describe("AppShell", () => {
  it("locks persistent content without disabling window chrome during reset", () => {
    render(
      <AppShell
        navigation={navigation}
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
        navigation={navigation}
        titlebar={<div>Window chrome</div>}
        leftRail={<div>Rail</div>}
        leftPane={<NavigationProbe />}
        mainContent={<div>Workbench</div>}
        roster={<div>Roster</div>}
      />,
    );

    expect(screen.getByTestId("app-shell-titlebar")).toHaveClass("app-shell-titlebar");
    expect(screen.getByTestId("app-shell-content")).toHaveClass("app-shell-content");
    expect(screen.getByTestId("app-shell-main")).toHaveClass("app-shell-workbench");
    expect(screen.getByText("navigation ready")).toBeInTheDocument();
  });
});
