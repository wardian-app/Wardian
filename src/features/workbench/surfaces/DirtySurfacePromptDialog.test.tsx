import { act, fireEvent, render, screen } from "@testing-library/react";
import { useEffect } from "react";
import { describe, expect, it, vi } from "vitest";

import type { DirtySurfacePrompt } from "./dirtySurfaceGuards";
import { useDirtySurfacePrompt } from "./DirtySurfacePromptDialog";

function Harness({ expose }: { expose: (prompt: DirtySurfacePrompt) => void }) {
  const controller = useDirtySurfacePrompt();
  useEffect(() => expose(controller.prompt), [controller.prompt, expose]);
  return controller.dialog;
}

const request = {
  surface_type: "library" as const,
  title: "Library",
  message: "Save changes in Library before closing?",
  choices: ["save", "discard", "cancel"] as const,
};

describe("useDirtySurfacePrompt", () => {
  it.each([
    ["Save", "save"],
    ["Discard", "discard"],
    ["Cancel", "cancel"],
  ] as const)("resolves %s as %s", async (label, choice) => {
    let prompt: DirtySurfacePrompt = () => "cancel";
    render(<Harness expose={(next) => { prompt = next; }} />);

    let result: string | undefined;
    await act(async () => {
      const pending = prompt(request);
      Promise.resolve(pending).then((value) => { result = value; });
    });
    fireEvent.click(screen.getByRole("button", { name: label }));
    await act(async () => Promise.resolve());

    expect(result).toBe(choice);
  });

  it("serializes concurrent close prompts", async () => {
    let prompt: DirtySurfacePrompt = () => "cancel";
    render(<Harness expose={(next) => { prompt = next; }} />);
    const first = vi.fn();
    const second = vi.fn();

    await act(async () => {
      Promise.resolve(prompt(request)).then(first);
      Promise.resolve(prompt({ ...request, surface_type: "workflows", title: "Workflows" })).then(second);
    });
    expect(screen.getByRole("heading", { name: /library/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    await act(async () => Promise.resolve());
    expect(first).toHaveBeenCalledWith("discard");
    expect(second).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: /workflows/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await act(async () => Promise.resolve());
    expect(second).toHaveBeenCalledWith("cancel");
  });

  it("labels Files discard as Don't Save without changing Library or Workflows", async () => {
    let prompt: DirtySurfacePrompt = () => "cancel";
    render(<Harness expose={(next) => { prompt = next; }} />);

    await act(async () => {
      void prompt({
        ...request,
        surface_type: "files",
        title: "Files",
        discard_label: "Don't Save",
      });
    });
    expect(screen.getByRole("button", { name: "Don't Save" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Discard" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await act(async () => Promise.resolve());

    await act(async () => { void prompt(request); });
    expect(screen.getByRole("button", { name: "Discard" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Don't Save" })).toBeNull();
  });
});
