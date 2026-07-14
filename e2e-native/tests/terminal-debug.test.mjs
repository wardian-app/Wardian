import assert from "node:assert/strict";
import test from "node:test";

import {
  resolveAgentSessionTerminalPresentationId,
  resolveAgentTerminalPresentationId,
} from "../lib/terminal-debug.mjs";

function terminalHost(sessionId, presentationId) {
  const attributes = new Map([
    ["data-terminal-session-id", sessionId],
    ["data-terminal-presentation-id", presentationId],
  ]);
  return { getAttribute: (name) => attributes.get(name) ?? null };
}

function terminalRoot(hosts, attributes = {}) {
  return {
    getAttribute: (name) => attributes[name] ?? null,
    querySelectorAll: () => hosts,
  };
}

function fakeDriver({ cards = new Map(), panels = [], presentationIds = [] }) {
  return {
    async executeScript(callback, ...args) {
      const previousDocument = globalThis.document;
      const previousWindow = globalThis.window;
      globalThis.document = {
        getElementById: (id) => cards.get(id) ?? null,
        querySelectorAll: () => panels,
      };
      globalThis.window = {
        __wardianTerminalDebug: { presentationIds: () => presentationIds },
      };
      try {
        return callback(...args);
      } finally {
        globalThis.document = previousDocument;
        globalThis.window = previousWindow;
      }
    },
    async wait(probe, _timeoutMs, message) {
      const result = await probe();
      if (!result) throw new Error(message);
      return result;
    },
  };
}

test("resolves an Agents renderer only from the exact session card host", async () => {
  const sessionId = "session-a";
  const presentationId = "agents-surface:agent:session-a";
  const driver = fakeDriver({
    cards: new Map([[`agent-card-${sessionId}`, terminalRoot([
      terminalHost(sessionId, presentationId),
    ])]]),
    presentationIds: [presentationId],
  });

  assert.equal(
    await resolveAgentTerminalPresentationId(driver, sessionId),
    presentationId,
  );
});

test("fails closed when one Agents card has ambiguous terminal presentations", async () => {
  const sessionId = "session-a";
  const driver = fakeDriver({
    cards: new Map([[`agent-card-${sessionId}`, terminalRoot([
      terminalHost(sessionId, "presentation-a"),
      terminalHost(sessionId, "presentation-b"),
    ])]]),
    presentationIds: ["presentation-a", "presentation-b"],
  });

  await assert.rejects(
    resolveAgentTerminalPresentationId(driver, sessionId, 1),
    /Timed out resolving the terminal presentation for agent session-a/,
  );
});

test("agent-session resolution requires an exact surface when a session has two tabs", async () => {
  const sessionId = "session-a";
  const panels = [
    terminalRoot([terminalHost(sessionId, "surface-a:agent")], {
      "data-resource-key": sessionId,
      "data-surface-id": "surface-a",
    }),
    terminalRoot([terminalHost(sessionId, "surface-b:agent")], {
      "data-resource-key": sessionId,
      "data-surface-id": "surface-b",
    }),
  ];
  const driver = fakeDriver({
    panels,
    presentationIds: ["surface-a:agent", "surface-b:agent"],
  });

  await assert.rejects(
    resolveAgentSessionTerminalPresentationId(driver, sessionId, { timeoutMs: 1 }),
    /Timed out resolving the terminal presentation for agent-session session-a/,
  );
  assert.equal(
    await resolveAgentSessionTerminalPresentationId(driver, sessionId, {
      surfaceId: "surface-b",
      timeoutMs: 1,
    }),
    "surface-b:agent",
  );
});
