#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const defaultRepoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const scanRoots = Object.freeze(["src", "e2e", "e2e-native", "scripts"]);
const surfaceNames = "Grid|Dashboard|Queue|Library|Workflows|Graph|Garden";

const frozenAudit = Object.freeze([
  ["e2e/tests/agent-lifecycle.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/critical-flows.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/features.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/garden.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/graph-topology.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/library-redesign.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/queue-v2.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/run-params.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/run-view.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/schedule-monitor.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/workflow-builder.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/workflow.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e/tests/workflows.spec.ts", "migrated", "Task 15 semantic workbench helper migration."],
  ["e2e-native/tests/real-provider-rendering-native.test.mjs", "migrated", "Task 17 native semantic helper migration."],
  ["e2e-native/tests/terminal-geometry-sweep-native.test.mjs", "migrated", "Task 17 native semantic helper migration."],
  ["e2e-native/tests/terminal-rendering-native.test.mjs", "migrated", "Task 17 native semantic helper migration."],
  ["e2e-native/tests/terminal-visibility-snapshot-native.test.mjs", "migrated", "Task 17 native semantic helper migration."],
  ["e2e-native/tests/terminal-wheel-scroll-native.test.mjs", "migrated", "Task 17 native semantic helper migration."],
  ["scripts/measure-view-performance.mjs", "removed", "Removed by Task 18 after the production workbench benchmark replaced it."],
  ["scripts/capture-doc-screenshots.mjs", "migrated", "Desktop capture must use semantic workbench navigation."],
  ["scripts/capture-readme-demo-real.mjs", "migrated", "Desktop capture must use semantic workbench navigation."],
  ["src/styles/App.css", "migrated", "Task 19 removes the flag-off titlebar navigation styles."],
  ["e2e/tests/remote-pwa.spec.ts", "intentionally-unrelated", "Remote PWA Queue navigation is not desktop surface navigation."],
  ["src/features/remote/RemoteMobileApp.test.tsx", "intentionally-unrelated", "Remote mobile navigation is not desktop surface navigation."],
  ["src/features/settings/SettingsModal.test.tsx", "intentionally-unrelated", "Settings Queue and Grid controls configure alerts and density, not desktop surfaces."],
].map(([auditPath, disposition, reason]) => Object.freeze({
  path: auditPath,
  disposition,
  reason,
})));

const rules = Object.freeze([
  {
    id: "legacy-titlebar-selector",
    description: "legacy .titlebar-center/.titlebar-tab selector",
    pattern: /\.titlebar-(?:center|tab)\b/g,
  },
  {
    id: "legacy-role-button-selector",
    description: "fixed desktop surface button role selector",
    pattern: new RegExp(
      `getByRole\\(\\s*["']button["']\\s*,\\s*\\{\\s*name:\\s*["'](?:${surfaceNames})["']`,
      "g",
    ),
  },
  {
    id: "legacy-xpath-button-selector",
    description: "fixed desktop surface XPath button selector",
    pattern: new RegExp(
      `normalize-space\\(\\.\\)\\s*=\\s*["'](?:${surfaceNames})["']`,
      "g",
    ),
  },
  {
    id: "legacy-desktop-navigation-symbol",
    description: "legacy desktop navigation state/component symbol",
    paths: [
      /^src\/views\/App\.tsx$/,
      /^src\/layout\/titlebar\/(?:CustomTitleBar|WorkspaceTabs)(?:\.test)?\.tsx$/,
    ],
    pattern: /\b(?:WorkspaceTabs|(?:LEGACY_)?CACHED_CANVAS_VIEWS|ViewMode|setViewMode|viewMode)\b/g,
  },
  {
    id: "direct-desktop-surface-launch-click",
    description: "direct click on a fixed desktop surface label",
    pattern: new RegExp(
      [
        `(?:await\\s+)?[\\w$.()]+\\.getByRole\\(\\s*["']button["']\\s*,\\s*\\{\\s*name:\\s*["'](?:${surfaceNames})["'][^;]{0,120}?\\)\\s*\\.click\\s*\\(`,
        `(?:await\\s+)?[\\w$.()]+\\.getByText\\(\\s*["'](?:${surfaceNames})["'][^;]{0,80}?\\)\\s*\\.click\\s*\\(`,
        `(?:fireEvent|userEvent)\\.click\\([^;]{0,180}?getBy(?:Role|Text)\\([^;]{0,120}?["'](?:${surfaceNames})["'][^;]{0,80}?\\)\\s*\\)`,
      ].join("|"),
      "g",
    ),
  },
]);

