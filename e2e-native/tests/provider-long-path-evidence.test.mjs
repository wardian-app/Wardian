// @tier nightly — Offline lifecycle assertions for the opt-in long-habitat gate.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import {
  WINDOWS_CWD_LIMIT,
  afterMaintainedProviderPause,
  assertCodexLaunchArtifacts,
  assertLongHabitatPrerequisites,
} from "../lib/provider-long-path-evidence.mjs";

const AGENT_ID = "11111111-1111-4111-8111-111111111111";
const PROVIDER_SESSION = "provider-session-11111111";
const DELIVERY_MARKER = "WARDIAN_LONG_HABITAT_OFFLINE";

async function removeIfPresent(target) {
  try {
    await fs.unlink(target);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
}

async function makeFixture() {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "ch"));
  let home = path.join(root, "wardian-home");
  while (path.join(home, "agents", AGENT_ID, "habitat", "workspace").length <= WINDOWS_CWD_LIMIT) {
    home = path.join(home, "long-home-segment-abcdefghijkl");
  }
  const habitat = path.join(home, "agents", AGENT_ID, "habitat");
  const workspacePath = path.join(root, "short-external-project");
  const workspaceMarker = path.join(workspacePath, "marker.txt");
  const aliasSlot = path.join(root, "c", "abcdef12");
  const aliasTarget = path.join(aliasSlot, "h");
  const aliasRecord = path.join(home, "agents", AGENT_ID, ".wardian-habitat-alias.json");
  const slotRecord = path.join(aliasSlot, ".wardian-habitat-alias.json");
  const markerBytes = Buffer.from("external logical workspace marker\n");

  await fs.mkdir(habitat, { recursive: true });
  await fs.mkdir(workspacePath, { recursive: true });
  await fs.mkdir(aliasSlot, { recursive: true });
  await fs.mkdir(path.join(habitat, ".codex"), { recursive: true });
  await fs.writeFile(workspaceMarker, markerBytes);
  await fs.writeFile(
    path.join(habitat, ".codex", "config.toml"),
    'model = "fixture-model"\nmodel_reasoning_effort = "low"\n',
  );
  await fs.symlink(workspacePath, path.join(habitat, "workspace"), "junction");
  await fs.symlink(habitat, aliasTarget, "junction");

  const record = {
    version: 1,
    agent_id: AGENT_ID,
    wardian_home: home,
    habitat: await fs.realpath(habitat),
    target: aliasTarget,
    token: "11111111-1111-4111-8111-111111111111",
    habitat_identity: [1, 2],
    slot_identity: [3, 4],
  };
  await fs.writeFile(aliasRecord, JSON.stringify(record, null, 2));
  await fs.writeFile(slotRecord, JSON.stringify(record, null, 2));

  return {
    root,
    home,
    habitat,
    workspacePath,
    workspaceMarker,
    aliasSlot,
    aliasTarget,
    aliasRecord,
    slotRecord,
    markerBytes,
  };
}

async function removeFixture(fixture) {
  await removeIfPresent(path.join(fixture.habitat, "workspace"));
  await removeIfPresent(fixture.aliasTarget);
  await fs.rm(fixture.root, { recursive: true, force: true });
}

function expectedCodexValues() {
  return [
    { key: "model", value: "fixture-model", source: "offline-fixture.config_override.model" },
    {
      key: "model_reasoning_effort",
      value: "low",
      source: "offline-fixture.config_override.provider_config.reasoning_effort",
    },
  ];
}

test("long-habitat helper requires explicit nonempty semantic Codex values", async () => {
  if (process.platform !== "win32") return;
  const fixture = await makeFixture();
  try {
    await assert.rejects(
      () => assertCodexLaunchArtifacts({ home: fixture.home, agentId: AGENT_ID, expectedValues: [] }),
      /require explicit expected values/,
    );
    await assert.rejects(
      () => assertCodexLaunchArtifacts({
        home: fixture.home,
        agentId: AGENT_ID,
        expectedValues: [{ key: "model", value: "fixture-model" }],
      }),
      /source-derived label/,
    );
  } finally {
    await removeFixture(fixture);
  }
});

