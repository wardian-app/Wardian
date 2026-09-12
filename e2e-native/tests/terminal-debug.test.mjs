// @tier nightly — Runs on the nightly schedule; too slow or too broad for every pull request.
import assert from "node:assert/strict";
import test from "node:test";

import {
  assertTerminalDebugAvailable,
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

function fakeDriver({
  cards = new Map(), panels = [], presentationIds = [],
  debugAvailable = true, missingDebugMethods = [],
}) {
  return {
    async executeScript(callback, ...args) {
      const previousDocument = globalThis.document;
      const previousWindow = globalThis.window;
      globalThis.document = {
        getElementById: (id) => cards.get(id) ?? null,
        querySelectorAll: () => panels,
      };
      globalThis.window = {
        __wardianTerminalDebug: debugAvailable
          ? {
            presentationIds: () => presentationIds,
            snapshot: () => null,
            rawOutputLog: () => [],
            scrollToTop: () => false,
            scrollToBottom: () => false,
            scrollToViewportLine: () => false,
          }
          : undefined,
      };
      for (const method of missingDebugMethods) {
        delete globalThis.window.__wardianTerminalDebug[method];
      }
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

test("rendering preflight accepts an instrumented overview before any presentation exists", async () => {
  await assertTerminalDebugAvailable(fakeDriver({ presentationIds: [] }));
});

test("rendering preflight rejects missing or incomplete instrumentation", async () => {
  await assert.rejects(
    assertTerminalDebugAvailable(fakeDriver({ debugAvailable: false })),
    /VITE_WARDIAN_TERMINAL_DEBUG=1.*build/i,
  );
  await assert.rejects(
    assertTerminalDebugAvailable(fakeDriver({ missingDebugMethods: ["scrollToViewportLine"] })),
    /Missing methods: scrollToViewportLine/,
  );
});

test("missing build-time terminal debug reports the prerequisite instead of an identity timeout", async () => {
  const sessionId = "session-a";
  const driver = fakeDriver({
    cards: new Map([[`agent-card-${sessionId}`, terminalRoot([
      terminalHost(sessionId, "agents-surface:agent:session-a"),
    ])]]),
    debugAvailable: false,
  });

  await assert.rejects(
    resolveAgentTerminalPresentationId(driver, sessionId, 1),
    /VITE_WARDIAN_TERMINAL_DEBUG=1.*build/i,
  );
});

test("a host identity absent from the debug registry still fails closed", async () => {
  const sessionId = "session-a";
  const driver = fakeDriver({
    cards: new Map([[`agent-card-${sessionId}`, terminalRoot([
      terminalHost(sessionId, "agents-surface:agent:session-a"),
    ])]]),
    presentationIds: ["other-surface:agent:session-a"],
  });

  await assert.rejects(
    resolveAgentTerminalPresentationId(driver, sessionId, 1),
    /Timed out resolving the terminal presentation/,
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
