import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { HOME_LOCK_DIRECTORY, readHomeLock } from "./sessionHome.mjs";

const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
const component = (value) => typeof value === "string" && /^[a-z0-9][a-z0-9._-]{0,200}$/i.test(value)
  && value !== "." && value !== "..";
const hash = (value) => typeof value === "string" && /^[a-f0-9]{64}$/i.test(value);

/**
 * Prepare a text witness from the independently frozen raw body before App start.
 * Reuse ChatMarkdown's parser/options, without rendering UI or consulting an RPC.
 * Code, images and controls have custom presentation; refuse those as witnesses.
 */
export function chatBodyWitness(body) {
  assert.ok(typeof body === "string" && body.length > 0 && body.length <= 65536,
    "Saved fixture eligibility: bounded independent raw body required");
  const strings = [];
  const candidates = [];
  let visited = 0;
  const walk = (node, eligible = true) => {
    assert.ok(++visited <= 8192, "Saved fixture eligibility: Markdown witness tree bound");
    if (typeof node === "string") {
      strings.push(node);
      if (eligible) candidates.push(...node.split(/\r?\n/).map((line) => line.trim()).filter((line) => line.length >= 16));
    } else if (Array.isArray(node)) {
      for (const child of node) walk(child, eligible);
    } else if (node && typeof node === "object") {
      // ChatMarkdown renders image alt/URL as text. Include that presentation
      // in collision checks, while never selecting an image as the witness.
      if (node.type === "img") {
        strings.push(node.props?.alt?.trim() || "Image");
        if (typeof node.props?.src === "string") strings.push(node.props.src);
      }
      walk(node.props?.children, eligible && !["pre", "code", "img", "input"].includes(node.type));
    }
  };
  walk(Markdown({ children: body, remarkPlugins: [[remarkGfm, { singleTilde: false }]], skipHtml: true }));
  const bodySha256 = digest(body);
  if (strings.length === 1 && strings[0] === body) return { kind: "whole", text: body, bodySha256 };
  const rendered = strings.join("");
  for (const candidate of candidates.toReversed()) {
    const text = candidate.slice(0, 128);
    const at = rendered.indexOf(text);
    const rawAt = body.indexOf(text);
    if (at >= 0 && rendered.indexOf(text, at + 1) === -1
      && (rawAt < 0 || body.indexOf(text, rawAt + 1) === -1)) return { kind: "needle", text, bodySha256 };
  }
  assert.fail("Saved fixture eligibility: no independent bounded rendered text witness; do not start App");
}

/** Pure destination validation; original native/source paths are never inputs. */
export function savedFixturePlan(manifest) {
  assert.ok(component(manifest.identity?.owner) && component(manifest.identity?.conversation_id), "invalid archive identity");
  assert.equal(manifest.identity.provider, "codex", "external Saved fixture must retain its declared Codex identity");
  assert.ok(manifest.source_drift === false && manifest.original_sources_changed === false, "unqualified fixture capture");
  assert.ok(Array.isArray(manifest.files) && manifest.files.length > 0 && manifest.files.length <= 1024, "fixture file bound");
  const originalPath = path.win32.isAbsolute(manifest.fixture_root) ? path.win32 : path.posix;
  assert.ok(originalPath.isAbsolute(manifest.fixture_root), "absolute fixture root required");
  const relative = (value) => {
    assert.equal(typeof value, "string", "fixture destination required");
    const result = (originalPath.isAbsolute(value) ? originalPath.relative(manifest.fixture_root, value) : value).replaceAll("\\", "/");
    assert.ok(result && !originalPath.isAbsolute(result) && result.split("/").every(component), "fixture path escape");
    return result;
  };
  const prefix = `agents/${manifest.identity.owner}/conversations`;
  const used = new Set();
  let total = 0;
  const files = manifest.files.map((file) => {
    const input = relative(file.destination);
    assert.ok(hash(file.sha256) && Number.isSafeInteger(file.bytes) && file.bytes > 0
      && file.bytes <= 64 * 1024 * 1024, "invalid bounded fixture leaf");
    let output;
    if (file.kind === "full-index" && input === "index.jsonl") output = `${prefix}/index.jsonl`;
    else if (["manifest.json", "conversation.jsonl", "events.jsonl", "sources.jsonl", "turns.jsonl"].includes(file.kind)
      && input === `archive/${file.kind}`) output = `${prefix}/${manifest.identity.conversation_id}/${file.kind}`;
    else if (file.kind.startsWith("artifact:") && input.startsWith("archive/artifacts/")
      && input.split("/").length === 3 && file.kind.slice(9) === input.split("/")[2]) {
      output = `${prefix}/${manifest.identity.conversation_id}/artifacts/${input.split("/")[2]}`;
    } else assert.fail("unexpected Saved fixture leaf mapping");
    assert.ok(!used.has(output.toLowerCase()), "duplicate fixture destination");
    used.add(output.toLowerCase());
    total += file.bytes;
    return { input, output, bytes: file.bytes, sha256: file.sha256.toLowerCase() };
  });
  assert.equal(total, manifest.copied_bytes, "fixture byte total mismatch");
  assert.ok(total <= 128 * 1024 * 1024, "fixture aggregate bound");
  for (const leaf of ["manifest.json", "conversation.jsonl", "events.jsonl", "sources.jsonl", "turns.jsonl"]) {
    assert.ok(files.some((file) => file.input === `archive/${leaf}`), "missing archive leaf");
  }
  assert.ok(files.some((file) => file.input === "index.jsonl"), "missing original index");
  assert.equal(files.filter((file) => file.input.startsWith("archive/artifacts/")).length, manifest.artifact_count);
  return { files, relative, total };
}

