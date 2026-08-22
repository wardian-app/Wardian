import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ProviderModelSelector } from "./ProviderModelSelector";

const invokeMock = vi.mocked(invoke);

describe("ProviderModelSelector", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("uses live provider model capabilities to keep effort compatible with the selected model", async () => {
    invokeMock.mockResolvedValue({
      provider: "codex",
      version: "codex-cli 0.146.0",
      source: "live_catalog",
      refresh_error: null,
      models: [
        {
          id: "gpt-5.6-sol",
          display_name: "GPT-5.6 Sol",
          effort_options: ["low", "high"],
          default_effort: "low",
          is_default: true,
        },
        {
          id: "gpt-5.6-mini",
          display_name: "GPT-5.6 Mini",
          effort_options: ["low"],
          default_effort: "low",
          is_default: false,
        },
      ],
    });
    const onSelectionChange = vi.fn();
    const user = userEvent.setup();

    render(
      <ProviderModelSelector
        idPrefix="test"
        provider="codex"
        selection={{ model: "gpt-5.6-sol", reasoning_effort: "high" }}
        onSelectionChange={onSelectionChange}
      />,
    );

    await screen.findByRole("option", { name: "GPT-5.6 Mini" });
    await user.selectOptions(screen.getByLabelText("Model"), "gpt-5.6-mini");

    expect(onSelectionChange).toHaveBeenCalledWith({
      model: "gpt-5.6-mini",
      reasoning_effort: "low",
    });
  });

  it("does not expose a manual catalogue refresh control", async () => {
    invokeMock.mockResolvedValue({
      provider: "opencode",
      version: "1.18.3",
      source: "live_catalog",
      refresh_error: null,
      models: [],
    });
    render(
      <ProviderModelSelector
        idPrefix="refresh"
        provider="opencode"
        selection={{}}
        onSelectionChange={() => {}}
      />,
    );

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("list_provider_model_catalog", {
      provider: "opencode",
      forceRefresh: false,
    }));
    expect(screen.queryByRole("button", { name: "Refresh models" })).not.toBeInTheDocument();
  });

  it("drops a superseded catalog load when the provider changes mid-flight", async () => {
    let resolveCodexLoad: (catalog: unknown) => void = () => {};
    invokeMock.mockImplementation((command, args) => {
      if (command !== "list_provider_model_catalog") return Promise.reject(new Error("unexpected"));
      if ((args as { provider: string }).provider === "codex") {
        return new Promise((resolve) => {
          resolveCodexLoad = resolve;
        });
      }
      return Promise.resolve({
        provider: "claude",
        version: "claude-code 2.1",
        source: "provider_aliases",
        refresh_error: null,
        models: [
          { id: "claude-sonnet", display_name: "Sonnet", effort_options: [], default_effort: null, is_default: true },
        ],
      });
    });

    const { rerender } = render(
      <ProviderModelSelector
        idPrefix="race"
        provider="codex"
        selection={{}}
        onSelectionChange={() => {}}
      />,
    );
    rerender(
      <ProviderModelSelector
        idPrefix="race"
        provider="claude"
        selection={{}}
        onSelectionChange={() => {}}
      />,
    );

    await screen.findByRole("option", { name: "Sonnet" });

    // The slow codex discovery resolves after the switch; it must not land.
    await act(async () => {
      resolveCodexLoad({
        provider: "codex",
        version: "codex-cli 0.146.0",
        source: "live_catalog",
        refresh_error: null,
        models: [
          { id: "gpt-5.6-sol", display_name: "GPT-5.6 Sol", effort_options: ["low"], default_effort: "low", is_default: true },
        ],
      });
    });

    expect(screen.getByRole("option", { name: "Sonnet" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "GPT-5.6 Sol" })).not.toBeInTheDocument();
  });

  it("does not expose Codex's hidden Work Mode alias", async () => {
    invokeMock.mockResolvedValue({
      provider: "codex",
      version: "codex-cli 0.147.0",
      source: "live_catalog",
      refresh_error: null,
      models: [
        {
          id: "gpt-5.6-sol",
          display_name: "GPT-5.6 Sol",
          effort_options: ["low"],
          default_effort: "low",
          is_default: true,
        },
        {
          id: "gpt-5.6-sol-wm",
          display_name: "GPT-5.6 Sol WM",
          effort_options: ["low"],
          default_effort: "low",
          is_default: false,
        },
      ],
    });

    render(
      <ProviderModelSelector
        idPrefix="hidden-work-mode"
        provider="codex"
        selection={{}}
        onSelectionChange={() => {}}
      />,
    );

    await screen.findByRole("option", { name: "GPT-5.6 Sol" });
    expect(screen.queryByRole("option", { name: "GPT-5.6 Sol WM" })).not.toBeInTheDocument();
  });

  it("refreshes an open selector every five minutes", async () => {
    vi.useFakeTimers();
    invokeMock.mockResolvedValue({
      provider: "gemini",
      version: "0.42.0",
      source: "provider_aliases",
      refresh_error: null,
      models: [],
    });

    try {
      render(
        <ProviderModelSelector
          idPrefix="automatic-refresh"
          provider="gemini"
          selection={{}}
          onSelectionChange={() => {}}
        />,
      );

      await act(async () => {
        await vi.advanceTimersByTimeAsync(5 * 60 * 1000);
      });

      expect(invokeMock).toHaveBeenCalledWith("list_provider_model_catalog", {
        provider: "gemini",
        forceRefresh: true,
      });
    } finally {
      vi.useRealTimers();
    }
  });
});
