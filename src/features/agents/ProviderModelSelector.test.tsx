import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ProviderModelSelector } from "./ProviderModelSelector";

const invokeMock = vi.mocked(invoke);

describe("ProviderModelSelector", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("defers discovery and its timer until explicit activation and blocks read-only activation", async () => {
    invokeMock.mockResolvedValue({ provider: "codex", models: [], refresh_error: null });
    const timer = vi.spyOn(window, "setInterval");
    const props = { idPrefix: "history", provider: "codex", selection: { model: "saved-model" }, onSelectionChange: vi.fn(), deferDiscovery: true };
    const view = render(<ProviderModelSelector {...props} />);
    const picker = screen.getByRole("button", { name: "Choose model" });
    expect(picker).toHaveTextContent("saved-model");
    picker.focus();
    expect(invokeMock).not.toHaveBeenCalled();
    expect(timer).not.toHaveBeenCalled();
    const user = userEvent.setup();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("list_provider_model_catalog", { provider: "codex", forceRefresh: false }));
    expect(timer.mock.calls.filter(([, delay]) => delay === 5 * 60 * 1000)).toHaveLength(1);
    timer.mockRestore();
    view.rerender(<ProviderModelSelector {...props} disabled />);
    expect(screen.getByRole("button", { name: "Choose model" })).toBeDisabled();
    const discoveryCalls = invokeMock.mock.calls.length;
    await user.click(screen.getByRole("button", { name: "Choose model" }));
    expect(invokeMock.mock.calls).toHaveLength(discoveryCalls);
  });

  // #1626 is stacked on #1615: these exercise its explicitly activated deferred picker.
  const deferredProps = { idPrefix: "deferred-focus", provider: "codex", selection: {}, onSelectionChange: vi.fn(), deferDiscovery: true };
  const discoveredCatalog = {
    provider: "codex", version: "test", source: "live_catalog", refresh_error: null,
    models: [{ id: "test-model", display_name: "Test model", effort_options: [], default_effort: null, is_default: true }],
  };

  function delayCatalog() {
    let resolve: (catalog: unknown) => void = () => {};
    let reject: (reason: Error) => void = () => {};
    invokeMock.mockReturnValueOnce(new Promise((resolvePromise, rejectPromise) => {
      resolve = resolvePromise;
      reject = rejectPromise;
    }));
    return { resolve, reject };
  }

  it.each(["{Enter}", " "])("transfers focus after delayed deferred discovery activated with %s", async (key) => {
    const pending = delayCatalog();
    const user = userEvent.setup();
    render(<ProviderModelSelector {...deferredProps} />);
    screen.getByRole("button", { name: "Choose model" }).focus();
    expect(invokeMock).not.toHaveBeenCalled();
    await user.keyboard(key);
    const selector = screen.getByRole("combobox", { name: "Model" });
    expect(selector).toBeDisabled();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    // Repeating activation while the original request is pending must not rediscover.
    await user.keyboard(key);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    await act(async () => pending.resolve(discoveredCatalog));
    expect(selector).toBeEnabled();
    expect(selector).toHaveFocus();
  });

  it.each(["tab", "focus", "pointer", "away-and-back"])("leaves moved-away focus alone during deferred discovery (%s)", async (interaction) => {
    const pending = delayCatalog();
    const user = userEvent.setup();
    render(<><ProviderModelSelector {...deferredProps} /><button>Elsewhere</button><div data-testid="blank">Blank area</div></>);
    screen.getByRole("button", { name: "Choose model" }).focus();
    await user.keyboard("{Enter}");
    const elsewhere = screen.getByRole("button", { name: "Elsewhere" });
    if (interaction === "tab") await user.tab();
    if (interaction === "focus" || interaction === "away-and-back") elsewhere.focus();
    if (interaction === "pointer" || interaction === "away-and-back") await user.click(screen.getByTestId("blank"));
    const focusedBeforeResolution = document.activeElement;
    await act(async () => pending.resolve(discoveredCatalog));
    expect(screen.getByRole("combobox", { name: "Model" })).toBeEnabled();
    expect(document.activeElement).toBe(focusedBeforeResolution);
    expect(screen.getByRole("combobox", { name: "Model" })).not.toHaveFocus();
  });

  it.each(["provider", "identity", "disabled"])("retires deferred focus intent when %s changes during discovery", async (change) => {
    const pending = delayCatalog();
    const user = userEvent.setup();
    const view = render(<ProviderModelSelector {...deferredProps} />);
    screen.getByRole("button", { name: "Choose model" }).focus();
    await user.keyboard("{Enter}");
    const nextProps = {
      ...deferredProps,
      provider: change === "provider" ? "claude" : deferredProps.provider,
      idPrefix: change === "identity" ? "different-session" : deferredProps.idPrefix,
      disabled: change === "disabled",
    };
    view.rerender(<ProviderModelSelector {...nextProps} />);
    await act(async () => pending.resolve(discoveredCatalog));
    expect(document.activeElement).toBe(document.body);
    if (change === "provider" || change === "disabled") {
      expect(screen.getByRole("button", { name: "Choose model" })).toBeInTheDocument();
      expect(screen.queryByRole("option", { name: "Test model" })).not.toBeInTheDocument();
    }
  });

  it("keeps repeated activations independent after an interrupted request", async () => {
    const first = delayCatalog();
    const second = delayCatalog();
    const user = userEvent.setup();
    const view = render(<ProviderModelSelector {...deferredProps} />);
    screen.getByRole("button", { name: "Choose model" }).focus();
    await user.keyboard("{Enter}");
    view.rerender(<ProviderModelSelector {...deferredProps} disabled />);
    view.rerender(<ProviderModelSelector {...deferredProps} />);
    // The parent's activated provider resumes discovery once read-only mode ends.
    expect(invokeMock).toHaveBeenCalledTimes(2);
    await act(async () => first.resolve(discoveredCatalog));
    expect(screen.getByRole("combobox", { name: "Model" })).toBeDisabled();
    await act(async () => second.resolve(discoveredCatalog));
    expect(screen.getByRole("combobox", { name: "Model" })).toBeEnabled();
    expect(screen.getByRole("combobox", { name: "Model" })).not.toHaveFocus();
  });

  it.each(["empty", "error"])("does not defer a focus jump until an automatic retry after an initial %s result", async (result) => {
    vi.useFakeTimers();
    try {
      const pending = delayCatalog();
      invokeMock.mockResolvedValue(discoveredCatalog);
      render(<ProviderModelSelector {...deferredProps} />);
      const trigger = screen.getByRole("button", { name: "Choose model" });
      trigger.focus();
      fireEvent.click(trigger);
      await act(async () => {
        if (result === "empty") pending.resolve({ ...discoveredCatalog, models: [] });
        else pending.reject(new Error("Catalog unavailable"));
      });
      expect(screen.getByRole("combobox", { name: "Model" })).toBeDisabled();
      await act(async () => vi.advanceTimersByTimeAsync(5 * 60 * 1000));
      expect(screen.getByRole("combobox", { name: "Model" })).toBeEnabled();
      expect(screen.getByRole("combobox", { name: "Model" })).not.toHaveFocus();
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not steal focus on refresh after transferring it once", async () => {
    vi.useFakeTimers();
    try {
      const pending = delayCatalog();
      invokeMock.mockResolvedValue(discoveredCatalog);
      render(<><ProviderModelSelector {...deferredProps} /><button>Elsewhere</button></>);
      const trigger = screen.getByRole("button", { name: "Choose model" });
      trigger.focus();
      fireEvent.click(trigger);
      await act(async () => pending.resolve(discoveredCatalog));
      expect(screen.getByRole("combobox", { name: "Model" })).toHaveFocus();
      screen.getByRole("button", { name: "Elsewhere" }).focus();
      await act(async () => vi.advanceTimersByTimeAsync(5 * 60 * 1000));
      expect(screen.getByRole("button", { name: "Elsewhere" })).toHaveFocus();
    } finally {
      vi.useRealTimers();
    }
  });

  it("retires pending focus intent on unmount", async () => {
    const pending = delayCatalog();
    const user = userEvent.setup();
    const view = render(<ProviderModelSelector {...deferredProps} />);
    screen.getByRole("button", { name: "Choose model" }).focus();
    await user.keyboard("{Enter}");
    view.unmount();
    render(<button>Replacement</button>);
    screen.getByRole("button", { name: "Replacement" }).focus();
    await act(async () => pending.resolve(discoveredCatalog));
    expect(screen.getByRole("button", { name: "Replacement" })).toHaveFocus();
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
