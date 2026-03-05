import { describe, it, expect } from "vitest";
import type { AgentConfig, AgentTelemetry, AgentClassDefinition, AgentOutputPayload, AgentJsonEvent } from "./types";

describe("TypeScript Interface Shape Tests", () => {
  describe("AgentConfig", () => {
    it("accepts valid agent config", () => {
      const config: AgentConfig = {
        session_id: "abc-123",
        session_name: "Coder_Alpha",
        agent_class: "Coder",
        folder: "C:/projects/my-app",
        is_off: false,
      };
      expect(config.session_id).toBe("abc-123");
      expect(config.agent_class).toBe("Coder");
    });

    it("accepts config with resume_session", () => {
      const config: AgentConfig = {
        session_id: "abc-123",
        session_name: "Resumed Agent",
        agent_class: "Architect",
        folder: "",
        resume_session: "def-456",
        is_off: false,
      };
      expect(config.resume_session).toBe("def-456");
    });

    it("accepts config with optional resume_session omitted", () => {
      const config: AgentConfig = {
        session_id: "abc-123",
        session_name: "Fresh Agent",
        agent_class: "QA",
        folder: "/tmp",
        is_off: false,
      };
      expect(config.resume_session).toBeUndefined();
    });
  });

  describe("AgentTelemetry", () => {
    it("accepts valid telemetry data", () => {
      const telemetry: AgentTelemetry = {
        session_id: "abc-123",
        cpu_usage: 15.5,
        memory_mb: 256.3,
        uptime_seconds: 3600,
        query_count: 42,
        init_timestamp: "2026-02-27T12:00:00Z",
        current_status: "Idle",
      };
      expect(telemetry.cpu_usage).toBe(15.5);
      expect(telemetry.query_count).toBe(42);
    });

    it("accepts telemetry with null init_timestamp", () => {
      const telemetry: AgentTelemetry = {
        session_id: "abc-123",
        cpu_usage: 0,
        memory_mb: 0,
        uptime_seconds: 0,
        query_count: 0,
        init_timestamp: null,
        current_status: "Pending...",
      };
      expect(telemetry.init_timestamp).toBeNull();
    });
  });

  describe("AgentClassDefinition", () => {
    it("accepts a default class definition", () => {
      const cls: AgentClassDefinition = {
        name: "Coder",
        description: "Writes clean code",
        is_default: true,
      };
      expect(cls.is_default).toBe(true);
    });

    it("accepts a custom class definition", () => {
      const cls: AgentClassDefinition = {
        name: "DevOps",
        description: "Manages CI/CD pipelines",
        is_default: false,
      };
      expect(cls.is_default).toBe(false);
      expect(cls.name).toBe("DevOps");
    });
  });

  describe("AgentOutputPayload", () => {
    it("accepts stdout output", () => {
      const payload: AgentOutputPayload = {
        session_id: "abc-123",
        text: "Hello world\n",
        stream: "stdout",
      };
      expect(payload.stream).toBe("stdout");
    });

    it("accepts stderr output", () => {
      const payload: AgentOutputPayload = {
        session_id: "abc-123",
        text: "Error occurred",
        stream: "stderr",
      };
      expect(payload.stream).toBe("stderr");
    });
  });

  describe("AgentJsonEvent", () => {
    it("accepts arbitrary JSON event data", () => {
      const event: AgentJsonEvent = {
        session_id: "abc-123",
        data: { type: "progress", content: "Analyzing code..." },
      };
      expect(event.data.type).toBe("progress");
    });
  });
});
