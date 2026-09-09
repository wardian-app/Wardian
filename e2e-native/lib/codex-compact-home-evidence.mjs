// Private real-harness evidence and cleanup, not a product mapping API.
import fs from "node:fs/promises";
import { constants } from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { isDeepStrictEqual } from "node:util";

const RECORD = ".wardian-codex-home.json";
const READY = ".wardian-codex-home-ready.json";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu;
const TOKEN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu;
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
const requireThat = (condition, message) => { if (!condition) throw new Error(message); };

function spelling(value) {
  requireThat(typeof value === "string" && value === value.toWellFormed() && !value.includes("\0"), "Invalid compact-home path encoding");
  if (process.platform === "win32") {
    if (value.startsWith("\\\\?\\UNC\\")) value = `\\\\${value.slice(8)}`;
    else if (value.startsWith("\\\\?\\")) value = value.slice(4);
  }
  requireThat(path.isAbsolute(value), "Compact-home paths must be absolute");
  requireThat(path.normalize(value) === value, "Compact-home paths must use canonical component spelling");
  return value;
}

async function present(file) {
  try { return await fs.lstat(file, { bigint: true }); }
  catch (error) { if (error.code === "ENOENT") return null; throw error; }
}

// Walk every directory component without following links, including parents of
// external compact roots. Callers supply canonical OS spellings (/private on Mac).
async function plainDirectory(directory, privateLeaf = false) {
  directory = spelling(directory);
  let current = path.parse(directory).root;
  let stat = await fs.lstat(current, { bigint: true });
  for (const component of path.relative(current, directory).split(path.sep).filter(Boolean)) {
    current = path.join(current, component);
    stat = await fs.lstat(current, { bigint: true });
    requireThat(stat.isDirectory() && !stat.isSymbolicLink(), "Compact-home directory parent is linked or not a directory");
  }
  requireThat(spelling(await fs.realpath(directory)) === directory, "Compact-home directory resolves outside its recorded spelling");
  if (privateLeaf && process.platform !== "win32") {
    requireThat(stat.uid === BigInt(process.getuid()) && (stat.mode & 0o077n) === 0n, "Compact-home root or slot is not private to this user");
  }
  return stat;
}

function identity(stat) { return [stat.dev, stat.ino]; }
function recordedIdentity(value) {
  requireThat(Array.isArray(value) && value.length === 2, "Missing compact-home directory identity");
  return value.map((number) => {
    requireThat(typeof number === "bigint" || (typeof number === "number" && Number.isSafeInteger(number)), "Invalid compact-home directory identity");
    const integer = BigInt(number);
    requireThat(integer >= 0n && integer <= 0xffffffffffffffffn, "Compact-home identity exceeds u64");
    return integer;
  });
}

function sameIdentity(stat, expected) {
  requireThat(isDeepStrictEqual(identity(stat), expected), "Compact-home directory identity changed");
}

async function privateFile(file, maximum = 65536) {
  await plainDirectory(path.dirname(file));
  const before = await fs.lstat(file, { bigint: true });
  requireThat(before.isFile() && !before.isSymbolicLink() && before.nlink === 1n && before.size <= BigInt(maximum), "Evidence or credential file is linked, oversized, or not regular");
  const handle = await fs.open(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  try {
    const opened = await handle.stat({ bigint: true });
    sameIdentity(opened, identity(before));
    const bytes = await handle.readFile();
    const after = await handle.stat({ bigint: true });
    requireThat(bytes.length <= maximum && after.size === before.size && after.mtimeNs === before.mtimeNs && after.nlink === 1n,
      "Evidence or credential file changed while reading");
    return { bytes, stat: after, sha256: digest(bytes) };
  } finally { await handle.close(); }
}

async function readRecord(file, fields) {
  const stored = await privateFile(file);
  let data;
  try {
    data = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(stored.bytes), (_key, value, context) => {
      // Node's ordinary JSON number would round Windows file IDs. A runtime
      // without source-aware revivers must fail closed on unsafe integers.
      if (typeof value !== "number" || Number.isSafeInteger(value)) return value;
      requireThat(/^(0|[1-9][0-9]*)$/u.test(context?.source ?? ""), "Lossless u64 JSON decoding is required");
      return BigInt(context.source);
    });
  } catch { throw new Error("Malformed compact-home ownership record"); }
  requireThat(data && typeof data === "object" && !Array.isArray(data) &&
    isDeepStrictEqual(Object.keys(data).sort(), [...fields].sort()), "Unexpected compact-home ownership record fields");
  return { path: file, sha256: stored.sha256, data };
}

