// @tier ci — Pure harness error propagation; no native app or provider.
import test from "node:test";
import assert from "node:assert/strict";
import { invokeTauri } from "../lib/harness.mjs";

test("invokeTauri preserves a structured backend error code", async () => {
  const driver = {
    executeAsyncScript: async () => ({
      ok: false,
      error: { code: "provider_input_not_ready", message: "input is still booting" },
    }),
  };

  await assert.rejects(invokeTauri(driver, "submit_prompt_to_agent"), (error) => {
    assert.equal(error.code, "provider_input_not_ready");
    assert.match(error.message, /submit_prompt_to_agent failed/);
    return true;
  });
});
