import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  AgentConfig,
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
});

describe("AgentSessionSurface", () => {
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