const INTENT_FIELDS = ["version", "token", "agent_id", "wardian_home", "source", "target", "source_identity", "snapshot"];
const READY_FIELDS = ["version", "token", "target_identity", "copied"];

async function inspect(home, agentId) {
  requireThat(UUID.test(agentId), "Credential cleanup requires a full captured agent UUID");
  home = spelling(home);
  await plainDirectory(home);
  const agent = path.join(home, "agents", agentId);
  const habitat = path.join(agent, "habitat");
  const source = path.join(habitat, ".codex");
  await plainDirectory(habitat);
  const recordPath = path.join(agent, RECORD);
  const sourceStat = await present(source);
  if (!await present(recordPath)) {
    requireThat(!sourceStat?.isSymbolicLink(), "Linked fixture home has no ownership record; credentials retained");
    return { home, source, directories: sourceStat ? [{ path: source, identity: identity(await plainDirectory(source)) }] : [], evidence: null };
  }
  const record = await readRecord(recordPath, INTENT_FIELDS);
  const intent = record.data;
  requireThat(intent.version === 1 && TOKEN.test(intent.token) && intent.agent_id === agentId &&
    spelling(intent.wardian_home) === home && spelling(intent.source) === source && /^[0-9a-f]{64}$/u.test(intent.snapshot),
  "Compact-home intent does not belong to this isolated home and captured agent");
  const sourceIdentity = recordedIdentity(intent.source_identity);
  const target = spelling(intent.target);
  const slot = path.dirname(target);
  const root = path.dirname(slot);
  requireThat(path.basename(target) === "h" && /^[0-9a-f]{8}$/u.test(path.basename(slot)), "Invalid compact-home slot layout");
  await plainDirectory(root, true);
  await plainDirectory(slot, true);
  const slotRecord = await readRecord(path.join(slot, RECORD), INTENT_FIELDS);
  requireThat(isDeepStrictEqual(slotRecord.data, intent), "Agent and slot ownership records disagree; credentials retained");
  const staging = path.join(slot, "c");
  const backup = path.join(habitat, `.codex-precompact-${intent.token}`);
  const targetStat = await present(target);
  const stagingStat = await present(staging);
  const backupStat = await present(backup);
  const readyPath = path.join(slot, READY);
  const ready = await present(readyPath) ? await readRecord(readyPath, READY_FIELDS) : null;
  const directories = [];
  const add = async (directory, expected) => {
    const stat = await plainDirectory(directory);
    sameIdentity(stat, expected);
    directories.push({ path: directory, identity: identity(stat) });
  };
  if (!ready) {
    requireThat(!targetStat && !stagingStat && !backupStat && sourceStat && !sourceStat.isSymbolicLink(),
      "Incomplete compact migration has unverified copies; credentials retained");
    await add(source, sourceIdentity);
  } else {
    const receipt = ready.data;
    requireThat(receipt.version === 1 && receipt.token === intent.token && typeof receipt.copied === "boolean", "Foreign compact-home Ready receipt");
    const targetIdentity = recordedIdentity(receipt.target_identity);
    requireThat(Boolean(targetStat) !== Boolean(stagingStat), "Compact migration has ambiguous target/staging topology");
    await add(targetStat ? target : staging, targetIdentity);
    if (!receipt.copied) {
      requireThat(targetStat && !stagingStat && !backupStat && (!sourceStat || sourceStat.isSymbolicLink()) &&
        isDeepStrictEqual(targetIdentity, sourceIdentity), "Renamed compact-home identity or topology changed");
    } else {
      const plainSource = sourceStat && !sourceStat.isSymbolicLink();
      requireThat(Boolean(plainSource) !== Boolean(backupStat), "Copied compact-home original/backup topology is ambiguous");
      requireThat(!stagingStat || (plainSource && !backupStat), "Compact staging has unexpected publication topology");
      await add(plainSource ? source : backup, sourceIdentity);
    }
    if (sourceStat?.isSymbolicLink()) {
      requireThat(targetStat && !stagingStat && spelling(await fs.realpath(source)) === target,
        "Logical Codex home points outside the authenticated compact target");
    }
  }
  const physical = sourceStat?.isSymbolicLink() ? spelling(await fs.realpath(source)) : null;
  return { home, source, directories, evidence: {
    agent_id: agentId, token: intent.token, wardian_home: home, source, target, physical_home: physical,
    source_identity: sourceIdentity.map(String), target_identity: ready ? recordedIdentity(ready.data.target_identity).map(String) : null,
    copied: ready?.data.copied ?? null,
    records: [record, slotRecord, ...(ready ? [ready] : [])].map(({ path: file, sha256 }) => ({ path: file, sha256 })),
    complete: Boolean(physical && ready),
  } };
}