test("long-habitat lifecycle retains bounded proof before owned deletion", async (t) => {
  if (process.platform !== "win32") {
    t.skip("ConPTY habitat junction evidence is Windows-only");
    return;
  }
  const fixture = await makeFixture();
  const evidenceRoot = path.join(fixture.root, "evidence");
  let deleted = false;
  let isOff = true;
  const agent = {
    session_id: AGENT_ID,
    session_name: "offline-long-habitat-codex",
    provider: "codex",
    folder: fixture.workspacePath,
    is_off: true,
    resume_session: PROVIDER_SESSION,
  };
  const maintainedReport = {
    transcript_user_evidence: [{
      provider: "codex",
      candidates: [{
        id: "user-event",
        provider: "codex",
        kind: "message",
        role: "user",
        source: "response_item",
        provider_log: true,
        native_identity: { session_id: PROVIDER_SESSION },
        timestamp: "2026-09-17T00:00:00.000Z",
        text_sha256: "user-hash",
        text_byte_count: DELIVERY_MARKER.length,
      }],
    }],
  };
  const events = [
    {
      id: "user-event",
      provider: "codex",
      kind: "message",
      role: "user",
      source: "response_item",
      text: DELIVERY_MARKER,
      metadata: { provider_log: true, provider_session_id: PROVIDER_SESSION },
    },
    {
      id: "assistant-event",
      provider: "codex",
      kind: "message",
      role: "assistant",
      source: "response_item",
      text: DELIVERY_MARKER,
      metadata: { provider_log: true, provider_session_id: PROVIDER_SESSION, turn_id: "turn-1" },
    },
  ];
  const invokeTauri = async (_driver, command) => {
    if (command === "list_agents") return deleted ? [] : [{ ...agent, is_off: isOff }];
    if (command === "resume_agent") {
      isOff = false;
      return null;
    }
    if (command === "pause_agent") {
      isOff = true;
      return null;
    }
    if (command === "load_agent_chat_transcript") return events;
    if (command === "list_conversations") {
      return { conversations: [{ agent_id: AGENT_ID, provider: "codex", conversation_id: "conversation-1", record_count: 2 }] };
    }
    throw new Error(`unexpected offline IPC command: ${command}`);
  };
  const runCliOk = async (_cliPath, _harness, args) => {
    assert.deepEqual(args, ["agent", "delete", agent.session_name, "--confirm", agent.session_name]);
    await removeIfPresent(fixture.aliasRecord);
    await removeIfPresent(fixture.aliasTarget);
    await removeIfPresent(fixture.slotRecord);
    await fs.rmdir(fixture.aliasSlot);
    deleted = true;
    return { status: 0, stdout: "", stderr: "" };
  };

  try {
    const preflight = await assertLongHabitatPrerequisites({
      home: fixture.home,
      agent,
      provider: "codex",
      workspacePath: fixture.workspacePath,
      expectedCodexValues: expectedCodexValues(),
    });
    const result = await afterMaintainedProviderPause({
      driver: {},
      harness: { isolatedHome: fixture.home },
      cliPath: "offline-cli",
      agent,
      provider: "codex",
      workspacePath: fixture.workspacePath,
      preflight,
      markerPath: fixture.workspaceMarker,
      deliveryMarker: DELIVERY_MARKER,
      maintainedReport,
      evidenceRoot,
      invokeTauri,
      pauseRealProviderAgent: async (driver, sessionId) => {
        assert.equal(sessionId, AGENT_ID);
        await invokeTauri(driver, "pause_agent", { sessionId });
        assert.equal((await invokeTauri(driver, "list_agents"))[0].is_off, true);
      },
      waitForProviderInputReady: async (_driver, provider, sessionId) => {
        assert.equal(provider, "codex");
        assert.equal(sessionId, AGENT_ID);
      },
      runCliOk,
    });

    assert.equal(result.pause_resume_identity_preserved, true);
    assert.equal(result.alias_reused_across_pause_resume, true);
    assert.equal(result.alias_removed_after_explicit_agent_delete, true);
    assert.equal(result.external_marker_retained, true);
    assert.equal(result.maintained_report_turn_evidence_reused, true);
    assert.equal(result.removed_agent_local_habitat_or_archive_asserted, false);
    const report = JSON.parse(await fs.readFile(result.bounded_evidence_path, "utf8"));
    assert.equal(report.credentials_or_auth_material_copied, false);
    assert.equal(report.raw_config_or_transcript_copied, false);
    assert.equal(report.archive.record_count, 2);
    assert.equal(report.config.expected_values.length, 2);
  } finally {
    await removeFixture(fixture);
  }
});

