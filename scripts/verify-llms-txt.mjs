import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const repoRoot = process.cwd();
const llmsPath = join(repoRoot, "docs", "public", "llms.txt");

const requiredEntries = [
  ["## Getting Started", "https://docs.wardian.org/guide/getting-started", "docs/guide/getting-started.md"],
  ["## Core Workspace", "https://docs.wardian.org/guide/grid", "docs/guide/grid.md"],
  ["## Core Workspace", "https://docs.wardian.org/guide/queue", "docs/guide/queue.md"],
  ["## Core Workspace", "https://docs.wardian.org/guide/watchlists", "docs/guide/watchlists.md"],
  ["## Workflow Automation", "https://docs.wardian.org/workflows/", "docs/workflows/index.md"],
  ["## CLI and Agent Communication", "https://docs.wardian.org/guide/cli", "docs/guide/cli.md"],
  ["## Providers and Runtime", "https://docs.wardian.org/guide/provider-readiness", "docs/guide/provider-readiness.md"],
  ["## Developer Internals", "https://docs.wardian.org/developer/architecture", "docs/developer/architecture.md"],
];

const requiredMaintenanceText = "llms.txt";
const maintenancePath = join(repoRoot, "docs", "developer", "docs-maintenance.md");

function fail(message) {
  console.error(message);
  process.exitCode = 1;
}

if (!existsSync(llmsPath)) {
  fail("docs/public/llms.txt is missing.");
} else {
  const text = readFileSync(llmsPath, "utf8");

  if (!text.startsWith("# Wardian Docs")) {
    fail("llms.txt must start with '# Wardian Docs'.");
  }

  if (text.length > 12000) {
    fail("llms.txt should stay concise for agent ingestion.");
  }

  if (/(^|[\s("'`])[A-Za-z]:[\\/]|\\\\Users\\\\|\/Users\//m.test(text)) {
    fail("llms.txt must not contain local machine paths.");
  }

  for (const [section, url, sourcePath] of requiredEntries) {
    if (!text.includes(section)) {
      fail(`llms.txt is missing required section ${section}.`);
    }

    if (!text.includes(url)) {
      fail(`llms.txt is missing required link ${url}.`);
    }

    if (!existsSync(join(repoRoot, sourcePath))) {
      fail(`llms.txt source target does not exist: ${sourcePath}.`);
    }
  }
}

const maintenanceText = readFileSync(maintenancePath, "utf8");
if (!maintenanceText.includes(requiredMaintenanceText)) {
  fail("docs maintenance guidance must mention llms.txt upkeep.");
}