export async function compactHomeEvidence(home, agentId) {
  const mapping = await inspect(home, agentId);
  requireThat(mapping.evidence?.complete, "Deep isolated Codex habitat did not complete compact-home relocation");
  const socket = path.join(mapping.evidence.physical_home, "app-server-control", "app-server-control.sock");
  const logicalSocket = path.join(mapping.source, "app-server-control", "app-server-control.sock");
  const capacity = process.platform === "darwin" ? 104 : 108;
  requireThat(Buffer.byteLength(logicalSocket, "utf8") >= capacity, "Fixture does not exercise an over-limit logical socket path");
  requireThat(Buffer.byteLength(socket, "utf8") < capacity, "Canonical compact socket path exceeds the provider limit");
  return { ...mapping.evidence, socket_path: socket, socket_bytes: Buffer.byteLength(socket, "utf8"),
    socket_capacity: capacity, logical_socket_bytes: Buffer.byteLength(logicalSocket, "utf8") };
}

async function removeCredential(directory, expectedIdentity) {
  sameIdentity(await plainDirectory(directory), expectedIdentity);
  const file = path.join(directory, "auth.json");
  if (!await present(file)) return null;
  const before = await fs.lstat(file, { bigint: true });
  requireThat(before.isFile() && !before.isSymbolicLink() && before.nlink === 1n, "Credential copy is linked or not regular; retained");
  const handle = await fs.open(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  let credential;
  try {
    credential = await handle.stat({ bigint: true });
    sameIdentity(credential, identity(before));
    requireThat(credential.isFile() && credential.nlink === 1n, "Credential copy changed while opening; retained");
  } finally { await handle.close(); }
  // OAuth refresh can replace these bytes legitimately. Authorization comes
  // from this run's authenticated directory mapping, never credential content.
  sameIdentity(await plainDirectory(directory), expectedIdentity);
  const last = await fs.lstat(file, { bigint: true });
  sameIdentity(last, identity(credential));
  requireThat(last.nlink === 1n && last.size === credential.size && last.mtimeNs === credential.mtimeNs,
    "Credential copy changed before cleanup; retained");
  await fs.unlink(file); // Only this regular, verified test-created file; never a tree.
  return file;
}

export async function cleanupAgentCredentials(home, agentId) {
  const mapping = await inspect(home, agentId);
  // Authenticate the complete topology before removing any copy for this agent.
  const removed = [];
  for (const directory of mapping.directories) {
    const file = await removeCredential(directory.path, directory.identity);
    if (file) removed.push(file);
  }
  return { agent_id: agentId, credential_copies_removed: removed, ownership: mapping.evidence };
}

export async function cleanupFixtureCredential(home, fixtureHome) {
  home = spelling(home);
  await plainDirectory(home);
  const expected = path.join(home, "fixture-profile", ".codex");
  requireThat(spelling(fixtureHome) === expected, "Fixture credential path is not this run's exact fixture home");
  const stat = await plainDirectory(expected);
  return { fixture_credential_removed: await removeCredential(expected, identity(stat)) };
}