const allowlistedMatches = Object.freeze([
  {
    path: "e2e/tests/remote-pwa.spec.ts",
    rule_ids: ["legacy-role-button-selector", "direct-desktop-surface-launch-click"],
    context: /Queue/,
    reason: "Remote PWA Queue navigation is an intentionally unrelated mobile surface.",
  },
  {
    path: "src/features/remote/RemoteMobileApp.test.tsx",
    rule_ids: ["legacy-role-button-selector", "direct-desktop-surface-launch-click"],
    context: /(?:Grid|Queue|Graph|Garden)/,
    reason: "Remote mobile buttons do not launch desktop workbench surfaces.",
  },
  {
    path: "src/features/settings/SettingsModal.test.tsx",
    rule_ids: ["legacy-role-button-selector", "direct-desktop-surface-launch-click"],
    context: /(?:Grid|Queue)/,
    reason: "Settings Queue and Grid buttons configure alerts or density.",
  },
  {
    path: "e2e/tests/agent-lifecycle.spec.ts",
    rule_ids: ["legacy-role-button-selector", "direct-desktop-surface-launch-click"],
    context: /overview[\s\S]*Grid/i,
    reason: "Grid is an Agents Overview presentation mode in this migrated suite.",
  },
  {
    path: "e2e/tests/features.spec.ts",
    rule_ids: ["legacy-role-button-selector", "direct-desktop-surface-launch-click"],
    context: /overview[\s\S]*Grid/i,
    reason: "Grid is an Agents Overview presentation mode in this migrated suite.",
  },
  {
    path: "e2e/tests/responsive-layout.spec.ts",
    rule_ids: ["legacy-role-button-selector", "direct-desktop-surface-launch-click"],
    context: /overview[\s\S]*Grid/i,
    reason: "Grid is an Agents Overview presentation mode, not global navigation.",
  },
  {
    path: "e2e/tests/workbench-overview.spec.ts",
    rule_ids: ["legacy-role-button-selector", "direct-desktop-surface-launch-click"],
    context: /Grid/,
    reason: "This suite explicitly tests the Agents Overview Grid mode.",
  },
  {
    path: "src/features/workbench/surfaces/AgentsOverviewSurface.test.tsx",
    rule_ids: ["legacy-role-button-selector", "direct-desktop-surface-launch-click"],
    context: /Grid/,
    reason: "This unit test changes the Agents Overview presentation mode.",
  },
  {
    path: "src/views/App.test.tsx",
    rule_ids: ["legacy-role-button-selector"],
    context: /overviewSurface[\s\S]*Grid/,
    reason: "This match selects the Agents Overview Grid mode; other App legacy matches remain forbidden.",
  },
  {
    path: "e2e/tests/workflows.spec.ts",
    rule_ids: ["legacy-role-button-selector"],
    context: /titlebar[\s\S]*Workflows[\s\S]*toHaveCount\(0\)/,
    reason: "This negative assertion proves the removed legacy titlebar Workflows button stays absent.",
  },
]);

const ignoredInfrastructurePaths = Object.freeze([
  {
    path: "scripts/verify-workbench-cutover.mjs",
    reason: "The verifier necessarily declares the forbidden patterns and frozen audit.",
  },
  {
    path: "src/config/workbenchCutoverCheck.test.ts",
    reason: "The focused verifier test necessarily asserts forbidden-rule diagnostics.",
  },
]);

function normalizePath(value) {
  return value.replaceAll("\\", "/");
}

