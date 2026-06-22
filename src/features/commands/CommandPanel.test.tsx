import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { ConfirmProvider } from "../../components/ConfirmDialog";
import { useLibraryStore } from "../../store/useLibraryStore";
import type { LibraryFolder } from "../../types";
import { flattenPromptForInjection } from "../../utils/terminalInput";
import { CommandPanel } from "./CommandPanel";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockWriteText = vi.mocked(writeText);
const originalState = useLibraryStore.getState();

function promptTree(): LibraryFolder {
  return {
    type: "Folder",
    path: "",
    name: "Prompts",
    children: [
      {
        type: "Prompt",
        path: "quick/ship.md",
        name: "Ship Summary",
        content: "Ship it\nwith notes",
        metadata: { id: "prompt-1", tags: [], is_starred: true },
      },
      {
        type: "Prompt",
        path: "draft.md",
        name: "Draft",
        content: "Hidden prompt",
        metadata: { id: "prompt-2", tags: [], is_starred: false },
      },
      {
        type: "Folder",
        path: "nested",
        name: "Nested",
        children: [
          {
            type: "Prompt",
            path: "nested/review.md",
            name: "Review Notes",
            content: "Review this",
            metadata: { id: "prompt-3", tags: [], is_starred: true },
          },
        ],
      },
    ],
  };
}

function renderCommandPanel(options?: {
  selectedAgentIds?: Set<string>;
  broadcastMessage?: string;
  setBroadcastMessage?: (message: string) => void;
  onBroadcast?: (event: React.FormEvent) => void;
}) {
  const props = {
    selectedAgentIds: options?.selectedAgentIds ?? new Set(["agent-1"]),
    broadcastMessage: options?.broadcastMessage ?? "",
    setBroadcastMessage: options?.setBroadcastMessage ?? vi.fn(),
    onBroadcast: options?.onBroadcast ?? vi.fn(),
  };

  render(
    <ConfirmProvider>
      <CommandPanel {...props} />
    </ConfirmProvider>,
  );

  return props;
}

describe("CommandPanel", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockWriteText.mockReset();
    useLibraryStore.setState({
      ...originalState,
      promptTree: promptTree(),
      skillTree: null,
      activeTab: "prompts",
      isLoading: false,
      error: null,
    });
  });

  afterAll(() => {
    useLibraryStore.setState(originalState);
  });

  it("lists starred prompts from nested folders and omits unstarred prompts", () => {
    renderCommandPanel();

    expect(screen.queryByRole("link", { name: /command guide/i })).not.toBeInTheDocument();
    expect(screen.getByText("Ship Summary")).toBeInTheDocument();
    expect(screen.getByText("Review Notes")).toBeInTheDocument();
    expect(screen.queryByText("Draft")).not.toBeInTheDocument();
  });

  it("shows an empty quick prompt state when no prompts are starred", () => {
    useLibraryStore.setState({
      promptTree: {
        type: "Folder",
        path: "",
        name: "Prompts",
        children: [
          {
            type: "Prompt",
            path: "draft.md",
            name: "Draft",
            content: "Hidden prompt",
            metadata: { id: "prompt-2", tags: [], is_starred: false },
          },
        ],
      },
    });

    renderCommandPanel();

    expect(screen.getByText("No quick prompts in Library.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Draft/i })).not.toBeInTheDocument();
  });

  it("uses compact sidebar typography for title and section labels", () => {
    renderCommandPanel();

    expect(screen.getByRole("heading", { name: "Command", level: 2 })).toHaveClass("text-sm");
    expect(screen.getByRole("heading", { name: "Quick Prompts", level: 3 })).toHaveClass("text-xs");
    expect(screen.getByRole("heading", { name: "Broadcast", level: 3 })).toHaveClass("text-xs");
  });

  it("flattens and injects a quick prompt into selected agents", async () => {
    const user = userEvent.setup();

    renderCommandPanel({ selectedAgentIds: new Set(["agent-1", "agent-2"]) });
    await user.click(screen.getByRole("button", { name: /Ship Summary/i }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("submit_prompt_to_agent", {
        sessionId: "agent-1",
        prompt: "Ship it with notes",
      });
    });
    expect(mockInvoke).toHaveBeenCalledWith("submit_prompt_to_agent", {
      sessionId: "agent-2",
      prompt: "Ship it with notes",
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("list_agents");
  });

  it("injects the full flattened content of a long quick prompt", async () => {
    const user = userEvent.setup();
    const longContent = Array.from({ length: 30 }, (_, index) => `Line ${index + 1}: review this section.`).join(
      "\n\n",
    );
    useLibraryStore.setState({
      promptTree: {
        type: "Folder",
        path: "",
        name: "Prompts",
        children: [
          {
            type: "Prompt",
            path: "long.md",
            name: "Long Prompt",
            content: longContent,
            metadata: { id: "prompt-long", tags: [], is_starred: true },
          },
        ],
      },
    });

    renderCommandPanel({ selectedAgentIds: new Set(["agent-1"]) });
    await user.click(screen.getByRole("button", { name: /Long Prompt/i }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("submit_prompt_to_agent", {
        sessionId: "agent-1",
        prompt: flattenPromptForInjection(longContent),
      });
    });
  });

  it("confirms before broadcasting a quick prompt when no agents are selected", async () => {
    const user = userEvent.setup();
    mockInvoke.mockImplementation(async (command) => {
      if (command === "list_agents") {
        return [
          { session_id: "agent-1", session_name: "One", agent_class: "Coder", folder: "C:/repo", is_off: false },
          { session_id: "agent-2", session_name: "Two", agent_class: "Coder", folder: "C:/repo", is_off: false },
        ];
      }
      return null;
    });

    renderCommandPanel({ selectedAgentIds: new Set() });
    await user.click(screen.getByRole("button", { name: /Review Notes/i }));
    expect(screen.getByText(/broadcast the prompt to all agents/i)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("list_agents");
    });
    expect(mockInvoke).toHaveBeenCalledWith("submit_prompt_to_agent", {
      sessionId: "agent-1",
      prompt: "Review this",
    });
    expect(mockInvoke).toHaveBeenCalledWith("submit_prompt_to_agent", {
      sessionId: "agent-2",
      prompt: "Review this",
    });
  });

  it("copies prompt content without injecting it", async () => {
    const user = userEvent.setup();

    renderCommandPanel();
    await user.click(screen.getAllByTitle("Copy to clipboard")[0]);

    expect(mockWriteText).toHaveBeenCalledWith("Ship it\nwith notes");
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("confirms empty-selection broadcasts before submitting", async () => {
    const user = userEvent.setup();
    const onBroadcast = vi.fn();

    renderCommandPanel({
      selectedAgentIds: new Set(),
      broadcastMessage: "Status?",
      onBroadcast,
    });
    await user.click(screen.getByTestId("broadcast-submit"));
    expect(onBroadcast).not.toHaveBeenCalled();
    expect(screen.getByText("No agents selected. This will broadcast to ALL agents. Are you sure?")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Confirm" }));
    expect(onBroadcast).toHaveBeenCalledTimes(1);
  });
});
