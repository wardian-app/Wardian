import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { ConfirmProvider } from "../../components/ConfirmDialog";
import { useLibraryStore } from "../../store/useLibraryStore";
import { useOnboardingStore } from "../../store/useOnboardingStore";
import type { LibraryIndex } from "../../types";
import { flattenPromptForInjection } from "../../utils/terminalInput";
import { CommandPanel } from "./CommandPanel";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockWriteText = vi.mocked(writeText);
const originalState = useLibraryStore.getState();

function emptySection() {
  return { tree: { path: "", name: "Root", children: [] }, stubbed: false };
}

function indexWithPrompts(promptsTree: LibraryIndex["sections"]["prompts"]["tree"]): LibraryIndex {
  return {
    sections: {
      skills: emptySection(),
      prompts: { tree: promptsTree, stubbed: false },
      workflows: emptySection(),
      classes: emptySection(),
      mcps: { ...emptySection(), stubbed: true },
    },
    deployments: {},
    orphans: [],
  };
}

function samplePromptsTree(): LibraryIndex["sections"]["prompts"]["tree"] {
  return {
    path: "",
    name: "Root",
    children: [
      {
        kind: "prompt",
        path: "quick/ship.md",
        entry_ref: "prompts/quick/ship.md",
        name: "Ship Summary",
        description: "Ship it with notes",
        tags: [],
        is_starred: true,
        deployment_count: 0,
      },
      {
        kind: "prompt",
        path: "draft.md",
        entry_ref: "prompts/draft.md",
        name: "Draft",
        description: "Hidden prompt",
        tags: [],
        is_starred: false,
        deployment_count: 0,
      },
      {
        path: "nested",
        name: "Nested",
        children: [
          {
            kind: "prompt",
            path: "nested/review.md",
            entry_ref: "prompts/nested/review.md",
            name: "Review Notes",
            description: "Review this",
            tags: [],
            is_starred: true,
            deployment_count: 0,
          },
        ],
      },
    ],
  };
}

const promptContents: Record<string, string> = {
  "quick/ship.md": "Ship it\nwith notes",
  "draft.md": "Hidden prompt",
  "nested/review.md": "Review this",
};

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
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === "read_library_item") {
        const path = (args as { path?: string } | undefined)?.path ?? "";
        return promptContents[path] ?? "";
      }
      return null;
    });
    useLibraryStore.setState({
      ...originalState,
      index: indexWithPrompts(samplePromptsTree()),
      isLoading: false,
      error: null,
    });
    useOnboardingStore.setState({
      dismissedHintIds: [],
      contextualTipsEnabled: true,
      hintsLoaded: true,
    });
  });

  afterAll(() => {
    useLibraryStore.setState(originalState);
  });

  it("lists starred prompts from nested folders and omits unstarred prompts", () => {
    renderCommandPanel();

    expect(screen.getByText("Ship Summary")).toBeInTheDocument();
    expect(screen.getByText("Review Notes")).toBeInTheDocument();
    expect(screen.queryByText("Draft")).not.toBeInTheDocument();
  });

  it("shows an empty quick prompt state when no prompts are starred", () => {
    useLibraryStore.setState({
      index: indexWithPrompts({
        path: "",
        name: "Root",
        children: [
          {
            kind: "prompt",
            path: "draft.md",
            entry_ref: "prompts/draft.md",
            name: "Draft",
            description: "Hidden prompt",
            tags: [],
            is_starred: false,
            deployment_count: 0,
          },
        ],
      }),
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

  it("requires an agent selection before enabling command actions", () => {
    renderCommandPanel({ selectedAgentIds: new Set() });

    expect(screen.getByRole("button", { name: /Ship Summary/i })).toBeDisabled();
    expect(screen.getByTestId("broadcast-textarea")).toBeDisabled();
    expect(screen.getByTestId("broadcast-submit")).toBeDisabled();
    expect(screen.getByTestId("broadcast-submit")).toHaveAttribute(
      "title",
      "Select at least one agent to send a command",
    );
  });

  it("flattens and injects a quick prompt into selected agents", async () => {
    const user = userEvent.setup();

    renderCommandPanel({ selectedAgentIds: new Set(["agent-1", "agent-2"]) });
    await user.click(screen.getByRole("button", { name: /Ship Summary/i }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("read_library_item", { section: "prompts", path: "quick/ship.md" });
    });
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
    promptContents["long.md"] = longContent;
    useLibraryStore.setState({
      index: indexWithPrompts({
        path: "",
        name: "Root",
        children: [
          {
            kind: "prompt",
            path: "long.md",
            entry_ref: "prompts/long.md",
            name: "Long Prompt",
            description: "Long prompt",
            tags: [],
            is_starred: true,
            deployment_count: 0,
          },
        ],
      }),
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

  it("copies prompt content without injecting it", async () => {
    const user = userEvent.setup();

    renderCommandPanel();
    await user.click(screen.getAllByTitle("Copy to clipboard")[0]);

    await waitFor(() => {
      expect(mockWriteText).toHaveBeenCalledWith("Ship it\nwith notes");
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("submit_prompt_to_agent", expect.anything());
  });

  it("does not broadcast after an empty-selection form submit", () => {
    const onBroadcast = vi.fn();

    renderCommandPanel({
      selectedAgentIds: new Set(),
      broadcastMessage: "Status?",
      onBroadcast,
    });

    fireEvent.submit(screen.getByTestId("broadcast-submit").closest("form")!);

    expect(onBroadcast).not.toHaveBeenCalled();
  });
});
