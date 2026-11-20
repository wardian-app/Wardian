// @tier ci — Data-only staging and path/owner/oracle checks; no app or provider.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createHash } from "node:crypto";
import { acquireHomeLock, HOME_LOCK_FILE } from "../lib/sessionHome.mjs";
import { chatBodyWitness, savedFixturePlan, freshSavedHome, stageSavedFixture, verifySavedFixture } from "../lib/chatSavedFixture.mjs";

const digest = (value) => createHash("sha256").update(value).digest("hex");
const rosterContract = JSON.parse(fs.readFileSync(
  new URL("../../crates/wardian-core/tests/fixtures/chat-saved-roster.json", import.meta.url), "utf8"));

function fixture(t, body = "An independently declared recent body.") {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-chat-saved-tests-"));
  t.after(() => {
    const relative = path.relative(os.tmpdir(), root);
    assert.ok(relative.startsWith("wardian-chat-saved-tests-") && !relative.includes(path.sep));
    fs.rmSync(root, { recursive: true, force: true });
  });
  const source = path.join(root, "source");
  fs.mkdirSync(source);
  const oracle = { canonical_saved_event_id: "saved-public-event", role: "assistant", provider: "codex",
    body_sha256: digest(body), text_characters: body.length, narrative_seq: 1, upper_bound_display_rank_from_tail: 1,
    saved_event_rows_after: 0, saved_narrative_rows_after: 0 };
  const write = (relative, value) => {
    const destination = path.join(source, relative);
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    fs.writeFileSync(destination, value);
    return { destination, bytes: Buffer.byteLength(value), sha256: digest(value) };
  };
  const jsonl = (value) => `${JSON.stringify(value)}\n`;
  const leaves = [
    ["manifest.json", JSON.stringify({ agent_id: "fixture-agent", conversation_id: "fixture-conversation", provider: "codex",
      provider_source_key: "fixture-source", provider_session_ids: ["fixture-native"], effective_logging: "enabled" })],
    ["conversation.jsonl", jsonl({ seq: 1, role: "assistant", text: body, event_refs: [oracle.canonical_saved_event_id] })],
    ["events.jsonl", jsonl({ id: oracle.canonical_saved_event_id, session_id: "fixture-agent", provider: "codex",
      kind: "message", role: "assistant", text: body })],
    ["sources.jsonl", jsonl({ id: "fixture-source" })],
    ["turns.jsonl", jsonl({ id: "fixture-turn" })],
  ].map(([kind, value]) => ({ kind, ...write(`archive/${kind}`, value) }));
  leaves.push({ kind: "full-index", ...write("index.jsonl", jsonl({ conversation_id: "fixture-conversation" })) });
  leaves.push({ kind: "artifact:fixture.txt", ...write("archive/artifacts/fixture.txt", "original fixture artifact") });
  const config = write("selected-config.json", JSON.stringify({ session_id: "fixture-agent", session_name: "Saved Fixture",
    resume_session: "fixture-native", provider: "codex", provider_config: { type: "codex" }, model: "fixture-model", is_off: false,
    folder: "unowned-original-folder", include_directories: ["unowned-original-include"],
    custom_args: ["unowned-original-argument"], conversation_logging: "default", session_persistence: "resume" }));
  const oracleFile = write("independent-oracle.json", JSON.stringify(oracle));
  const manifest = { fixture_root: source, source_drift: false, original_sources_changed: false,
    identity: { owner: "fixture-agent", conversation_id: "fixture-conversation", provider: "codex",
      source_key: "fixture-source", native_session: "fixture-native", archived_effective_logging: "enabled",
      source_config_policy: "default" }, files: leaves, artifact_count: 1,
    copied_bytes: leaves.reduce((sum, file) => sum + file.bytes, 0), selected_config_file: "selected-config.json",
    selected_config_sha256: config.sha256, recent_oracle_file: "independent-oracle.json",
    recent_oracle_sha256: oracleFile.sha256, recent_oracle: oracle };
  const manifestPath = path.join(source, "fixture-manifest.json");
  const save = () => fs.writeFileSync(manifestPath, JSON.stringify(manifest));
  const saveOracle = () => {
    manifest.recent_oracle_sha256 = write("independent-oracle.json", JSON.stringify(manifest.recent_oracle)).sha256;
    save();
  };
  const replaceLeaf = (relative, value) => {
    const original = manifest.files.find((file) => file.destination === path.join(source, relative));
    manifest.copied_bytes -= original.bytes;
    Object.assign(original, write(relative, value));
    manifest.copied_bytes += original.bytes;
    save();
  };
  save();
  const harness = { isolatedHome: path.join(root, "owned-home"), runId: "fixture-run" };
  const claim = () => { harness.homeLock = acquireHomeLock({ home: harness.isolatedHome, runId: harness.runId }).lock; };
  return { root, source, body, manifest, manifestPath, save, saveOracle, replaceLeaf, harness, claim };
}