/** Reject links and path escape before opening a private metadata/archive leaf. */
function plainFile(root, relative, expected, maxBytes = 64 * 1024 * 1024) {
  const parts = relative.split("/");
  assert.ok(parts.length && parts.every(component), "unsafe fixture relative path");
  let current = root;
  assert.equal(path.resolve(fs.realpathSync(root)), path.resolve(root), "fixture root must not traverse a link");
  for (const part of parts) {
    current = path.join(current, part);
    assert.equal(fs.lstatSync(current).isSymbolicLink(), false, "fixture links are forbidden");
  }
  const before = fs.lstatSync(current, { bigint: true });
  assert.ok(before.isFile() && !before.isSymbolicLink(), "plain fixture leaf required");
  const fd = fs.openSync(current, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0));
  try {
    const stat = fs.fstatSync(fd, { bigint: true });
    assert.equal(stat.dev, before.dev); assert.equal(stat.ino, before.ino, "fixture leaf changed during open");
    assert.equal(path.resolve(fs.realpathSync(current)), path.resolve(current), "fixture path became a link");
    assert.ok(stat.isFile() && stat.size > 0n && stat.size <= BigInt(maxBytes), "bounded plain fixture file required");
    const bytes = Buffer.alloc(Number(stat.size));
    let offset = 0;
    while (offset < bytes.length) {
      const count = fs.readSync(fd, bytes, offset, bytes.length - offset, offset);
      assert.ok(count > 0, "fixture truncated while reading");
      offset += count;
    }
    assert.equal(fs.readSync(fd, Buffer.alloc(1), 0, 1, offset), 0, "fixture grew while reading");
    const sha256 = digest(bytes);
    const after = fs.lstatSync(current, { bigint: true });
    assert.equal(after.dev, stat.dev); assert.equal(after.ino, stat.ino, "fixture leaf replaced while reading");
    if (expected) {
      if (expected.bytes !== undefined) assert.equal(Number(stat.size), expected.bytes, "fixture leaf size changed");
      assert.equal(sha256, expected.sha256.toLowerCase(), "fixture leaf hash changed");
    }
    return { bytes, sha256, dev: String(stat.dev), ino: String(stat.ino) };
  } finally { fs.closeSync(fd); }
}

/** Called before prepareIsolatedHome: a reusable stateful home is not a cold fixture. */
export function freshSavedHome(harness) {
  const home = path.resolve(harness.isolatedHome);
  if (!fs.existsSync(home)) return { home, runId: harness.runId, absent: true };
  assert.equal(path.resolve(fs.realpathSync(home)), home, "owned home must not traverse a link");
  assert.ok(fs.readdirSync(home).every((name) => name === HOME_LOCK_DIRECTORY), "external fixture needs a fresh home before reset");
  const owner = readHomeLock(home);
  assert.equal(owner?.runId, harness.runId, "foreign native home claim");
  const stat = fs.statSync(home, { bigint: true });
  return { home, runId: harness.runId, dev: String(stat.dev), ino: String(stat.ino), absent: false };
}

