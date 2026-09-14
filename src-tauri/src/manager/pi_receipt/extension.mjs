/* global WARDIAN_PI_LAUNCH */
// Stock Pi extension: observe actual user messages, never input/echo/agent_start alone.
import { appendFileSync, closeSync, constants, fstatSync, lstatSync, openSync } from "node:fs";
import { createHash, randomUUID } from "node:crypto";

export const MAX_STREAM_BYTES = 262144;
export const MAX_RECORD_BYTES = 2048;

// The launcher supplies this immutable descriptor in the generated module.
export function registerReceipt(pi, launch, io = {}) {
  const append = io.append ?? appendFileSync;
  const size = io.size ?? ((fd) => fstatSync(fd).size);
  const fd = io.fd ?? openSync(launch.event_path,
    constants.O_WRONLY | constants.O_APPEND | (constants.O_NOFOLLOW ?? 0));
  if (io.fd === undefined) {
    const opened = fstatSync(fd);
    const named = lstatSync(launch.event_path);
    if (!opened.isFile() || named.isSymbolicLink() || opened.dev !== named.dev || opened.ino !== named.ino) {
      closeSync(fd);
      throw new Error("Pi receipt file identity changed");
    }
  }
  const stream = randomUUID();
  let seq = 0;
  let failed = false;
  let ready = false;
  let active = false;
  const emit = (kind, ctx, extra = {}) => {
    if (failed) return;
    try {
      const native = ctx.sessionManager.getSessionId();
      if (native !== launch.native_session_id) throw new Error("Pi receipt session changed");
      const line = JSON.stringify({ v: 1, launch: launch.nonce, stream, seq: ++seq,
        native_session_id: native, kind, ...extra }) + "\n";
      if (Buffer.byteLength(line, "utf8") > MAX_RECORD_BYTES ||
          size(fd) + Buffer.byteLength(line, "utf8") > MAX_STREAM_BYTES) {
        throw new Error("Pi receipt stream capacity exceeded");
      }
      append(fd, line, "utf8");
    } catch {
      // Pi catches handler exceptions. Latch locally too: no later false ready/start.
      failed = true;
    }
  };
  pi.on("session_start", (_event, ctx) => {
    if (ctx.mode !== "tui") { failed = true; return; }
    if (ready) { emit("invalidated", ctx); failed = true; return; }
    emit("ready", ctx);
    ready = !failed;
  });
  pi.on("agent_start", (_event, ctx) => {
    if (!ready) return;
    active = true;
    emit("loop_start", ctx);
  });
  pi.on("agent_end", (_event, ctx) => {
    if (!ready) return;
    active = false;
    emit("loop_end", ctx);
  });
  pi.on("message_start", (event, ctx) => {
    if (!ready || !active || event.message?.role !== "user") return;
    const parts = event.message.content;
    const text = Array.isArray(parts) && parts.every((part) => part.type === "text" && typeof part.text === "string")
      ? parts.map((part) => part.text).join("") : null;
    if (text === "") return;
    emit("user_start", ctx, { text_sha256: text === null ? null : createHash("sha256").update(text, "utf8").digest("hex"),
      text_bytes: text === null ? null : Buffer.byteLength(text, "utf8") });
  });
  pi.on("session_shutdown", () => {
    failed = true;
    if (io.fd === undefined) closeSync(fd);
  });
}

export default function receiptExtension(pi) {
  registerReceipt(pi, WARDIAN_PI_LAUNCH);
}
