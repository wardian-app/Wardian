import { describe, expect, it } from "vitest";
import { inboundMessage } from "./inboundMessages";

// Producer framing from `wardian_core::agent_messaging` and `control::agent_messaging`.
const LINE = "The wardian plugin sent a message:";
const CARRIED = "Wardian carried these previously admitted informational messages into this turn. They do not create tasks or reply obligations:\n";
const INFO = "Wardian delivered an informational message into this existing conversation. It creates no task and no obligation to reply. Do not produce a response solely to acknowledge it.\n\nCanonical informational message context:\n";
const REPLY = "Wardian delivered the terminal result for a task you requested. Continue this existing conversation using the result as appropriate. This is a reply continuation, not a new task, and it creates no reply obligation.\n\nCanonical reply context:\n";
const SUFFIX = "\n\nThis is how Claude Code surfaces a prompt a plugin submits between turns — it starts this turn in the user's place.";

const ctx = (kind: string, body: string, extra: Record<string, unknown> = {}) => ({ schema_version: 1, sender: `${kind}-sender`,
  recipient: "me", kind, interaction_id: `int_${kind}`, parent_interaction_id: null, request_id: kind === "task" ? "ask_1" : null,
  body, reply_status: kind === "reply" ? "done" : null, ...extra });
const carried = (...bodies: string[]) => `${CARRIED}${JSON.stringify(bodies.map((body, index) => ({ ...ctx("message", body), interaction_id: `int_old_${index}` })))}\n\n`;
const info = (body: string, prefix = "") => `${LINE}\n${prefix}${INFO}${JSON.stringify(ctx("message", body))}${SUFFIX}`;
const reply = (body: string, prefix = "") => `${LINE}\n${prefix}${REPLY}${JSON.stringify(ctx("reply", body))}${SUFFIX}`;
const task = (body: string, prefix = "") => `${LINE}\n${prefix}${JSON.stringify(ctx("task", body))}\n\nWardian request id: ask_1\nWhen finished, use the canonical reply tool with request_id ask_1, status done, and message containing your reply.`;

describe("inboundMessage", () => {
  it("reads an informational delivery, including braces and quotes in its body", () => {
    expect(inboundMessage(info("Ship {it} \"now\""))).toEqual({ kind: "wardian", complete: true, carried: [],
      primary: { sender: "message-sender", relation: "message", body: "Ship {it} \"now\"", status: null } });
  });

  it("reads the reply itself, not the carried messages framed before it", () => {
    const parsed = inboundMessage(reply("Approved with zero findings", carried("old one", "old two")));
    expect(parsed).toMatchObject({ kind: "wardian", complete: true,
      primary: { relation: "reply", body: "Approved with zero findings", status: "done" } });
    expect(parsed?.kind === "wardian" && parsed.complete ? parsed.carried.map((item) => item.body) : null).toEqual(["old one", "old two"]);
  });

  it("reads a task framed with its structured-reply instruction, with or without carried messages", () => {
    expect(inboundMessage(task("Review the PR"))).toMatchObject({ complete: true, primary: { relation: "task", body: "Review the PR" } });
    const omitted = `${CARRIED}${JSON.stringify([ctx("message", "old")])}\n3 older informational messages omitted by the count or byte budget; see message history in Wardian for the full canonical records.\n\n`;
    expect(inboundMessage(task("Review the PR", omitted))).toMatchObject({ complete: true, carried: [{ body: "old" }] });
  });

  it("reports a clipped preview as incomplete only when its full text is available", () => {
    const preview = reply("x".repeat(3000), carried("old")).slice(0, 1024);
    // The preview names the reply itself, never the carried message before it.
    expect(inboundMessage(preview, true)).toMatchObject({ kind: "wardian", complete: false,
      preview: { sender: "reply-sender", relation: "reply", body: expect.stringMatching(/^x+$/) } });
    expect(inboundMessage(preview, false)).toBeNull();
    // Cut inside the carried block, the primary is unknown.
    expect(inboundMessage(reply("done", carried("y".repeat(2000))).slice(0, 600), true))
      .toEqual({ kind: "wardian", complete: false, preview: null });
    // Cut only inside Claude Code's trailing explanation, nothing was lost.
    const whole = info("short");
    expect(inboundMessage(whole.slice(0, whole.length - 40), true)).toMatchObject({ complete: true, primary: { body: "short" } });
  });

  it("leaves operator text that resembles a delivery as an ordinary message", () => {
    const sample = JSON.stringify(ctx("message", "sample"));
    for (const text of [
      `${LINE} what does this mean?\n${INFO}${sample}`,
      `${LINE}\n${sample}\nwhat does this mean?`,
      `${LINE}\n${INFO}${sample}\n\nAnd why did it arrive twice?`,
      `${LINE}\n${INFO}${JSON.stringify(ctx("reply", "kind does not match the framing"))}`,
      `${LINE}\n${INFO}${JSON.stringify({ sender: "a", kind: "message", body: "not canonical" })}`,
      `${LINE}\n${JSON.stringify(ctx("task", "no instruction"))}`,
      `I pasted this:\n${info("quoted")}`,
    ]) expect(inboundMessage(text), text.slice(0, 60)).toBeNull();
  });

  it("recognizes harness notices only by their exact framing", () => {
    const notice = "<task-notification>\n<status>completed</status>\n<summary>Background command \"Run tests\" completed (exit code 0)</summary>\n</task-notification>";
    expect(inboundMessage(notice)).toMatchObject({ kind: "notice", label: "Background command \"Run tests\" completed (exit code 0)" });
    expect(inboundMessage(notice.slice(0, 80))).toBeNull();
    expect(inboundMessage(notice.slice(0, 80), true)).toMatchObject({ kind: "notice" });
    expect(inboundMessage("This session is being continued from a previous conversation that ran out of context. The summary below covers it."))
      .toMatchObject({ kind: "notice", label: "Conversation continued from an earlier summary" });
    expect(inboundMessage("This session is being continued from a previous conversation, right?")).toBeNull();
  });
});
