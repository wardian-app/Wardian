// @tier ci — Pure harness error propagation; no native app or provider.
import test from "node:test";
import assert from "node:assert/strict";
import { invokeTauri } from "../lib/harness.mjs";

function driverResult(result) {
  return { executeAsyncScript: async () => result };
}

test("invokeTauri preserves a structured backend error code", async () => {
  const driver = driverResult({
    ok: false,
    error: { code: "provider_input_not_ready", message: "input is still booting" },
  });

  await assert.rejects(invokeTauri(driver, "submit_prompt_to_agent"), (error) => {
    assert.equal(error.code, "provider_input_not_ready");
    assert.equal(error.message, "submit_prompt_to_agent failed: input is still booting");
    return true;
  });
});

test("invokeTauri retains rejection details without promoting non-string codes", async () => {
  for (const code of [undefined, null, 408, { nested: "code" }]) {
    const driver = driverResult({ ok: false, error: { code, message: "read failed" } });
    await assert.rejects(invokeTauri(driver, "load_agent_chat_page"), (error) => {
      assert.equal(error.code, undefined);
      assert.equal(error.message, "load_agent_chat_page failed: read failed");
      return true;
    });
  }
});

test("invokeTauri returns a successful response unchanged", async () => {
  const value = { session_id: "session", events: [] };
  assert.equal(await invokeTauri(driverResult({ ok: true, value }), "load_agent_chat_page"), value);
});