function gitTrackedFiles(repoRoot) {
  const output = execFileSync(
    "git",
    ["-C", repoRoot, "ls-files", "-z", "--", ...scanRoots],
    { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
  );
  return output.split("\0").filter(Boolean).map(normalizePath).sort();
}

function ruleMatches(rule, filePath, source) {
  if (rule.paths && !rule.paths.some((pattern) => pattern.test(filePath))) return [];
  const pattern = new RegExp(rule.pattern.source, rule.pattern.flags);
  const matches = [];
  for (const match of source.matchAll(pattern)) {
    const index = match.index ?? 0;
    const line = source.slice(0, index).split(/\r?\n/).length;
    const lineStart = source.lastIndexOf("\n", Math.max(0, index - 1)) + 1;
    const lineEnd = source.indexOf("\n", index);
    const lineText = source.slice(lineStart, lineEnd < 0 ? source.length : lineEnd).trim();
    matches.push({
      path: filePath,
      line,
      rule_id: rule.id,
      description: rule.description,
      excerpt: match[0].replace(/\s+/g, " ").trim().slice(0, 240),
      context: `${lineText}\n${match[0]}`,
    });
  }
  return matches;
}

function validateDefinitions() {
  const errors = [];
  const dispositions = new Set(["migrated", "removed", "intentionally-unrelated"]);
  const ruleIds = new Set(rules.map((rule) => rule.id));
  if (frozenAudit.length !== 25) errors.push(`frozen audit must contain exactly 25 entries, got ${frozenAudit.length}`);
  if (new Set(frozenAudit.map((entry) => entry.path)).size !== frozenAudit.length) {
    errors.push("frozen audit paths must be unique");
  }
  for (const entry of frozenAudit) {
    if (!dispositions.has(entry.disposition)) errors.push(`invalid disposition for ${entry.path}`);
    if (!entry.reason.trim()) errors.push(`missing audit reason for ${entry.path}`);
  }
  for (const entry of allowlistedMatches) {
    if (!entry.reason.trim()) errors.push(`missing allowlist reason for ${entry.path}`);
    if (!(entry.context instanceof RegExp)) errors.push(`missing allowlist context for ${entry.path}`);
    for (const ruleId of entry.rule_ids) {
      if (!ruleIds.has(ruleId)) errors.push(`unknown allowlist rule ${ruleId} for ${entry.path}`);
    }
  }
  for (const entry of ignoredInfrastructurePaths) {
    if (!entry.reason.trim()) errors.push(`missing infrastructure exclusion reason for ${entry.path}`);
  }
  for (const entry of frozenAudit.filter((candidate) => candidate.disposition === "intentionally-unrelated")) {
    if (!allowlistedMatches.some((candidate) => candidate.path === entry.path)) {
      errors.push(`intentionally unrelated audit path lacks an allowlist: ${entry.path}`);
    }
  }
  return errors;
}

function matchingAllowlist(match) {
  return allowlistedMatches.find((entry) => entry.path === match.path
    && entry.rule_ids.includes(match.rule_id)
    && entry.context.test(match.context));
}

function scanTrackedTree(repoRoot) {
  const definitionErrors = validateDefinitions();
  const trackedFiles = gitTrackedFiles(repoRoot);
  const tracked = new Set(trackedFiles);
  const ignored = new Set(ignoredInfrastructurePaths.map((entry) => entry.path));
  const violations = definitionErrors.map((message) => ({
    path: "<verifier>", line: 0, rule_id: "invalid-verifier-definition", description: message,
  }));
  const allowed = [];
  const usedAllowlist = new Set();

  for (const filePath of trackedFiles) {
    if (ignored.has(filePath)) continue;
    const absolute = path.join(repoRoot, ...filePath.split("/"));
    if (!fs.existsSync(absolute)) continue;
    const source = fs.readFileSync(absolute);
    if (source.includes(0)) continue;
    const text = source.toString("utf8");
    for (const rule of rules) {
      for (const match of ruleMatches(rule, filePath, text)) {
        const allowlist = matchingAllowlist(match);
        if (allowlist) {
          usedAllowlist.add(allowlist);
          allowed.push({ ...match, reason: allowlist.reason });
        } else {
          violations.push(match);
        }
      }
    }
  }

  for (const entry of allowlistedMatches) {
    if (!usedAllowlist.has(entry)) {
      violations.push({
        path: entry.path,
        line: 0,
        rule_id: "stale-allowlist",
        description: `allowlist no longer matches tracked source: ${entry.reason}`,
      });
    }
  }

  const auditEntries = frozenAudit.map((entry) => {
    const absolute = path.join(repoRoot, ...entry.path.split("/"));
    const exists = fs.existsSync(absolute);
    const isTracked = tracked.has(entry.path);
    const pathViolations = violations.filter((violation) => violation.path === entry.path);
    let verified = false;
    if (entry.disposition === "removed") {
      verified = !exists && !isTracked;
    } else {
      verified = exists && isTracked && pathViolations.length === 0;
    }
    if (!verified && pathViolations.length === 0) {
      violations.push({
        path: entry.path,
        line: 0,
        rule_id: "audit-disposition-mismatch",
        description: entry.disposition === "removed"
          ? "frozen audit says removed but the path still exists or is tracked"
          : `frozen audit says ${entry.disposition} but the tracked path is missing`,
      });
    }
    return { ...entry, exists, tracked: isTracked, verified };
  });

  const counts = Object.fromEntries(
    ["migrated", "removed", "intentionally-unrelated"].map((disposition) => [
      disposition,
      auditEntries.filter((entry) => entry.disposition === disposition).length,
    ]),
  );
  const result = {
    schema_version: 1,
    passed: violations.length === 0 && auditEntries.every((entry) => entry.verified),
    roots: scanRoots,
    tracked_files_scanned: trackedFiles.length - trackedFiles.filter((entry) => ignored.has(entry)).length,
    audit: {
      baseline_revision: "d53842dc",
      expected_entries: 25,
      verified_entries: auditEntries.filter((entry) => entry.verified).length,
      counts,
      entries: auditEntries,
    },
    allowlisted_matches: allowed,
    violations,
  };
  return result;
}

function selfTest() {
  const samples = [
    ["src/styles/App.css", ".titlebar-tab { color: red; }", "legacy-titlebar-selector"],
    ["e2e/tests/example.spec.ts", 'page.getByRole("button", { name: "Dashboard" })', "legacy-role-button-selector"],
    ["e2e-native/tests/example.test.mjs", "//button[normalize-space(.)='Queue']", "legacy-xpath-button-selector"],
    ["src/views/App.tsx", "const [viewMode, setViewMode] = useState<ViewMode>();", "legacy-desktop-navigation-symbol"],
    ["e2e/tests/example.spec.ts", 'await page.getByRole("button", { name: "Library" }).click();', "direct-desktop-surface-launch-click"],
  ];
  const errors = validateDefinitions();
  for (const [filePath, source, expectedRule] of samples) {
    const detected = rules.flatMap((rule) => ruleMatches(rule, filePath, source));
    if (!detected.some((match) => match.rule_id === expectedRule)) {
      errors.push(`self-test failed to detect ${expectedRule}`);
    }
  }
  const clean = rules.flatMap((rule) => ruleMatches(
    rule,
    "e2e/tests/workbench.spec.ts",
    'await openWorkbenchSurface(page, "library");',
  ));
  if (clean.length > 0) errors.push("semantic workbench helper produced a false positive");
  return {
    schema_version: 1,
    self_test: errors.length === 0 ? "passed" : "failed",
    audit_entries: frozenAudit.length,
    rules: rules.map((rule) => rule.id),
    errors,
  };
}

function parseArguments(argv) {
  const parsed = { root: defaultRepoRoot, json: false, describeAudit: false, selfTest: false };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--json") parsed.json = true;
    else if (argument === "--describe-audit") parsed.describeAudit = true;
    else if (argument === "--self-test") parsed.selfTest = true;
    else if (argument === "--root") {
      const value = argv[index + 1];
      if (!value) throw new Error("--root requires an absolute path");
      if (!path.isAbsolute(value)) throw new Error("--root requires an absolute path");
      parsed.root = path.resolve(value);
      index += 1;
    } else {
      throw new Error(`unknown argument: ${argument}`);
    }
  }
  return parsed;
}

