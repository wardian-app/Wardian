import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CustomCloneModal } from "./CustomCloneModal";
import type { AgentClonePreview } from "../../types";

const invokeMock = vi.mocked(invoke);

const preview: AgentClonePreview = {
  source_session_id: "agent-1",
  source_session_name: "Alpha",
  suggested_session_name: "Alpha-copy",
  provider: "claude",
  agent_class: "Coder",
  folder: "D:/Development/Wardian",
  files: {
    name: "agent-1",
    path: "",
    kind: "directory",
    children: [
      { name: "AGENTS.md", path: "AGENTS.md", kind: "file", children: [] },
      { name: "notes.md", path: "notes.md", kind: "file", children: [] },
    ],
  },
  default_selected_files: ["AGENTS.md", "notes.md"],
  skills: [{ name: "planner", source_path: "group-a/planner" }],
  default_selected_skills: [{ name: "planner", source_path: "group-a/planner" }],
};

const classes = [
  { name: "Coder", description: "", is_default: true },
  { name: "Reviewer", description: "", is_default: true },
];

describe("CustomCloneModal", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("loads preview when opened", async () => {
    invokeMock.mockResolvedValue(preview);

    render(
      <CustomCloneModal
        sourceSessionId="agent-1"
        agentClasses={classes}
        isOpen
        onClose={() => {}}
        onCloned={() => {}}
      />,
    );

    expect(await screen.findByDisplayValue("Alpha-copy")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("get_agent_clone_preview", {
      sourceSessionId: "agent-1",
    });
  });

  it("shows a blocking error when preview fails", async () => {
    invokeMock.mockRejectedValue(new Error("preview failed"));

    render(
      <CustomCloneModal
        sourceSessionId="agent-1"
        agentClasses={classes}
        isOpen
        onClose={() => {}}
        onCloned={() => {}}
      />,
    );

    expect(await screen.findByText(/preview failed/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close" })).toBeInTheDocument();
  });

  it("submits edited identity fields and selected profile items", async () => {
    const user = userEvent.setup();
    const onCloned = vi.fn();
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_agent_clone_preview") return preview;
      if (command === "clone_agent") return { session_id: "clone-1" };
      return null;
    });

    render(
      <CustomCloneModal
        sourceSessionId="agent-1"
        agentClasses={classes}
        isOpen
        onClose={() => {}}
        onCloned={onCloned}
      />,
    );

    await screen.findByDisplayValue("Alpha-copy");
    await user.selectOptions(screen.getByLabelText("Provider Engine"), "codex");
    await user.selectOptions(screen.getByLabelText("Agent Class"), "Reviewer");
    await user.clear(screen.getByLabelText("Workspace Path"));
    await user.type(screen.getByLabelText("Workspace Path"), "D:/Development/Wardian");
    await user.click(screen.getByRole("checkbox", { name: "notes.md" }));
    await user.click(screen.getByRole("checkbox", { name: /planner/ }));
    await user.click(screen.getByRole("button", { name: "Clone" }));

    expect(invokeMock).toHaveBeenCalledWith("clone_agent", {
      req: expect.objectContaining({
        source_session_id: "agent-1",
        mode: "profile",
        session_name: "Alpha-copy",
        provider: "codex",
        agent_class: "Reviewer",
        folder: "D:/Development/Wardian",
        profile_selection: {
          files: ["AGENTS.md"],
          skills: [],
        },
      }),
    });
    expect(onCloned).toHaveBeenCalled();
  });

  it("rejects a blank clone name without submitting", async () => {
    const user = userEvent.setup();
    invokeMock.mockResolvedValue(preview);

    render(
      <CustomCloneModal
        sourceSessionId="agent-1"
        agentClasses={classes}
        isOpen
        onClose={() => {}}
        onCloned={() => {}}
      />,
    );

    const nameInput = await screen.findByLabelText("Clone Name");
    await user.clear(nameInput);
    await user.click(screen.getByRole("button", { name: "Clone" }));

    expect(screen.getByText(/Clone name is required/)).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("clone_agent", expect.anything());
  });

  it("keeps the modal open when submit fails", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_agent_clone_preview") return preview;
      if (command === "clone_agent") throw new Error("clone failed");
      return null;
    });

    render(
      <CustomCloneModal
        sourceSessionId="agent-1"
        agentClasses={classes}
        isOpen
        onClose={() => {}}
        onCloned={() => {}}
      />,
    );

    await screen.findByDisplayValue("Alpha-copy");
    await user.click(screen.getByRole("button", { name: "Clone" }));

    expect(await screen.findByText(/clone failed/)).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByRole("dialog", { name: "Custom Clone" })).toBeInTheDocument();
    });
  });
});
