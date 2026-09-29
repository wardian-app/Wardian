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

  it("ignores a repeated request while the same agent's New Session is running", async () => {
    let finish: () => void = () => {};
    const clearAgent = vi
      .fn()
      .mockReturnValueOnce(
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
      )
      .mockResolvedValue(undefined);

    const first = runNewSessionAction(clearAgent, "agent-repeat");
    await runNewSessionAction(clearAgent, "agent-repeat");
    await runNewSessionAction(clearAgent, "agent-other-repeat");

    expect(clearAgent).toHaveBeenCalledTimes(2);
    expect(clearAgent).toHaveBeenNthCalledWith(1, "agent-repeat");
    expect(clearAgent).toHaveBeenNthCalledWith(2, "agent-other-repeat");

    finish();
    await first;
    await runNewSessionAction(clearAgent, "agent-repeat");
    expect(clearAgent).toHaveBeenCalledTimes(3);
  });

  it("allows another New Session after a failed one", async () => {
    const clearAgent = vi.fn().mockRejectedValueOnce("boom").mockResolvedValue(undefined);
    const alert = vi.spyOn(window, "alert").mockImplementation(() => {});
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});

    await runNewSessionAction(clearAgent, "agent-retry");
    await runNewSessionAction(clearAgent, "agent-retry");

    expect(clearAgent).toHaveBeenCalledTimes(2);
    alert.mockRestore();
    consoleError.mockRestore();
  });
});