test("external archive plan preserves declared identities and maps only owned archive/index leaves", (t) => {
  const f = fixture(t);
  const plan = savedFixturePlan(f.manifest);
  assert.equal(plan.files.length, 7);
  assert.ok(plan.files.every((file) => file.output.startsWith("agents/fixture-agent/conversations/")));
  assert.equal(plan.relative("selected-config.json"), "selected-config.json");
  assert.throws(() => plan.relative("../foreign.json"), /escape/);
  for (const mutate of [
    (m) => { m.files[0].destination = path.join(f.source, "..", "foreign.json"); },
    (m) => { m.files.push({ ...m.files[0] }); },
    (m) => { m.identity.owner = "../foreign"; },
    (m) => { m.copied_bytes += 1; },
    (m) => { m.files[0].sha256 = "invalid"; },
  ]) {
    const copy = structuredClone(f.manifest); mutate(copy);
    assert.throws(() => savedFixturePlan(copy));
  }
});

test("staging publishes normal startup settings and preserves Off archive identity without a logging override", (t) => {
  const f = fixture(t);
  const fresh = freshSavedHome(f.harness); f.claim();
  const staged = stageSavedFixture(f.harness, f.manifestPath, fresh);
  const configs = JSON.parse(fs.readFileSync(path.join(f.harness.isolatedHome, "settings", "state.json"), "utf8"));
  assert.equal(configs.length, 1);
  const [config] = configs;
  assert.equal(fs.existsSync(path.join(f.harness.isolatedHome, "state.json")), false);
  assert.equal(config.session_id, "fixture-agent"); assert.equal(config.resume_session, "fixture-native");
  assert.equal(config.is_off, true); assert.equal(config.agent_class, "TestClass");
  assert.equal(config.conversation_logging, "default"); assert.deepEqual(config.include_directories, []);
  assert.equal(config.custom_args, null); assert.notEqual(config.folder, "unowned-original-folder");
  assert.deepEqual(fs.readdirSync(staged.codexHome), []);
  assert.deepEqual(fs.readdirSync(staged.claudeConfigDir), []);
  assert.equal(path.dirname(staged.claudeConfigDir), f.harness.isolatedHome);
  assert.equal(staged.body, f.body); assert.equal(staged.eventId, "saved-public-event");
  verifySavedFixture(f.harness, staged);
  assert.throws(() => stageSavedFixture(f.harness, f.manifestPath, fresh), /precede/);
});

test("actual staged roster matches the shared production serde contract", (t) => {
  const f = fixture(t);
  const fresh = freshSavedHome(f.harness); f.claim();
  const staged = stageSavedFixture(f.harness, f.manifestPath, fresh);
  const configs = JSON.parse(fs.readFileSync(path.join(f.harness.isolatedHome, "settings", "state.json"), "utf8"));
  assert.equal(configs.length, 1);
  assert.equal(configs[0].folder, path.join(f.harness.isolatedHome, "saved-chat-workspace"));
  // Only the owned workspace path varies; retain every serialized key and value.
  assert.deepEqual(configs.map((config) => ({ ...config, folder: "<owned-workspace>" })), rosterContract);
  verifySavedFixture(f.harness, staged);
});

test("existing startup settings cannot be overwritten after fresh-home ownership was claimed", (t) => {
  const f = fixture(t);
  const fresh = freshSavedHome(f.harness); f.claim();
  const settings = path.join(f.harness.isolatedHome, "settings");
  fs.mkdirSync(settings);
  const state = path.join(settings, "state.json");
  fs.writeFileSync(state, "original-startup-state");
  assert.throws(() => stageSavedFixture(f.harness, f.manifestPath, fresh), /precede/);
  assert.equal(fs.readFileSync(state, "utf8"), "original-startup-state");
  assert.equal(fs.existsSync(path.join(f.harness.isolatedHome, "agents")), false);
});

test("a settings directory link cannot redirect staged startup state outside the owned home", (t) => {
  const f = fixture(t);
  const fresh = freshSavedHome(f.harness); f.claim();
  fs.symlinkSync(f.source, path.join(f.harness.isolatedHome, "settings"), "junction");
  assert.throws(() => stageSavedFixture(f.harness, f.manifestPath, fresh), /ordinary owned directory/);
  assert.equal(fs.existsSync(path.join(f.source, "state.json")), false);
  assert.equal(fs.existsSync(path.join(f.harness.isolatedHome, "agents")), false);
});

