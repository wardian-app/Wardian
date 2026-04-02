/**
 * E2E fixture seeder — populates an isolated WARDIAN_HOME with test data.
 *
 * Usage: npx tsx e2e/fixtures/seed.ts <target-dir>
 * Also importable: import { seedTestHome } from './seed';
 */
import * as fs from "fs";
import * as path from "path";

export interface SeedOptions {
  /** Include a pre-configured mock agent */
  withMockAgent?: boolean;
  /** Include a test class definition */
  withTestClass?: boolean;
  /** Include a test workflow */
  withTestWorkflow?: boolean;
}

const DEFAULT_OPTIONS: SeedOptions = {
  withMockAgent: true,
  withTestClass: true,
  withTestWorkflow: false,
};

const MOCK_AGENT_ID = "e2e-mock-agent-001";
const MOCK_SESSION_ID = "mock-session-e2e-001";

export function seedTestHome(
  targetDir: string,
  options: SeedOptions = DEFAULT_OPTIONS
): void {
  // Create base directory structure
  const dirs = [
    "",
    "agents",
    "classes",
    "common",
    "workflows",
    "library",
    "debug",
  ];

  for (const dir of dirs) {
    fs.mkdirSync(path.join(targetDir, dir), { recursive: true });
  }

  // Common AGENTS.md
  fs.writeFileSync(
    path.join(targetDir, "common", "AGENTS.md"),
    "# E2E Test Agent Instructions\n\nYou are a test agent.\n"
  );

  if (options.withTestClass) {
    const classDir = path.join(targetDir, "classes", "TestClass");
    fs.mkdirSync(classDir, { recursive: true });
    fs.writeFileSync(
      path.join(classDir, "class.json"),
      JSON.stringify(
        {
          name: "TestClass",
          description: "E2E test class",
          provider: "mock",
          model: null,
          debug: false,
          sandbox: false,
          yolo: false,
        },
        null,
        2
      )
    );
    fs.writeFileSync(
      path.join(classDir, "AGENTS.md"),
      "# TestClass Instructions\n\nYou are a mock test agent.\n"
    );
  }

  if (options.withMockAgent) {
    // Agent session directory
    const agentDir = path.join(targetDir, "agents", MOCK_SESSION_ID);
    fs.mkdirSync(agentDir, { recursive: true });

    // wardian_state.json — app state with one mock agent
    const state = {
      agents: [
        {
          id: MOCK_AGENT_ID,
          session_name: "E2E Mock Agent",
          session_id: MOCK_SESSION_ID,
          agent_class: "TestClass",
          provider: "mock",
          folder: "",
          model: null,
          debug: false,
          sandbox: false,
          yolo: false,
          approval_mode: null,
          policy: null,
          include_directories: null,
          system_include_directories: null,
          resume_session: MOCK_SESSION_ID,
          custom_args: null,
          screen_reader: false,
          output_format: null,
          experimental_acp: false,
          allowed_mcp_server_names: null,
          extensions: null,
        },
      ],
      watchlists: [
        {
          id: "all",
          name: "All Agents",
          agent_ids: [MOCK_AGENT_ID],
        },
      ],
    };
    fs.writeFileSync(
      path.join(targetDir, "wardian_state.json"),
      JSON.stringify(state, null, 2)
    );
  }

  if (options.withTestWorkflow) {
    const workflowsDir = path.join(targetDir, "workflows");
    fs.mkdirSync(workflowsDir, { recursive: true });

    const workflow = {
      id: "e2e-test-workflow",
      name: "E2E Test Workflow",
      nodes: [
        {
          id: "trigger-1",
          type: "trigger",
          name: "Manual Trigger",
          config: { type: "Manual" },
          position: { x: 100, y: 100 },
          dependencies: null,
        },
        {
          id: "agent-1",
          type: "agent",
          name: "Mock Agent Step",
          config: {
            agent_id: MOCK_AGENT_ID,
            prompt: "Run a quick test",
          },
          position: { x: 300, y: 100 },
          dependencies: ["trigger-1"],
        },
      ],
      edges: [{ source: "trigger-1", target: "agent-1" }],
    };
    fs.writeFileSync(
      path.join(workflowsDir, "e2e-test-workflow.json"),
      JSON.stringify(workflow, null, 2)
    );

    // workflows.json index
    fs.writeFileSync(
      path.join(targetDir, "workflows.json"),
      JSON.stringify(
        [
          {
            id: "e2e-test-workflow",
            name: "E2E Test Workflow",
            path: "workflows/e2e-test-workflow.json",
          },
        ],
        null,
        2
      )
    );
  }
}

export function cleanTestHome(targetDir: string): void {
  if (fs.existsSync(targetDir)) {
    fs.rmSync(targetDir, { recursive: true, force: true });
  }
}

// CLI entry point
if (require.main === module) {
  const target = process.argv[2];
  if (!target) {
    console.error("Usage: npx tsx e2e/fixtures/seed.ts <target-dir>");
    process.exit(1);
  }
  seedTestHome(target);
  console.log(`Seeded test home at: ${target}`);
}
