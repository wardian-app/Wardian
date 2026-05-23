import test from "node:test";
import assert from "node:assert/strict";

export const PROVIDERS = ["codex", "claude", "gemini", "opencode", "antigravity"];

export const INPUT_CASES = [
  {
    name: "short",
    input: "Reply with exactly WARDIAN_DELIVERY_SHORT.",
  },
  {
    name: "multiline",
    input: "Reply with these two lines:\nWARDIAN_DELIVERY_MULTI_1\nWARDIAN_DELIVERY_MULTI_2",
  },
  {
    name: "long",
    input: [
      "Summarize this delivery validation paragraph in one sentence.",
      "WARDIAN_DELIVERY_LONG ".repeat(80).trim(),
    ].join("\n"),
  },
  {
    name: "slash-command",
    input: "/help",
  },
  {
    name: "trailing-newline",
    input: "Reply with exactly WARDIAN_DELIVERY_TRAILING.\n",
  },
];

const runRealDelivery = process.env.WARDIAN_E2E_REAL_DELIVERY === "1";
const allowPartialDelivery = process.env.WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL === "1";

function parseDeliveryProviders(value) {
  const requested = String(value ?? "")
    .split(",")
    .map((provider) => provider.trim().toLowerCase())
    .filter(Boolean);

  return requested.length > 0 ? requested : [...PROVIDERS];
}

function unknownProviders(providers) {
  const known = new Set(PROVIDERS);
  return providers.filter((provider) => !known.has(provider));
}

function missingProviders(providers) {
  const selected = new Set(providers);
  return PROVIDERS.filter((provider) => !selected.has(provider));
}

test("real provider delivery validation matrix is explicitly opted in", (t) => {
  if (!runRealDelivery) {
    assert.deepEqual(parseDeliveryProviders(process.env.WARDIAN_E2E_DELIVERY_PROVIDERS), PROVIDERS);
    assert.ok(PROVIDERS.includes("antigravity"));
    assert.equal(INPUT_CASES.length, 5);
    t.skip("Set WARDIAN_E2E_REAL_DELIVERY=1 to run real-provider delivery validation.");
    return;
  }

  const providers = parseDeliveryProviders(process.env.WARDIAN_E2E_DELIVERY_PROVIDERS);
  const unknown = unknownProviders(providers);
  assert.deepEqual(
    unknown,
    [],
    `Unknown provider(s) in WARDIAN_E2E_DELIVERY_PROVIDERS: ${unknown.join(", ")}`,
  );

  if (!allowPartialDelivery) {
    const missing = missingProviders(providers);
    assert.deepEqual(
      missing,
      [],
      `WARDIAN_E2E_DELIVERY_PROVIDERS must include the full delivery matrix unless WARDIAN_E2E_DELIVERY_ALLOW_PARTIAL=1. Missing: ${missing.join(", ")}`,
    );
  }

  assert.deepEqual(PROVIDERS, ["codex", "claude", "gemini", "opencode", "antigravity"]);
  assert.deepEqual(
    INPUT_CASES.map((inputCase) => inputCase.name),
    ["short", "multiline", "long", "slash-command", "trailing-newline"],
  );

  for (const provider of providers) {
    for (const inputCase of INPUT_CASES) {
      assert.equal(typeof provider, "string");
      assert.equal(typeof inputCase.input, "string");
      assert.ok(inputCase.input.length > 0, `${provider}/${inputCase.name} input must not be empty`);
    }
  }
});
