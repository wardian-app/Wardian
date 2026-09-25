import { describe, expect, it, vi } from "vitest";
import { runNewSessionAction } from "./newSessionAction";

describe("runNewSessionAction", () => {
  it("shows a rejected clear failure", async () => {
    const clearAgent = vi.fn().mockRejectedValue(
      "Failed to acquire the closing provider log: ambiguous narrative ownership",
    );
    const alert = vi.spyOn(window, "alert").mockImplementation(() => {});
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});

    await runNewSessionAction(clearAgent, "agent-1");

    expect(clearAgent).toHaveBeenCalledWith("agent-1");
    expect(consoleError).toHaveBeenCalledWith(
      "Failed to acquire the closing provider log: ambiguous narrative ownership",
    );
    expect(alert).toHaveBeenCalledWith(
      "Failed to start a new session: Failed to acquire the closing provider log: ambiguous narrative ownership",
    );
    alert.mockRestore();
    consoleError.mockRestore();
  });

  it("does not show a failure when clear succeeds", async () => {
    const clearAgent = vi.fn().mockResolvedValue(undefined);
    const alert = vi.spyOn(window, "alert").mockImplementation(() => {});

    await runNewSessionAction(clearAgent, "agent-1");

    expect(alert).not.toHaveBeenCalled();
    alert.mockRestore();
  });
});