test("rendered witnesses are prepared from frozen raw Markdown before staging, without a response-selected oracle", (t) => {
  const plain = "An independently declared recent body.";
  assert.deepEqual(chatBodyWitness(plain), { kind: "whole", text: plain, bodySha256: digest(plain) });
  const body = "**An independently rendered assistant witness.**";
  const f = fixture(t, body);
  const fresh = freshSavedHome(f.harness); f.claim();
  const staged = stageSavedFixture(f.harness, f.manifestPath, fresh);
  assert.equal(staged.body, body);
  assert.deepEqual(staged.witness, { kind: "needle", text: "An independently rendered assistant witness.", bodySha256: digest(body) });
  assert.equal(staged.witness.bodySha256, f.manifest.recent_oracle.body_sha256);
});

test("a rich body without a safe independent rendered witness fails fixture eligibility before agent staging", (t) => {
  for (const body of ["```text\nAn independently declared code-only body.\n```", "![An image-only body](https://example.invalid/fixture.png)",
    "**An independently rendered assistant witness.**\n\n![An independently rendered assistant witness.](https://example.invalid/fixture.png)",
    "**An independently rendered &amp; visible witness.**\n\n![An independently rendered &amp; visible witness.](https://example.invalid/fixture.png)"]) {
    const f = fixture(t, body); const fresh = freshSavedHome(f.harness); f.claim();
    assert.throws(() => stageSavedFixture(f.harness, f.manifestPath, fresh), /fixture eligibility/);
    assert.equal(fs.existsSync(path.join(f.harness.isolatedHome, "agents")), false);
    assert.equal(fs.existsSync(path.join(f.harness.isolatedHome, "state.json")), false);
    assert.equal(fs.existsSync(path.join(f.harness.isolatedHome, "settings", "state.json")), false);
  }
});

test("reused or foreign homes are refused before reset or staging", (t) => {
  const f = fixture(t); f.claim();
  const sentinel = path.join(f.harness.isolatedHome, "state.json");
  fs.writeFileSync(sentinel, "original-state");
  assert.throws(() => freshSavedHome(f.harness), /fresh home before reset/);
  assert.equal(fs.readFileSync(sentinel, "utf8"), "original-state");
  const other = fixture(t);
  const fresh = freshSavedHome(other.harness); other.claim();
  fs.writeFileSync(path.join(other.harness.isolatedHome, HOME_LOCK_FILE), JSON.stringify({ runId: "foreign" }));
  assert.throws(() => stageSavedFixture(other.harness, other.manifestPath, fresh), /claim changed/);
  assert.equal(fs.existsSync(path.join(other.harness.isolatedHome, "agents")), false);
});

test("changed leaves, mismatched lineage and unproved eligibility fail before any agent staging", (t) => {
  for (const mutate of [
    (f) => fs.appendFileSync(path.join(f.source, "archive", "sources.jsonl"), "changed"),
    (f) => { f.manifest.identity.native_session = "foreign-native"; f.save(); },
    (f) => { f.manifest.recent_oracle.upper_bound_display_rank_from_tail = 81; f.saveOracle(); },
    (f) => { f.manifest.recent_oracle.saved_event_rows_after = 1;
      f.manifest.recent_oracle.upper_bound_display_rank_from_tail = 2; f.saveOracle(); },
    (f) => { f.replaceLeaf("archive/events.jsonl", JSON.stringify({ id: "saved-public-event", session_id: "fixture-agent",
      provider: "codex", kind: "message", role: "assistant", text: "Changed body with a freshly declared leaf hash" }) + "\n"); },
  ]) {
    const f = fixture(t); const fresh = freshSavedHome(f.harness); f.claim(); mutate(f);
    assert.throws(() => stageSavedFixture(f.harness, f.manifestPath, fresh));
    assert.equal(fs.existsSync(path.join(f.harness.isolatedHome, "agents")), false);
  }
});

test("a directory link cannot redirect fixture reads, and post-run archive mutation cannot be reported unchanged", (t) => {
  const f = fixture(t);
  fs.renameSync(path.join(f.source, "archive", "artifacts"), path.join(f.source, "original-artifacts"));
  fs.symlinkSync(path.join(f.source, "original-artifacts"), path.join(f.source, "archive", "artifacts"),
    process.platform === "win32" ? "junction" : "dir");
  const fresh = freshSavedHome(f.harness); f.claim();
  assert.throws(() => stageSavedFixture(f.harness, f.manifestPath, fresh), /links/);
  const other = fixture(t); const otherFresh = freshSavedHome(other.harness); other.claim();
  const staged = stageSavedFixture(other.harness, other.manifestPath, otherFresh);
  fs.appendFileSync(path.join(other.harness.isolatedHome, staged.staged[0].output), "changed");
  assert.throws(() => verifySavedFixture(other.harness, staged), /size changed/);
});
