import { describe, expect, it, vi } from "vitest";
import { runNewSessionAction } from "./newSessionAction";

describe("runNewSessionAction", () => {
  it("shows the capture cause and recovery guidance from a rejected clear", async () => {
    const captureError =
      "Failed to acquire the closing provider log: conversation archive append failed: conflicting source identity\n\n" +
      "Your current conversation is preserved. Retry New Session. If the error persists, report this capture error.";
    const clearAgent = vi.fn().mockRejectedValue(captureError);
    const alert = vi.spyOn(window, "alert").mockImplementation(() => {});
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});

    await runNewSessionAction(clearAgent, "agent-1");

    expect(clearAgent).toHaveBeenCalledWith("agent-1");
    expect(consoleError).toHaveBeenCalledWith(captureError);
    expect(alert).toHaveBeenCalledWith(
      `Failed to start a new session: ${captureError}`,
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
