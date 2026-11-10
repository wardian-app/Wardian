import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { ChatRowActions } from "./chatCopy";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));

beforeEach(() => { vi.mocked(writeText).mockReset().mockResolvedValue(); });
afterEach(() => { vi.restoreAllMocks(); });

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

describe("gesture-lazy copy values", () => {
  it("does not resolve during render or menu opening and uses the latest value on a gesture", async () => {
    const user = userEvent.setup();
    const oldValue = vi.fn(() => "old row");
    const currentValue = vi.fn(() => "current row");
    const view = render(<ChatRowActions actions={[{ label: "Copy message", value: oldValue }]} />);
    await user.click(screen.getByRole("button", { name: "Message actions" }));
    view.rerender(<ChatRowActions actions={[{ label: "Copy message", value: currentValue }]} />);
    expect(oldValue).not.toHaveBeenCalled();
    expect(currentValue).not.toHaveBeenCalled();
    await user.click(screen.getByRole("menuitem", { name: "Copy message" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("current row"));
    expect(oldValue).not.toHaveBeenCalled();
    expect(currentValue).toHaveBeenCalledTimes(1);
  });

  it.each(["direct", "menu"])("resolves once for the %s gesture including browser fallback", async (gesture) => {
    const user = userEvent.setup();
    const browserWrite = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    vi.mocked(writeText).mockRejectedValue(new Error("Native clipboard unavailable"));
    const value = vi.fn(() => "exact bytes\nincluding order\n");
    render(<ChatRowActions actions={[{ label: "Copy message", value }]} />);
    if (gesture === "menu") await user.click(screen.getByRole("button", { name: "Message actions" }));
    expect(value).not.toHaveBeenCalled();
    await user.click(screen.getByRole(gesture === "menu" ? "menuitem" : "button", { name: "Copy message" }));
    await waitFor(() => expect(browserWrite).toHaveBeenCalledWith("exact bytes\nincluding order\n"));
    expect(value).toHaveBeenCalledTimes(1);
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(browserWrite).toHaveBeenCalledTimes(1);
    expect(screen.getByRole(gesture === "menu" ? "menuitem" : "button", { name: "Copy message copied" })).toBeInTheDocument();
  });

  it.each(["direct", "menu"])("reports factory errors and recovers on the next %s gesture", async (gesture) => {
    const user = userEvent.setup();
    const value = vi.fn<() => string>().mockImplementationOnce(() => { throw new Error("Formatting failed"); })
      .mockReturnValue("recovered");
    render(<ChatRowActions actions={[{ label: "Copy message", value }]} />);
    if (gesture === "menu") await user.click(screen.getByRole("button", { name: "Message actions" }));
    const role = gesture === "menu" ? "menuitem" : "button";
    await user.click(screen.getByRole(role, { name: "Copy message" }));
    expect(screen.getByRole(role, { name: "Copy message failed" })).toBeInTheDocument();
    expect(writeText).not.toHaveBeenCalled();
    await user.click(screen.getByRole(role, { name: "Copy message failed" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("recovered"));
    expect(value).toHaveBeenCalledTimes(2);
    expect(screen.getByRole(role, { name: "Copy message copied" })).toBeInTheDocument();
  });

  it.each([false, true])("keeps an empty %s-factory value a no-op", async (factory) => {
    const user = userEvent.setup();
    const empty = vi.fn(() => "");
    render(<ChatRowActions actions={[{ label: "Copy message", value: factory ? empty : "" }]} />);
    await user.click(screen.getByRole("button", { name: "Copy message" }));
    expect(writeText).not.toHaveBeenCalled();
    expect(empty).toHaveBeenCalledTimes(factory ? 1 : 0);
    expect(screen.getByRole("button", { name: "Copy message" })).toBeInTheDocument();
  });
});
