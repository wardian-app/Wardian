import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ChatRowActions } from "./chatCopy";

describe("ChatRowActions Escape focus", () => {
  it("restores focus from a copy item and supports keyboard reopening", async () => {
    const user = userEvent.setup();
    render(<ChatRowActions actions={[{ label: "Copy message", value: "Hello" }]} />);
    const trigger = screen.getByRole("button", { name: "Message actions" });
    trigger.focus();
    await user.keyboard("{Enter}");
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    await user.tab();
    expect(screen.getByRole("menuitem", { name: "Copy message" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(trigger).toHaveFocus();
    await user.keyboard(" ");
    expect(screen.getByRole("menu")).toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(trigger).toHaveFocus();
  });

  it("does not steal focus when Escape dismisses a menu from elsewhere", async () => {
    const user = userEvent.setup();
    render(<><ChatRowActions actions={[{ label: "Copy message", value: "Hello" }]} /><button>Elsewhere</button></>);
    const trigger = screen.getByRole("button", { name: "Message actions" });
    trigger.focus();
    await user.keyboard("{Enter}");
    await user.tab();
    await user.tab();
    const elsewhere = screen.getByRole("button", { name: "Elsewhere" });
    expect(elsewhere).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(elsewhere).toHaveFocus();
  });
});
