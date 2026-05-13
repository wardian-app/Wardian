import { describe, it, expect } from "vitest";
import {
    defaultProviderConfig,
    normalizeAgentConfig,
    providerConfigFor,
    requiresRestart,
    toPersistedAgentConfig,
    withProvider,
} from "./configUtils";
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

    it("does not require restart when regular session persistence changes", () => {
        const config1: AgentConfig = { ...baseConfig, session_persistence: "default" };
        const config2: AgentConfig = { ...baseConfig, session_persistence: "fresh" };
        expect(requiresRestart(config1, config2)).toBe(false);
    });

    it("returns true when nested provider_config changes", () => {
        const config1: AgentConfig = {
            ...baseConfig,
            provider: "codex",
            provider_config: { type: "codex", sandbox_mode: "read-only" },
        };
        const config2: AgentConfig = {
            ...baseConfig,
            provider: "codex",
            provider_config: { type: "codex", sandbox_mode: "workspace-write" },
        };
        expect(requiresRestart(config1, config2)).toBe(true);
    });
});

describe("provider config utilities", () => {
    const baseConfig: AgentConfig = {
        session_id: "agent-1",
        session_name: "Agent",
        agent_class: "Coder",
        folder: "C:/project",
        is_off: false,
    };

    it("creates default provider configs for known providers", () => {
        expect(defaultProviderConfig("claude")).toEqual({ type: "claude" });
        expect(defaultProviderConfig("gemini")).toEqual({ type: "gemini" });
        expect(defaultProviderConfig("codex")).toEqual({ type: "codex" });
        expect(defaultProviderConfig("opencode")).toEqual({ type: "opencode" });
        expect(defaultProviderConfig("mock")).toEqual({ type: "mock" });
    });

    it("normalizes legacy flat Codex fields into provider_config", () => {
        const normalized = normalizeAgentConfig({
            ...baseConfig,
            provider: "codex",
            codex_sandbox_mode: "workspace-write",
            codex_approval_policy: "never",
            codex_cleared_provider_sessions: ["old-thread"],
        });

        expect(normalized.provider_config).toEqual({
            type: "codex",
            sandbox_mode: "workspace-write",
            approval_policy: "never",
            cleared_provider_sessions: ["old-thread"],
        });
    });

    it("uses nested provider_config over stale legacy flat fields", () => {
        const normalized = normalizeAgentConfig({
            ...baseConfig,
            provider: "codex",
            codex_sandbox_mode: "read-only",
            provider_config: { type: "codex", sandbox_mode: "workspace-write" },
        });

        expect(providerConfigFor(normalized)).toEqual({
            type: "codex",
            sandbox_mode: "workspace-write",
        });
    });

    it("resets mismatched provider_config to selected provider default", () => {
        const normalized = normalizeAgentConfig({
            ...baseConfig,
            provider: "gemini",
            provider_config: { type: "codex", sandbox_mode: "workspace-write" },
        });

        expect(normalized.provider_config).toEqual({ type: "gemini" });
    });

    it("preserves unknown provider config from backend state", () => {
        const normalized = normalizeAgentConfig({
            ...baseConfig,
            provider: "future-provider",
            provider_config: { type: "future-provider", launch_mode: "fast" },
        });

        expect(normalized.provider).toBe("future-provider");
        expect(normalized.provider_config).toEqual({
            type: "future-provider",
            launch_mode: "fast",
        });
    });

    it("clears custom args when changing provider", () => {
        expect(withProvider({ custom_args: "--codex-only" }, "gemini")).toEqual({
            provider: "gemini",
            provider_config: { type: "gemini" },
            custom_args: undefined,
        });
    });

    it("emits the nested persisted provider_config contract", () => {
        const persisted = toPersistedAgentConfig({
            ...baseConfig,
            provider: "opencode",
            opencode_agent: "build",
        });

        expect(persisted.provider_config).toEqual({ type: "opencode", agent: "build" });
    });
});
