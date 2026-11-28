import { describe, expect, it } from "vitest";
import { inboundMessage } from "./inboundMessages";

const delivery = (context: Record<string, unknown>) => [
  "The wardian plugin sent a message:",
  "Wardian delivered an informational message into this existing conversation. It creates no task and no obligation to reply.",
  "",
  "Canonical informational message context:",
  JSON.stringify(context),
  "",
  "This is how Claude Code surfaces a prompt a plugin submits between turns.",
].join("\n");

describe("inboundMessage", () => {
  it("reads sender, relation, status and body from a Wardian delivery", () => {
    expect(inboundMessage(delivery({ sender: "agent-2", kind: "reply", body: "Approve {with braces} and \"quotes\"", reply_status: "done" })))
      .toEqual({ kind: "wardian", sender: "agent-2", relation: "reply", body: "Approve {with braces} and \"quotes\"", status: "done" });
  });

  it("reads a delivery whose JSON context follows the first line directly", () => {
    expect(inboundMessage(`The wardian plugin sent a message:\n${JSON.stringify({ sender: "a", kind: "message", body: "Hi" })}`))
      .toMatchObject({ kind: "wardian", relation: "message", body: "Hi", status: null });
  });

  it("leaves a delivery without a readable body as an ordinary message", () => {
    expect(inboundMessage("The wardian plugin sent a message:\nnot json")).toBeNull();
    expect(inboundMessage(delivery({ sender: "a", kind: "message" }))).toBeNull();
  });

  it("turns harness notices into one-line labels", () => {
    expect(inboundMessage("<task-notification>\n<status>completed</status>\n<summary>Background command \"Run tests\" completed (exit code 0)</summary>\n</task-notification>"))
      .toMatchObject({ kind: "notice", label: "Background command \"Run tests\" completed (exit code 0)" });
    expect(inboundMessage("This session is being continued from a previous conversation that ran out of context."))
      .toMatchObject({ kind: "notice", label: "Conversation continued from an earlier summary" });
  });

  it("does not reclassify what the operator typed", () => {
    expect(inboundMessage("Why does the settings page load slowly?")).toBeNull();
    expect(inboundMessage("I pasted this: The wardian plugin sent a message:")).toBeNull();
  });
});
