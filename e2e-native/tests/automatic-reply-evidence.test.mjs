// @tier nightly — Read-only observer, owned gate, and deadline checks; no app or providers.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import {
  automaticReplyDeadlines, completedAssistantRows, exactCurrentCascade, readCanonicalTaskReply,
  writeOwnedReplyReport,
} from "../lib/automatic-reply-evidence.mjs";
import { acquireHomeLock, readHomeLock, releaseHomeLock } from "../lib/sessionHome.mjs";
import { waitForReplyRelease } from "./fixtures/automatic-reply-gate.mjs";

async function fixture(t) {
  const parent = path.resolve(os.tmpdir());
  const home = await fs.mkdtemp(path.join(parent, "wardian-automatic-reply-"));
  t.after(async () => {
    const relative = path.relative(parent, path.resolve(home));
    assert.ok(relative && !relative.startsWith("..") && !path.isAbsolute(relative));
    await fs.rm(home, { recursive: true, force: true });
  });
  return home;
}

function database(home, configure = () => {}) {
  const db = new DatabaseSync(path.join(home, "state.db"));
  try {
    db.exec(`
      CREATE TABLE interactions (
        id TEXT PRIMARY KEY, kind TEXT, sender_session_id TEXT, target_session_ids TEXT,
        parent_interaction_id TEXT, body_ref TEXT, status TEXT
      );
      CREATE TABLE structured_replies (request_id TEXT, status TEXT);
    `);
    configure(db);
  } finally { db.close(); }
}

test("report persistence preserves unowned homes and uses the actual acquired run claim", async (t) => {
  const home = await fixture(t);
  const reportPath = path.join(home, "automatic-inbound-reply-report.json");
  await fs.writeFile(reportPath, "operator evidence\n", { flag: "wx" });
  assert.equal(await writeOwnedReplyReport({ isolatedHome: home }, {}), false);
  const runId = "report-owner-" + path.basename(home);
  const lock = acquireHomeLock({ home, runId }).lock;
  try {
    assert.equal(await writeOwnedReplyReport({ isolatedHome: home, runId: "other-run" }, {}), false);
    assert.equal(await writeOwnedReplyReport({ isolatedHome: home, runId: "other-run", homeLock: lock }, {}), false);
    assert.equal(await fs.readFile(reportPath, "utf8"), "operator evidence\n");
    assert.deepEqual(readHomeLock(home), lock);
    const report = { status: "fail", cleanup: { shutdown_confirmed: false } };
    assert.equal(await writeOwnedReplyReport({ isolatedHome: home, runId, homeLock: lock }, report), true);
    assert.deepEqual(JSON.parse(await fs.readFile(reportPath, "utf8")), report);
  } finally { releaseHomeLock({ home, runId }); }
});

function row(db, { id = "task-1", kind = "task", sender = "requester", targets = ["worker"],
  parent = null, body = "gate instruction", status = "pending" } = {}) {
  db.prepare("INSERT INTO interactions VALUES (?,?,?,?,?,?,?)").run(
    id, kind, sender, JSON.stringify(targets), parent,
    JSON.stringify({ storage: "inline", body }), status,
  );
}

function reply(db, overrides = {}) {
  row(db, { id: "reply-1", kind: "reply", sender: "worker", targets: ["requester"],
    parent: "task-1", body: "WORKER_ONLY_unpredictable", status: "completed", ...overrides });
  db.prepare("INSERT INTO structured_replies VALUES (?,?)").run("task-1", "done");
}

test("observer waits for missing database or unadmitted task without creating state", async (t) => {
  const home = await fixture(t);
  assert.equal(await readCanonicalTaskReply(home, "requester", "worker"), null);
  assert.deepEqual(await fs.readdir(home), []);
  database(home);
  assert.equal(await readCanonicalTaskReply(home, "requester", "worker"), null);
});

test("observer binds pending and completed replies to one exact request without writing", async (t) => {
  const home = await fixture(t);
  database(home, (db) => row(db));
  const pending = await readCanonicalTaskReply(home, "requester", "worker", "task-1");
  assert.equal(pending.reply_count, 0);
  assert.equal(pending.task_status, "pending");
  const db = new DatabaseSync(path.join(home, "state.db"));
  try {
    db.exec("UPDATE interactions SET status='completed' WHERE id='task-1'");
    reply(db);
  } finally { db.close(); }
  const before = await fs.readFile(path.join(home, "state.db"));
  const proof = await readCanonicalTaskReply(home, "requester", "worker", "task-1");
  assert.equal(proof.reply_count, 1);
  assert.equal(proof.reply.status, "done");
  assert.equal(proof.reply.sender, "worker");
  assert.equal(proof.reply.parent_interaction_id, "task-1");
  assert.equal(proof.requester_reply_count, 0);
  assert.equal(proof.reply_to_reply_count, 0);
  assert.deepEqual(await fs.readFile(path.join(home, "state.db")), before);
  assert.deepEqual(await fs.readdir(home), ["state.db"]);
  await assert.rejects(readCanonicalTaskReply(home, "requester", "worker", "other-task"), /owned request changed/u);
});