test("long-habitat Claude lifecycle uses the same alias ownership contract", async (t) => {
  if (process.platform !== "win32") {
    t.skip("Habitat junction evidence is Windows-only");
    return;
  }
  const fixture = await makeFixture();
  const evidenceRoot = path.join(fixture.root, "claude-evidence");
  let deleted = false;
  let isOff = true;
  const agent = {
    session_id: AGENT_ID,
    session_name: "offline-long-habitat-claude",
    provider: "claude",
    folder: fixture.workspacePath,
    is_off: true,
    resume_session: PROVIDER_SESSION,
  };
  const events = [
    {
      id: "claude-user-event",
      provider: "claude",
      kind: "message",
      role: "user",
      source: "provider_session",
      text: DELIVERY_MARKER,
      metadata: { provider_log: true, provider_session_id: PROVIDER_SESSION },
    },
    {
      id: "claude-assistant-event",
      provider: "claude",
      kind: "message",
      role: "assistant",
      source: "provider_session",
      text: DELIVERY_MARKER,
      metadata: { provider_log: true, provider_session_id: PROVIDER_SESSION, turn_id: "claude-turn-1" },
    },
  ];
  const invokeTauri = async (_driver, command) => {
    if (command === "list_agents") return deleted ? [] : [{ ...agent, is_off: isOff }];
    if (command === "resume_agent") {
      isOff = false;
      return null;
    }
    if (command === "pause_agent") {
      isOff = true;
      return null;
    }
    if (command === "load_agent_chat_transcript") return events;
    if (command === "list_conversations") {
      return { conversations: [{ agent_id: AGENT_ID, provider: "claude", conversation_id: "claude-conversation", record_count: 2 }] };
    }
    throw new Error(`unexpected Claude offline IPC command: ${command}`);
  };
  const runCliOk = async (_cliPath, _harness, args) => {
    assert.deepEqual(args, ["agent", "delete", agent.session_name, "--confirm", agent.session_name]);
    await removeIfPresent(fixture.aliasRecord);
    await removeIfPresent(fixture.aliasTarget);
    await removeIfPresent(fixture.slotRecord);
    await fs.rmdir(fixture.aliasSlot);
    deleted = true;
    return { status: 0, stdout: "", stderr: "" };
  };

  try {
    const preflight = await assertLongHabitatPrerequisites({
      home: fixture.home,
      agent,
      provider: "claude",
      workspacePath: fixture.workspacePath,
    });
    assert.equal(preflight.config, null);
    const result = await afterMaintainedProviderPause({
      driver: {},
      harness: { isolatedHome: fixture.home },
      cliPath: "offline-cli",
      agent,
      provider: "claude",
      workspacePath: fixture.workspacePath,
      preflight,
      markerPath: fixture.workspaceMarker,
      deliveryMarker: DELIVERY_MARKER,
      maintainedReport: {},
      evidenceRoot,
      invokeTauri,
      pauseRealProviderAgent: async (driver, sessionId) => {
        assert.equal(sessionId, AGENT_ID);
        await invokeTauri(driver, "pause_agent", { sessionId });
        assert.equal((await invokeTauri(driver, "list_agents"))[0].is_off, true);
      },
      waitForProviderInputReady: async (_driver, provider, sessionId) => {
        assert.equal(provider, "claude");
        assert.equal(sessionId, AGENT_ID);
      },
      runCliOk,
    });
    assert.equal(result.pause_resume_identity_preserved, true);
    assert.equal(result.alias_reused_across_pause_resume, true);
    assert.equal(result.alias_removed_after_explicit_agent_delete, true);
    assert.equal(result.external_marker_retained, true);
    assert.equal(result.codex_launch_artifacts, null);
  } finally {
    await removeFixture(fixture);
  }
});
