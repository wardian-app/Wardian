import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { describe, expect, it, beforeEach, vi } from "vitest";
import { DocsLink } from "./DocsLink";
import { OnboardingHint } from "./OnboardingHint";
import { useOnboardingStore } from "../store/useOnboardingStore";

const mockInvoke = vi.mocked(invoke);

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
    vi.clearAllMocks();
    localStorage.clear();
    useOnboardingStore.setState({
      dismissedHintIds: [],
      contextualTipsEnabled: true,
      hintsLoaded: false,
    });

    mockInvoke.mockImplementation(async (command, args) => {
      if (command === "load_onboarding_hints") {
        return { dismissed_hint_ids: [], contextual_tips_enabled: true };
      }
      if (command === "dismiss_onboarding_hint") {
        return {
          dismissed_hint_ids: [(args as { hintId: string }).hintId],
          contextual_tips_enabled: true,
        };
      }
      return null;
    });
  });

  it("loads dismissed hints from Wardian state before rendering", async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === "load_onboarding_hints") {
        return { dismissed_hint_ids: ["spawn-agent-first-run:v1"], contextual_tips_enabled: true };
      }
      return null;
    });

    render(
      <OnboardingHint id="spawn-agent-first-run:v1" title="Start here">
        Verify a provider before spawning.
      </OnboardingHint>,
    );

    expect(screen.queryByText("Start here")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("load_onboarding_hints");
      expect(useOnboardingStore.getState().hintsLoaded).toBe(true);
    });
    expect(screen.queryByText("Start here")).not.toBeInTheDocument();
  });

  it("stores dismissed hints in Wardian state so they do not repeat", async () => {
    const user = userEvent.setup();

    const { rerender } = render(
      <OnboardingHint id="spawn-agent-first-run:v1" title="Start here">
        Verify a provider before spawning.
      </OnboardingHint>,
    );

    expect(await screen.findByText("Start here")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /dismiss start here/i }));

    rerender(
      <OnboardingHint id="spawn-agent-first-run:v1" title="Start here">
        Verify a provider before spawning.
      </OnboardingHint>,
    );

    expect(screen.queryByText("Start here")).not.toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledWith("dismiss_onboarding_hint", {
      hintId: "spawn-agent-first-run:v1",
    });
    expect(localStorage.getItem("wardian:onboarding-hint:spawn-agent-first-run:v1")).toBeNull();
  });

  it("does not render an already-dismissed hint from hydrated state", () => {
    useOnboardingStore.setState({
      dismissedHintIds: ["spawn-agent-first-run:v1"],
      contextualTipsEnabled: true,
      hintsLoaded: true,
    });
    render(
      <OnboardingHint id="spawn-agent-first-run:v1" title="Start here">
        Verify a provider before spawning.
      </OnboardingHint>,
    );

    expect(screen.queryByText("Start here")).not.toBeInTheDocument();
  });

  it("hides every hint when contextual tips are disabled", () => {
    useOnboardingStore.setState({
      dismissedHintIds: [],
      contextualTipsEnabled: false,
      hintsLoaded: true,
    });

    render(
      <OnboardingHint id="spawn-agent-first-run:v1" title="Start here">
        Verify a provider before spawning.
      </OnboardingHint>,
    );

    expect(screen.queryByText("Start here")).not.toBeInTheDocument();
  });
});
