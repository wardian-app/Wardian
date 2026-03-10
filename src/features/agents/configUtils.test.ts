import { describe, it, expect } from "vitest";
import { requiresRestart } from "./configUtils";
import { AgentConfig } from "../../types";

describe("requiresRestart", () => {
    const baseConfig: AgentConfig = {
        session_id: "agent-1",
        session_name: "Original Name",
        agent_class: "Coder",
        folder: "C:/project",
        is_off: false,
    };

    it("returns false if only session_name changes", () => {
        const newConfig = { ...baseConfig, session_name: "New Name" };
        expect(requiresRestart(baseConfig, newConfig)).toBe(false);
    });

    it("returns false if nothing changes", () => {
        expect(requiresRestart(baseConfig, baseConfig)).toBe(false);
    });

    it("returns true if agent_class changes", () => {
        const newConfig = { ...baseConfig, agent_class: "Architect" };
        expect(requiresRestart(baseConfig, newConfig)).toBe(true);
    });

    it("returns true if folder changes", () => {
        const newConfig = { ...baseConfig, folder: "C:/other" };
        expect(requiresRestart(baseConfig, newConfig)).toBe(true);
    });

    it("returns true if a boolean flag like debug changes", () => {
        const newConfig = { ...baseConfig, debug: true };
        expect(requiresRestart(baseConfig, newConfig)).toBe(true);
    });

    it("returns true if an array field like policy changes", () => {
        const oldConfig = { ...baseConfig, policy: ["read"] };
        const newConfig = { ...baseConfig, policy: ["read", "write"] };
        expect(requiresRestart(oldConfig, newConfig)).toBe(true);
    });

    it("returns false if an array field stays the same (deep comparison)", () => {
        const oldConfig = { ...baseConfig, policy: ["read"] };
        const newConfig = { ...baseConfig, policy: ["read"] };
        expect(requiresRestart(oldConfig, newConfig)).toBe(false);
    });

    it("returns true when system_include_directories changes", () => {
        const config1: AgentConfig = { ...baseConfig, system_include_directories: ["/old"] };
        const config2: AgentConfig = { ...baseConfig, system_include_directories: ["/new"] };
        expect(requiresRestart(config1, config2)).toBe(true);
    });
});
