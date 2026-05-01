import { describe, expect, it } from "vitest";
import spawnManager from "../../src-tauri/src/manager/spawn.rs?raw";

describe("agent spawn manager contract", () => {
  it("keeps interactive agent spawning enabled on non-Windows platforms", () => {
    expect(spawnManager).not.toContain("#[cfg(not(windows))]\npub async fn spawn_agent");
    expect(spawnManager).not.toContain(
      "Interactive agent spawning is only supported on Windows",
    );
  });
});
