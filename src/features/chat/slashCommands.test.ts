import { describe, expect, it } from "vitest";
import { matchingSlashCommands, slashCommandsForProvider } from "./slashCommands";

describe("slashCommandsForProvider", () => {
  it("serves a catalog for known providers and none otherwise", () => {
    expect(slashCommandsForProvider("codex").length).toBeGreaterThan(0);
    expect(slashCommandsForProvider("OpenCode").length).toBeGreaterThan(0);
    expect(slashCommandsForProvider("mock")).toEqual([]);
    expect(slashCommandsForProvider(undefined)).toEqual([]);
  });
});

describe("matchingSlashCommands", () => {
  it("completes by case-insensitive prefix while the token is being typed", () => {
    const matches = matchingSlashCommands("/MO", "codex");
    expect(matches.map((entry) => entry.command)).toEqual(["/model"]);
  });

  it("offers the whole catalog for a bare slash", () => {
    expect(matchingSlashCommands("/", "claude").length).toBeGreaterThan(3);
  });

  it("stops matching once the command word is committed with a space", () => {
    expect(matchingSlashCommands("/model opus ", "codex")).toEqual([]);
    expect(matchingSlashCommands("/model ", "claude")).toEqual([]);
  });

  it("matches nothing when the draft is not a slash command", () => {
    expect(matchingSlashCommands("fix the /tests", "codex")).toEqual([]);
  });

  it("returns nothing for providers without a curated catalog", () => {
    expect(matchingSlashCommands("/", "antigravity")).toEqual([]);
  });
});