/** Validate all source bytes/oracle first, then stage data only before the first app start. */
export function stageSavedFixture(harness, manifestPath, fresh) {
  const home = path.resolve(harness.isolatedHome);
  assert.equal(fresh?.home, home, "missing fresh-home witness");
  assert.equal(fresh.runId, harness.runId);
  assert.equal(readHomeLock(home)?.runId, harness.runId, "owned home claim changed");
  assert.equal(harness.homeLock?.runId, harness.runId, "prepareIsolatedHome must own this claim");
  assert.equal(path.resolve(fs.realpathSync(home)), home, "owned home must not traverse a link");
  if (!fresh.absent) {
    const stat = fs.statSync(home, { bigint: true });
    assert.equal(String(stat.dev), fresh.dev); assert.equal(String(stat.ino), fresh.ino);
  }
  assert.ok(!fs.readdirSync(home).some((name) => /^(agents|state\.json|state\.db|\.webview2-)/.test(name)),
    "fixture staging must precede agent state and first app start");
  const settings = path.join(home, "settings");
  const settingsStat = fs.lstatSync(settings, { throwIfNoEntry: false });
  assert.ok(!settingsStat || (settingsStat.isDirectory() && !settingsStat.isSymbolicLink()),
    "fixture settings must be an ordinary owned directory");
  const statePath = path.join(settings, "state.json");
  assert.ok(!fs.lstatSync(statePath, { throwIfNoEntry: false }),
    "fixture staging must precede startup state and first app start");
  const root = path.dirname(path.resolve(manifestPath));
  const metadata = plainFile(root, path.basename(manifestPath), null, 4 * 1024 * 1024);
  const manifest = JSON.parse(metadata.bytes.toString("utf8"));
  const plan = savedFixturePlan(manifest);
  const configBytes = plainFile(root, plan.relative(manifest.selected_config_file),
    { sha256: manifest.selected_config_sha256 }, 64 * 1024);
  const config = JSON.parse(configBytes.bytes.toString("utf8"));
  assert.equal(config.session_id, manifest.identity.owner); assert.equal(config.provider, manifest.identity.provider);
  assert.equal(config.resume_session, manifest.identity.native_session);
  assert.equal(config.provider_config?.type, config.provider);
  assert.ok(["default", "enabled", "disabled"].includes(config.conversation_logging), "unknown source logging policy");
  assert.ok(/^[a-z0-9 _.-]{1,128}$/i.test(config.session_name), "unsupported fixture Roster name");
  const oracleBytes = plainFile(root, plan.relative(manifest.recent_oracle_file), null, 64 * 1024);
  assert.equal(oracleBytes.sha256, manifest.recent_oracle_sha256.toLowerCase(), "independent oracle hash changed");
  const oracle = JSON.parse(oracleBytes.bytes.toString("utf8"));
  assert.deepEqual(oracle, manifest.recent_oracle, "embedded oracle differs from independent oracle");
  assert.ok(hash(oracle.body_sha256) && oracle.role === "assistant" && oracle.provider === config.provider
    && Number.isSafeInteger(oracle.upper_bound_display_rank_from_tail) && oracle.upper_bound_display_rank_from_tail > 0
    && oracle.upper_bound_display_rank_from_tail <= 80, "unproved recent target eligibility");
  const sources = plan.files.map((file) => ({ ...file, ...plainFile(root, file.input, file) }));
  const json = (input) => JSON.parse(sources.find((file) => file.input === input).bytes.toString("utf8"));
  const archive = json("archive/manifest.json");
  assert.equal(archive.agent_id, config.session_id); assert.equal(archive.provider, config.provider);
  assert.equal(archive.conversation_id, manifest.identity.conversation_id);
  assert.equal(archive.provider_source_key, manifest.identity.source_key);
  assert.equal(archive.effective_logging, manifest.identity.archived_effective_logging);
  assert.equal(config.conversation_logging, manifest.identity.source_config_policy);
  assert.ok(archive.provider_session_ids.includes(config.resume_session), "native identity is absent from original archive");
  const records = (input) => {
    const text = sources.find((file) => file.input === input).bytes.toString("utf8");
    assert.ok(text.endsWith("\n"), "incomplete original JSONL");
    return text.trimEnd().split("\n").map((line) => JSON.parse(line));
  };
  const events = records("archive/events.jsonl");
  const narratives = records("archive/conversation.jsonl");
  const eventIndex = events.findIndex((row) => row.id === oracle.canonical_saved_event_id);
  const narrativeIndex = narratives.findIndex((row) => row.seq === oracle.narrative_seq);
  const event = events[eventIndex];
  const narrative = narratives[narrativeIndex];
  assert.ok(event && narrative && event.session_id === config.session_id && event.provider === config.provider
    && event.kind === "message" && event.role === "assistant" && narrative.role === "assistant"
    && narrative.event_refs.includes(event.id), "independent target lineage mismatch");
  assert.equal(typeof event.text, "string"); assert.equal(event.text, narrative.text);
  assert.equal(digest(event.text), oracle.body_sha256.toLowerCase(), "independent body hash mismatch");
  assert.equal(event.text.length, oracle.text_characters);
  const witness = chatBodyWitness(event.text);
  assert.equal(events.filter((row) => row.id === event.id).length, 1, "independent target event is not unique");
  assert.equal(events.length - eventIndex - 1, oracle.saved_event_rows_after, "saved event tail rank changed");
  assert.equal(narratives.length - narrativeIndex - 1, oracle.saved_narrative_rows_after, "saved narrative tail rank changed");
  assert.ok(Math.max(oracle.saved_event_rows_after, oracle.saved_narrative_rows_after) + 1
    <= oracle.upper_bound_display_rank_from_tail, "unproved independent final80 rank");
  assert.ok(records("index.jsonl").some((row) => row.conversation_id === archive.conversation_id), "original index omits archive");
  const workspace = path.join(home, "saved-chat-workspace");
  const codexHome = path.join(home, "saved-chat-empty-codex-home");
  const claudeConfigDir = path.join(home, "saved-chat-empty-claude-config");
  fs.mkdirSync(workspace); fs.mkdirSync(codexHome); fs.mkdirSync(claudeConfigDir);
  const staged = sources.map((file) => {
    assert.equal(readHomeLock(home)?.runId, harness.runId, "owned home claim changed during staging");
    const output = path.join(home, ...file.output.split("/"));
    fs.mkdirSync(path.dirname(output), { recursive: true });
    fs.writeFileSync(output, file.bytes, { flag: "wx" });
    const stat = fs.statSync(output, { bigint: true });
    return { output: file.output, bytes: file.bytes.length, sha256: file.sha256, dev: String(stat.dev), ino: String(stat.ino) };
  });
  // Preserve provider/native/agent identity and policy, while isolating launch configuration.
  const isolated = { session_id: config.session_id, session_name: config.session_name, description: "",
    agent_class: "TestClass", folder: workspace, git_worktree: null, git_worktree_source: null, git_worktree_folder: null,
    resume_session: config.resume_session, is_off: true, provider: config.provider, debug: null, model: config.model,
    include_directories: [], system_include_directories: [], custom_args: null, session_persistence: config.session_persistence,
    conversation_logging: config.conversation_logging, provider_config: { type: config.provider } };
  // Startup reads this current-layout path; root state.json has never been a
  // migration source (the legacy filename was wardian_state.json).
  fs.mkdirSync(settings, { recursive: true });
  fs.writeFileSync(statePath, JSON.stringify([isolated]), { flag: "wx" });
  return { sessionId: config.session_id, name: config.session_name, provider: config.provider, policy: config.conversation_logging,
    conversation: archive.conversation_id, eventId: event.id, body: event.text, witness, codexHome, claudeConfigDir, staged,
    metadata: { manifestSha256: metadata.sha256, oracleSha256: oracleBytes.sha256, leaves: staged.length, bytes: plan.total,
      bodySha256: witness.bodySha256, witnessKind: witness.kind,
      nativeSource: "not staged; empty owned CODEX_HOME and CLAUDE_CONFIG_DIR; original external paths never opened by fixture",
      eligibility: "independent final80 rank; viewport still requires actual paint",
      cacheState: "cold process only; validation/staging warmed filesystem" } };
}

/** Original staged archive/index bytes and inodes must survive both Off measurements. */
export function verifySavedFixture(harness, fixture) {
  for (const file of fixture.staged) {
    const current = plainFile(harness.isolatedHome, file.output, file);
    assert.equal(current.dev, file.dev); assert.equal(current.ino, file.ino, "Saved archive inode replaced");
  }
}
