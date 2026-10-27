import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  AgentConfig,
  AgentChatEvent,
  AgentChatPage,
  TerminalBrokerState,
  TerminalPresentationState,
} from "../../../types";
import {
  AgentSessionSurface,
  agentSessionPresentationId,
  type AgentSessionSurfaceProps,
} from "./AgentSessionSurface";

const terminalSpy = vi.hoisted(() => vi.fn());

vi.mock("../../terminal/AgentTerminal", () => ({
  AgentTerminal: (props: Record<string, unknown>) => {
    terminalSpy(props);
    return <div data-testid="agent-terminal" />;
  },
}));

const agent: AgentConfig = {
  session_id: "agent-1",
  session_name: "Mendel",
  agent_class: "Coder",
  folder: "/workspace/wardian",
  provider: "codex",
  is_off: false,
};
const replacementAgent: AgentConfig = {
  ...agent,
  session_id: "agent-2",
  session_name: "Curie",
};

function brokerState(overrides: Partial<TerminalBrokerState> = {}): TerminalBrokerState {
  return {
    session_id: "agent-1",
    runtime_generation: 1,
    lease_epoch: 2,
    stream_sequence: 3,
    interaction_sequence: 4,
    geometry: { cols: 100, rows: 30 },
    owner_presentation_id: null,
    pending_activation: null,
    runtime_state: "live",
    ...overrides,
  };
}

function presentationState(
  presentationId: string,
  overrides: Partial<TerminalPresentationState> = {},
): TerminalPresentationState {
  return {
    presentation_id: presentationId,
    client_kind: "desktop",
    desired_geometry: { cols: 100, rows: 30 },
    visibility: "visible",
    render_state: "mounted",
    interaction_capability: "interactive",
    interaction_sequence: 4,
    requires_resync: false,
    ...overrides,
  };
}

function surfaceProps(overrides: Partial<AgentSessionSurfaceProps> = {}): AgentSessionSurfaceProps {
  return {
    surface_id: "surface-7",
    resource_key: "agent-1",
    agent,
    theme: "dark",
    ...overrides,
  };
}

afterEach(() => {
  terminalSpy.mockClear();
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
});

