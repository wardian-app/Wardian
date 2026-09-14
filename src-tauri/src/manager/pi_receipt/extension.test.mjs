import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, writeFileSync, readFileSync, unlinkSync, rmdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { registerReceipt, MAX_STREAM_BYTES } from "./extension.mjs";

const digest = (text) => createHash("sha256").update(text).digest("hex");
function fixture(options = {}) {
  const handlers = new Map();
  const lines = [];
  const ctx = { mode: "tui", sessionManager: { getSessionId: () => "native-one" } };
  const launch = { nonce: "owned-launch", native_session_id: "native-one" };
  registerReceipt({ on: (name, fn) => handlers.set(name, fn) }, launch,
    { fd: 42, append: (_fd, line) => lines.push(JSON.parse(line)), size: () => 0, ...options });
  const emit = (name, event = {}, context = ctx) => handlers.get(name)?.(event, context);
  const user = (text) => emit("message_start", { message: { role: "user", content: [{ type: "text", text }] } });
  return { lines, ctx, emit, user };
}
test("ready binds native identity; input and loop start alone never acknowledge", () => {
  const f = fixture();
  f.emit("input", { text: "prompt", source: "interactive" });
  f.emit("before_agent_start", { prompt: "prompt" });
  f.emit("session_start"); f.emit("agent_start");
  assert.deepEqual(f.lines.map((x) => x.kind), ["ready", "loop_start"]);
});
test("actual user start precedes all assistant and transcript activity", () => {
  const f = fixture(); f.emit("session_start"); f.emit("agent_start"); f.user("alpha\nβ 😀");
  const start = f.lines.at(-1);
  assert.equal(start.kind, "user_start");
  assert.equal(start.text_sha256, digest("alpha\nβ 😀"));
  assert.equal(start.text_bytes, Buffer.byteLength("alpha\nβ 😀"));
  assert.equal(start.native_session_id, "native-one");
});
for (const [label, content] of [
  ["empty array", []],
  ["empty text", [{ type: "text", text: "" }]],
  ["multiple empty text parts", [{ type: "text", text: "" }, { type: "text", text: "" }]],
]) {
  test(`${label} cannot emit a user receipt or consume its sequence`, () => {
    const f = fixture(); f.emit("session_start"); f.emit("agent_start");
    f.emit("message_start", { message: { role: "user", content } });
    assert.deepEqual(f.lines.map((x) => x.kind), ["ready", "loop_start"]);
    f.user("next");
    assert.equal(f.lines.at(-1).kind, "user_start");
    assert.equal(f.lines.at(-1).seq, 3);
    assert.equal(f.lines.at(-1).text_sha256, digest("next"));
  });
}

for (const [label, parts, text] of [
  ["whitespace-only content", [" \t\n"], " \t\n"],
  ["empty parts around nonempty content", ["", "alpha\nβ 😀", ""], "alpha\nβ 😀"],
]) {
  test(`${label} retains its exact nonempty receipt bytes`, () => {
    const f = fixture(); f.emit("session_start"); f.emit("agent_start");
    f.emit("message_start", { message: { role: "user", content: parts.map((text) => ({ type: "text", text })) } });
    const start = f.lines.at(-1);
    assert.equal(start.kind, "user_start");
    assert.equal(start.text_sha256, digest(text));
    assert.equal(start.text_bytes, Buffer.byteLength(text, "utf8"));
  });
}

