import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, beforeEach } from "vitest";
import { DocsLink } from "./DocsLink";
import { OnboardingHint } from "./OnboardingHint";

describe("DocsLink", () => {
  it("builds public docs links for in-app help", () => {
    render(<DocsLink path="/guide/getting-started">Getting Started</DocsLink>);

    expect(screen.getByRole("link", { name: /getting started/i })).toHaveAttribute(
      "href",
      "https://docs.wardian.org/guide/getting-started",
    );
    expect(screen.getByRole("link", { name: /getting started/i })).toHaveAttribute("target", "_blank");
  });
});

describe("OnboardingHint", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("stores dismissed hints locally so they do not repeat", async () => {
    const user = userEvent.setup();

    const { rerender } = render(
      <OnboardingHint id="spawn-agent" title="Start here">
        Verify a provider before spawning.
      </OnboardingHint>,
    );

    expect(screen.getByText("Start here")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /dismiss start here/i }));

    rerender(
      <OnboardingHint id="spawn-agent" title="Start here">
        Verify a provider before spawning.
      </OnboardingHint>,
    );

    expect(screen.queryByText("Start here")).not.toBeInTheDocument();
    expect(localStorage.getItem("wardian:onboarding-hint:spawn-agent")).toBe("dismissed");
  });

  it("does not render an already-dismissed hint on first paint", () => {
    localStorage.setItem("wardian:onboarding-hint:spawn-agent", "dismissed");

    render(
      <OnboardingHint id="spawn-agent" title="Start here">
        Verify a provider before spawning.
      </OnboardingHint>,
    );

    expect(screen.queryByText("Start here")).not.toBeInTheDocument();
  });
});