describe("AgentSessionSurface", () => {
  function historyPage(sessionId: string, older = false): AgentChatPage {
    const message: AgentChatEvent = {
      id: `${sessionId}-${older ? "older" : "recent"}`, session_id: sessionId,
      provider: "codex", kind: "message", role: "assistant",
      text: `${sessionId} ${older ? "older" : "recent"} history`, title: null,
      status: null, turn_id: null, source: null, command: null, exit_code: null,
      path: null, language: null, created_at: null, sequence: older ? 1 : 2, metadata: {},
    };
    return {
      session_id: sessionId, conversation_id: `${sessionId}-conversation`,
      generation: "generation", source_epoch: null, revision: "revision",
      events: [message], next_before: older ? null : "older-cursor",
      unchanged: false, reset: false, progress: "ready", aliases: [], removed_ids: [],
      detail: null, bytes_read: 256, records_decoded: 1,
    };
  }

  function mockHistory() {
    vi.mocked(listen).mockResolvedValue(() => {});
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "load_agent_chat_page") {
        const request = args as { sessionId: string; cursor?: string };
        return historyPage(request.sessionId, Boolean(request.cursor));
      }
      if (command === "list_provider_model_catalog") {
        return { provider: "codex", models: [], refresh_error: null };
      }
      throw new Error(`Unexpected history side effect: ${command}`);
    });
  }

  it("opens Off history and older pages without mounting or starting a terminal", async () => {
    mockHistory();
    render(<AgentSessionSurface {...surfaceProps({ agent: { ...agent, is_off: true } })} />);

    expect(await screen.findByText("agent-1 recent history")).toBeInTheDocument();
    expect(terminalSpy).not.toHaveBeenCalled();
    expect(screen.queryByTestId("agent-session-presentation-mode")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Load older transcript" }));
    expect(await screen.findByText("agent-1 older history")).toBeInTheDocument();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("load_agent_chat_page", {
      sessionId: "agent-1", cursor: "older-cursor", revision: undefined, detailRef: undefined,
    });
    expect(vi.mocked(invoke).mock.calls.every(([command]) => command === "load_agent_chat_page")).toBe(true);
    expect(screen.getByRole("textbox", { name: "Message agent" })).toBeEnabled();
  });

  it("retains the initial Chat view when an Off agent becomes live", async () => {
    mockHistory();
    const view = render(<AgentSessionSurface {...surfaceProps({ agent: { ...agent, is_off: true } })} />);
    await screen.findByText("agent-1 recent history");
    view.rerender(<AgentSessionSurface {...surfaceProps()} />);
    expect(screen.getByRole("button", { name: /Switch to Terminal/ })).toBeInTheDocument();
    expect(terminalSpy).not.toHaveBeenCalled();
  });

  it("keeps drafts with their resource across toggles and rebinds", async () => {
    mockHistory();
    const view = render(<AgentSessionSurface {...surfaceProps()} />);
    expect(screen.getByTestId("agent-terminal")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Switch to Chat/ }));
    await screen.findByText("agent-1 recent history");
    fireEvent.change(screen.getByRole("textbox", { name: "Message agent" }), { target: { value: "Mendel draft" } });
    fireEvent.click(screen.getByRole("button", { name: /Switch to Terminal/ }));
    fireEvent.click(screen.getByRole("button", { name: /Switch to Chat/ }));
    expect(screen.getByRole("textbox", { name: "Message agent" })).toHaveValue("Mendel draft");

    view.rerender(<AgentSessionSurface {...surfaceProps({
      resource_key: "agent-2", agent: { ...replacementAgent, is_off: true },
    })} />);
    expect(await screen.findByText("agent-2 recent history")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Message agent" })).toHaveValue("");
    expect(screen.queryByText("agent-1 recent history")).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Message agent" }), { target: { value: "Curie draft" } });
    view.rerender(<AgentSessionSurface {...surfaceProps()} />);
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message agent" })).toHaveValue("Mendel draft"));
  });

  it("keeps Chat read only on an explicitly read-only surface", async () => {
    mockHistory();
    render(<AgentSessionSurface {...surfaceProps({
      agent: { ...agent, is_off: true }, requested_interaction: "read_only",
    })} />);
    expect(await screen.findByText("agent-1 recent history")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Message agent" })).toBeDisabled();
    expect(terminalSpy).not.toHaveBeenCalled();
  });

  it("suspends Chat polling in hidden presentations and restores the unsent draft", async () => {
    mockHistory();
    const view = render(<AgentSessionSurface {...surfaceProps({ agent: { ...agent, is_off: true } })} />);
    await screen.findByText("agent-1 recent history");
    fireEvent.change(screen.getByRole("textbox", { name: "Message agent" }), { target: { value: "Retained draft" } });
    const clearTimer = vi.spyOn(globalThis, "clearTimeout");
    view.rerender(<AgentSessionSurface {...surfaceProps({
      agent: { ...agent, is_off: true }, visibility: "hidden", render_state: "suspended",
    })} />);
    expect(clearTimer).toHaveBeenCalled();
    clearTimer.mockRestore();
    expect(screen.queryByRole("textbox", { name: "Message agent" })).not.toBeInTheDocument();
    expect(terminalSpy).not.toHaveBeenCalled();
    view.rerender(<AgentSessionSurface {...surfaceProps({ agent: { ...agent, is_off: true } })} />);
    expect(await screen.findByText("agent-1 recent history")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Message agent" })).toHaveValue("Retained draft");
  });

  it("allows explicit Off prompts in Chat without inheriting a PTY mirror's read-only lease", async () => {
    mockHistory();
    const historyInvoke = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "submit_prompt_to_agent") return {
        session_id: "agent-1", conversation_id: "agent-1-conversation",
        submitted_input_id: "input-1", status: "accepted",
      };
      return historyInvoke?.(command, args);
    });
    render(<AgentSessionSurface {...surfaceProps({
      agent: { ...agent, is_off: true },
      broker_state: brokerState({ owner_presentation_id: "another-presentation" }),
    })} />);
    await screen.findByText("agent-1 recent history");
    fireEvent.change(screen.getByRole("textbox", { name: "Message agent" }), { target: { value: "Explicit Off prompt" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("submit_prompt_to_agent", {
      sessionId: "agent-1", prompt: "Explicit Off prompt",
    }));
    expect(terminalSpy).not.toHaveBeenCalled();
  });

  it("derives a stable renderer identity and forwards explicit presentation lifecycle", () => {
    const onTitleChange = vi.fn();
    const onTerminalFocus = vi.fn();
    render(<AgentSessionSurface {...surfaceProps({
      visibility: "hidden",
      render_state: "suspended",
      requested_interaction: "read_only",
      on_title_change: onTitleChange,
      on_terminal_focus: onTerminalFocus,
    })} />);

    expect(agentSessionPresentationId("surface-7", "agent-1")).toBe("surface-7:agent:agent-1");
    const props = terminalSpy.mock.calls[terminalSpy.mock.calls.length - 1]?.[0] as Record<string, unknown>;
    expect(props).toMatchObject({
      sessionId: "agent-1",
      presentationId: "surface-7:agent:agent-1",
      visibility: "hidden",
      renderState: "suspended",
      requestedInteraction: "read_only",
      provider: "codex",
      workspacePath: "/workspace/wardian",
    });

    (props.onTitleChange as (title: string) => void)("Implementing navigation");
    (props.onTerminalFocus as () => void)();
    expect(onTitleChange).toHaveBeenCalledWith("agent-1", "Implementing navigation");
    expect(onTerminalFocus).toHaveBeenCalledWith("agent-1");
  });

  it("renders a recoverable placeholder when the resource agent is missing", () => {
    const onRefresh = vi.fn();
    const onRebind = vi.fn();
    const onReset = vi.fn();
    const onClose = vi.fn();
    render(<AgentSessionSurface {...surfaceProps({
      agent: undefined,
      on_refresh_agents: onRefresh,
      rebind_candidates: [agent, replacementAgent],
      on_rebind_agent: onRebind,
      on_reset_surface: onReset,
      on_close_surface: onClose,
    })} />);

    expect(screen.getByTestId("agent-session-surface")).toHaveAttribute("data-missing-agent", "true");
    expect(screen.getByText("Agent unavailable")).toBeInTheDocument();
    expect(screen.getByText(/agent-1/)).toBeInTheDocument();
    expect(screen.queryByTestId("agent-terminal")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Refresh agents" }));
    expect(onRefresh).toHaveBeenCalledOnce();
    fireEvent.change(screen.getByRole("combobox", { name: "Rebind Agent Session" }), {
      target: { value: "agent-2" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Rebind" }));
    expect(onRebind).toHaveBeenCalledWith("agent-2");
    fireEvent.click(screen.getByRole("button", { name: "Reset Surface" }));
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onReset).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("treats the resource key as authoritative when an unrelated agent is supplied", () => {
    render(<AgentSessionSurface {...surfaceProps({
      resource_key: "deleted-agent",
      agent,
    })} />);

    expect(screen.getByTestId("agent-session-surface")).toHaveAttribute("data-missing-agent", "true");
    expect(screen.queryByTestId("agent-terminal")).not.toBeInTheDocument();
  });

  it("updates owner, mirror, and read-only badges from broker presentation state", () => {
    const presentationId = "surface-7:agent:agent-1";
    const view = render(<AgentSessionSurface {...surfaceProps({
      broker_state: brokerState({ owner_presentation_id: presentationId }),
      presentation_state: presentationState(presentationId),
    })} />);

    expect(screen.getByTestId("agent-session-presentation-mode")).toHaveTextContent("Owner");
    expect(screen.queryByTestId("agent-session-read-only")).not.toBeInTheDocument();

    view.rerender(<AgentSessionSurface {...surfaceProps({
      broker_state: brokerState({ owner_presentation_id: "another-presentation" }),
      presentation_state: presentationState(presentationId),
    })} />);

    expect(screen.getByTestId("agent-session-presentation-mode")).toHaveTextContent("Mirror");
    expect(screen.getByTestId("agent-session-read-only")).toHaveTextContent("Read only");

    view.rerender(<AgentSessionSurface {...surfaceProps({
      broker_state: brokerState({ owner_presentation_id: null }),
      presentation_state: presentationState(presentationId, {
        interaction_capability: "read_only",
      }),
    })} />);

    expect(screen.getByTestId("agent-session-presentation-mode")).toHaveTextContent("Mirror");
    expect(screen.getByTestId("agent-session-read-only")).toHaveTextContent("Read only");
  });

  it("updates badges from the live terminal observation callback", () => {
    const presentationId = "surface-7:agent:agent-1";
    render(<AgentSessionSurface {...surfaceProps()} />);
    expect(screen.getByTestId("agent-session-presentation-mode")).toHaveTextContent("Connecting");

    const terminalProps = terminalSpy.mock.calls[terminalSpy.mock.calls.length - 1]?.[0] as Record<string, unknown>;
    act(() => {
      (terminalProps.onPresentationStateChange as (
        broker: TerminalBrokerState,
        presentation: TerminalPresentationState,
      ) => void)(
        brokerState({ owner_presentation_id: presentationId }),
        presentationState(presentationId),
      );
    });

    expect(screen.getByTestId("agent-session-presentation-mode")).toHaveTextContent("Owner");
    expect(screen.queryByTestId("agent-session-read-only")).not.toBeInTheDocument();
  });

  it("owns no agent runtime lifecycle callback when the presentation closes", () => {
    const view = render(<AgentSessionSurface {...surfaceProps()} />);
    const terminalProps = terminalSpy.mock.calls[terminalSpy.mock.calls.length - 1]?.[0] as Record<string, unknown>;

    expect(terminalProps).not.toHaveProperty("onKill");
    expect(terminalProps).not.toHaveProperty("onDelete");
    expect(terminalProps).not.toHaveProperty("onPause");
    expect(terminalProps).not.toHaveProperty("onClear");

    expect(() => view.unmount()).not.toThrow();
  });
});