test("user events outside a loop and non-user messages do not count", () => {
  const f = fixture(); f.emit("session_start"); f.user("before"); f.emit("agent_start");
  for (const role of ["assistant", "custom", "toolResult"]) f.emit("message_start", { message: { role, content: [] } });
  f.emit("agent_end"); f.user("after");
  assert.equal(f.lines.filter((x) => x.kind === "user_start").length, 0);
});
test("queued inputs count when consumed without a second agent_start", () => {
  const f = fixture(); f.emit("session_start"); f.emit("agent_start"); f.user("first");
  f.emit("input", { text: "queued", streamingBehavior: "followUp" });
  assert.equal(f.lines.filter((x) => x.kind === "user_start").length, 1);
  f.user("queued");
  assert.equal(f.lines.filter((x) => x.kind === "user_start").length, 2);
});
test("identical consecutive inputs retain distinct event sequence identities", () => {
  const f = fixture(); f.emit("session_start");
  for (let i = 0; i < 2; i++) { f.emit("agent_start"); f.user("same"); f.emit("agent_end"); }
  const events = f.lines.filter((x) => x.kind === "user_start");
  assert.equal(events[0].text_sha256, events[1].text_sha256);
  assert.notEqual(events[0].seq, events[1].seq);
});
test("transformed content reports native bytes, never the original input", () => {
  const f = fixture(); f.emit("session_start"); f.emit("input", { text: "original" });
  f.emit("agent_start"); f.user("transformed");
  assert.equal(f.lines.at(-1).text_sha256, digest("transformed"));
  assert.notEqual(f.lines.at(-1).text_sha256, digest("original"));
});
test("handled/slash commands without an actual user event cannot acknowledge", () => {
  const f = fixture(); f.emit("session_start");
  f.emit("input", { text: "/handled" }); f.emit("before_agent_start", { prompt: "/handled" });
  assert.deepEqual(f.lines.map((x) => x.kind), ["ready"]);
});
test("multimodal user activity has no plain-text receipt digest", () => {
  const f = fixture(); f.emit("session_start"); f.emit("agent_start");
  f.emit("message_start", { message: { role: "user", content: [{ type: "image" }] } });
  assert.equal(f.lines.at(-1).text_sha256, null);
  assert.equal(f.lines.at(-1).text_bytes, null);
});
test("reload invalidates instead of resetting a readable stream", () => {
  const f = fixture(); f.emit("session_start"); f.emit("session_start", { reason: "reload" });
  f.emit("agent_start"); f.user("later");
  assert.deepEqual(f.lines.map((x) => x.kind), ["ready", "invalidated"]);
});
test("identity changes latch failure even if identity later matches", () => {
  const f = fixture(); f.emit("session_start", {}, { mode: "tui", sessionManager: { getSessionId: () => "foreign" } });
  f.emit("session_start"); f.emit("agent_start"); f.user("later");
  assert.equal(f.lines.length, 0);
});
test("append failures and full streams cannot emit later success", () => {
  for (const options of [{ append: () => { throw new Error("disk failure"); } }, { size: () => MAX_STREAM_BYTES }]) {
    const f = fixture(options); f.emit("session_start"); f.emit("agent_start"); f.user("prompt");
    assert.equal(f.lines.length, 0);
  }
});
test("real owned stream path with spaces and Unicode, no model/auth", () => {
  const directory = mkdtempSync(join(tmpdir(), "wardian pi receipt-"));
  const path = join(directory, "events β.jsonl");
  const handlers = new Map();
  try {
    writeFileSync(path, "", { flag: "wx", mode: 0o600 });
    registerReceipt({ on: (name, handler) => handlers.set(name, handler) },
      { event_path: path, nonce: "launch", native_session_id: "native" });
    handlers.get("session_start")({}, { mode: "tui", sessionManager: { getSessionId: () => "native" } });
    assert.equal(JSON.parse(readFileSync(path, "utf8")).kind, "ready");
  } finally {
    handlers.get("session_shutdown")?.();
    unlinkSync(path); rmdirSync(directory);
  }
});

test("non-TUI mode cannot advertise interactive receipt capability", () => {
  const f = fixture(); f.emit("session_start", {}, { mode: "rpc", sessionManager: { getSessionId: () => "native-one" } });
  f.emit("agent_start"); f.user("prompt"); assert.equal(f.lines.length, 0);
});