test("observer rejects wrong recipients, extra tasks, duplicate replies, and spoofed reply origin", async (t) => {
  const cases = [
    (db) => row(db, { targets: ["foreign-worker"] }),
    (db) => { row(db); row(db, { id: "task-2" }); },
    (db) => { row(db); reply(db); reply(db, { id: "reply-2" }); },
    (db) => { row(db); reply(db, { sender: "foreign-worker" }); },
    (db) => { row(db); reply(db, { targets: ["foreign-requester"] }); },
    (db) => { row(db); reply(db); db.exec("DELETE FROM structured_replies"); },
  ];
  for (const configure of cases) {
    const home = await fixture(t);
    database(home, configure);
    await assert.rejects(readCanonicalTaskReply(home, "requester", "worker"), assert.AssertionError);
  }
});

test("observer exposes requester obligations and reply-to-reply loops to the acceptance oracle", async (t) => {
  const home = await fixture(t);
  database(home, (db) => {
    row(db); reply(db);
    row(db, { id: "loop", kind: "reply", sender: "requester", targets: ["worker"], parent: "reply-1" });
  });
  const proof = await readCanonicalTaskReply(home, "requester", "worker");
  assert.equal(proof.requester_reply_count, 1);
  assert.equal(proof.reply_to_reply_count, 1);
});

test("deadlines preserve the original absolute limit and require a positive cleanup reserve", () => {
  assert.deepEqual(automaticReplyDeadlines("2030-01-01T00:00:10Z", 2_000, Date.parse("2030-01-01T00:00:00Z")), {
    deadlineAt: Date.parse("2030-01-01T00:00:10Z"),
    workDeadlineAt: Date.parse("2030-01-01T00:00:08Z"), cleanupReserveMs: 2_000,
  });
  for (const [date, reserve, now] of [
    [undefined, 1, 0], ["2030-01-01T00:00:10", 1, 0], ["invalidZ", 1, 0],
    ["2030-01-01T00:00:10Z", 0, 0], ["2030-01-01T00:00:10Z", 1.5, 0],
    ["2030-01-01T00:00:10Z", 2_000, Date.parse("2030-01-01T00:00:08Z")],
  ]) assert.throws(() => automaticReplyDeadlines(date, reserve, now), assert.AssertionError);
});

test("native evidence requires exact direct workspace ownership and both completion signals", () => {
  const cascade = { id: "owned", ownership_basis: "workspace_uri", records: [] };
  assert.equal(exactCurrentCascade({ provider: "antigravity", sessions: [cascade] }, "owned"), cascade);
  for (const snapshot of [
    { provider: "codex", sessions: [cascade] },
    { provider: "antigravity", sessions: [cascade, cascade] },
    { provider: "antigravity", sessions: [{ ...cascade, id: "foreign" }] },
    { provider: "antigravity", sessions: [{ ...cascade, ownership_basis: "workspace_alias" }] },
  ]) assert.throws(() => exactCurrentCascade(snapshot, "owned"), assert.AssertionError);
  const complete = { role: "assistant", complete: true, native_status: 3 };
  assert.deepEqual(completedAssistantRows([
    complete, { ...complete, complete: false }, { ...complete, native_status: 2 }, { ...complete, role: "user" },
  ]), [complete]);
});

test("worker gate discloses one unpredictable token only after operator release", async (t) => {
  const home = await fixture(t);
  const readyPath = path.join(home, "ready");
  const releasePath = path.join(home, "release");
  let settled = false;
  const pending = waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 3_000, pollMs: 5 });
  pending.then(() => { settled = true; }, () => { settled = true; });
  const readyDeadline = Date.now() + 2_000;
  while (Date.now() < readyDeadline) {
    try { await fs.access(readyPath); break; }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.equal(await fs.readFile(readyPath, "utf8"), "ready\n");
  assert.equal(settled, false);
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 1_000 }), /EEXIST/u);
  await fs.writeFile(releasePath, "release\n", { flag: "wx" });
  assert.match(await pending, /^WORKER_ONLY_[0-9a-f-]{36}$/u);
  assert.equal(await fs.readFile(readyPath + ".consumed", "utf8"), "consumed\n");
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 1_000 }), /released before/u);
});

test("worker gate fails at the original deadline without release or token", async (t) => {
  const home = await fixture(t);
  const readyPath = path.join(home, "ready");
  const releasePath = path.join(home, "release");
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() + 30, pollMs: 5 }), /deadline expired/u);
  await assert.rejects(fs.access(readyPath + ".consumed"), /ENOENT/u);
  await assert.rejects(waitForReplyRelease({ readyPath, releasePath, deadlineAt: Date.now() - 1 }), /deadline expired/u);
});