function auditDescription() {
  const counts = Object.fromEntries(
    ["migrated", "removed", "intentionally-unrelated"].map((disposition) => [
      disposition,
      frozenAudit.filter((entry) => entry.disposition === disposition).length,
    ]),
  );
  return {
    schema_version: 1,
    baseline_revision: "d53842dc",
    expected_entries: 25,
    counts,
    entries: frozenAudit,
    allowlist: allowlistedMatches.map(({ context: _context, ...entry }) => entry),
  };
}

function printHuman(result) {
  if (result.passed) {
    console.log(`Workbench cutover verified: ${result.audit.verified_entries}/25 audit entries.`);
    return;
  }
  console.error("Workbench cutover verification failed.");
  console.error(`Verified frozen audit entries: ${result.audit.verified_entries}/25`);
  for (const violation of result.violations) {
    const location = violation.line > 0 ? `${violation.path}:${violation.line}` : violation.path;
    console.error(`- ${location} [${violation.rule_id}] ${violation.description}`);
    if (violation.excerpt) console.error(`  ${violation.excerpt}`);
  }
}

try {
  const options = parseArguments(process.argv.slice(2));
  if (options.selfTest) {
    const result = selfTest();
    console.log(JSON.stringify(result, null, 2));
    if (result.self_test !== "passed") process.exitCode = 1;
  } else if (options.describeAudit) {
    console.log(JSON.stringify(auditDescription(), null, 2));
  } else {
    const result = scanTrackedTree(options.root);
    if (options.json) console.log(JSON.stringify(result, null, 2));
    else printHuman(result);
    if (!result.passed) process.exitCode = 1;
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
