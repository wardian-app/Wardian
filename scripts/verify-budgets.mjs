/**
 * Compare debt metrics with the base revision instead of a committed budget.
 *
 * The base checkout is materialized as a temporary Git worktree, so the
 * repository has no shared mutable budgets.json that concurrent branches can
 * conflict over. A metric may improve or remain equal; any increase fails.
 *
 * Usage:
 *   node scripts/verify-budgets.mjs
 *   node scripts/verify-budgets.mjs --base origin/main
 */
import { execFileSync } from "node:child_process";
import {
  lstatSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  unlinkSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

const REPO_ROOT = process.cwd();
const ESLINT_BIN = path.join(REPO_ROOT, "node_modules", "eslint", "bin", "eslint.js");
// The package manifest and ESLint configuration define the dependency/tooling
// contract used for the comparison. A lockfile-only refresh must remain
// budget-checkable: the base worktree intentionally reuses this checkout's
// installed tooling, while the source debt metrics are independent of the
// transitive package graph.
const LINT_POLICY_FILES = ["eslint.config.js", "package.json"];
/**
 * The manifest keys that actually define the dependency and tooling contract.
 *
 * The guard exists so a branch cannot loosen lint policy to hide warnings it
 * introduced. Treating the whole manifest as policy overshoots that: adding an
 * npm script changes no dependency, engine, or rule, and cannot conceal a
 * warning, but it used to refuse the comparison anyway — which blocked every
 * PR that shipped a tool with the call site the pre-commit checklist requires.
 */
const PACKAGE_CONTRACT_FIELDS = [
  "dependencies",
  "devDependencies",
  "optionalDependencies",
  "peerDependencies",
  "overrides",
  "resolutions",
  "engines",
  "packageManager",
  "eslintConfig",
  "prettier",
];
const AUDITED_SECURITY_PIN_TRANSITIONS = [
  {
    path: ["overrides", "minimatch@10.2.5", "brace-expansion"],
    from: "5.0.9",
    to: "5.0.12",
  },
  { path: ["overrides", "undici"], from: "7.29.0", to: "7.29.1" },
  // GHSA-82fw-gwwq-j7x9: keep Vitest and its coverage adapter on patched 4.1.
  { path: ["devDependencies", "vitest"], from: "^4.1.8", to: "^4.1.11" },
  { path: ["devDependencies", "@vitest/coverage-v8"], from: "^4.1.8", to: "^4.1.11" },
  // GHSA-p98j-92pf-mc4p and GHSA-6688-9rhm-gjv2 fix in-place sanitization.
  { path: ["overrides", "dompurify"], from: "3.4.13", to: "3.4.16" },
];
const TRACKED_FILE_LINES = [
  "src-tauri/src/commands/agent.rs",
  "src-tauri/src/control.rs",
  "src-tauri/src/state/file_resources.rs",
  "src-tauri/src/manager/telemetry.rs",
  "src-tauri/src/commands/git.rs",
  "crates/wardian-cli/src/main.rs",
  "src/views/App.tsx",
];

function parseArgs(argv) {
  let base;
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--base") base = argv[++index];
    else if (argv[index]?.startsWith("--base=")) base = argv[index].slice("--base=".length);
    else throw new Error(`unknown argument: ${argv[index]}`);
  }
  return { base };
}

function git(args) {
  return execFileSync("git", args, { cwd: REPO_ROOT, encoding: "utf8" }).trim();
}

/** File contents at a revision, or undefined when it is not present there. */
function showFile(revision, file) {
  try {
    return execFileSync("git", ["show", `${revision}:${file}`], { cwd: REPO_ROOT, encoding: "utf8" });
  } catch {
    return undefined;
  }
}

/**
 * The contract fields of a manifest, as a stable string for comparison.
 *
 * Unparseable input returns the raw text, so a malformed manifest compares as
 * changed and the gate stays conservative rather than silently allowing it.
 */
export function packageContract(manifestText) {
  let parsed;
  try {
    parsed = JSON.parse(manifestText);
  } catch {
    return manifestText;
  }
  return JSON.stringify(PACKAGE_CONTRACT_FIELDS.map((field) => [field, parsed?.[field] ?? null]));
}

function manifestValueAtPath(manifest, keys) {
  return keys.reduce((value, key) => value?.[key], manifest);
}

function setManifestValueAtPath(manifest, keys, value) {
  const parent = keys.slice(0, -1).reduce((current, key) => current?.[key], manifest);
  if (parent && typeof parent === "object" && !Array.isArray(parent)) {
    parent[keys.at(-1)] = value;
  }
}

/**
 * Compare dependency contracts while admitting only the exact audited
 * base-to-head security pin updates above. All other contract fields remain
 * byte-for-byte equivalent after JSON serialization.
 */
function packageContractsMatch(baseManifestText, headManifestText) {
  let base;
  let head;
  try {
    base = JSON.parse(baseManifestText);
    head = JSON.parse(headManifestText);
  } catch {
    return false;
  }
  if (!base || typeof base !== "object" || Array.isArray(base)
    || !head || typeof head !== "object" || Array.isArray(head)) return false;

  const normalizedHead = JSON.parse(JSON.stringify(head));
  for (const transition of AUDITED_SECURITY_PIN_TRANSITIONS) {
    if (manifestValueAtPath(base, transition.path) === transition.from
      && manifestValueAtPath(head, transition.path) === transition.to) {
      setManifestValueAtPath(normalizedHead, transition.path, transition.from);
    }
  }
  return packageContract(JSON.stringify(base)) === packageContract(JSON.stringify(normalizedHead));
}

/**
 * @param {string[]} changedFiles Paths differing from the base revision.
 * @param {{base?: string, head?: string}} [manifests] `package.json` text on
 *   each side. Omit them to keep the conservative behaviour of treating any
 *   manifest change as a policy change.
 */
export function changedLintPolicyFiles(changedFiles, manifests) {
  const changed = new Set(changedFiles);
  return LINT_POLICY_FILES.filter((file) => {
    if (!changed.has(file)) return false;
    if (file !== "package.json") return true;
    if (!manifests || manifests.base === undefined || manifests.head === undefined) return true;
    return !packageContractsMatch(manifests.base, manifests.head);
  });
}

export function assertLintPolicyUnchanged(changedFiles, manifests) {
  const changed = changedLintPolicyFiles(changedFiles, manifests);
  if (changed.length > 0) {
    throw new Error(
      `Debt budget gate refuses to compare a revision that changes lint policy or dependencies: ${changed.join(", ")}`,
    );
  }
}

function resolveBaseRef(explicit) {
  const candidate = explicit
    ?? (process.env.GITHUB_BASE_REF ? `origin/${process.env.GITHUB_BASE_REF}` : "origin/main");
  try {
    return git(["rev-parse", "--verify", `${candidate}^{commit}`]);
  } catch {
    if (explicit || candidate !== "origin/main") throw new Error(`base revision is not available: ${candidate}`);
    return git(["rev-parse", "--verify", "HEAD^"]);
  }
}

function walk(dir, predicate, found = []) {
  let entries;
  try { entries = readdirSync(dir, { withFileTypes: true }); } catch { return found; }
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (["node_modules", "target", "dist", ".git"].includes(entry.name)) continue;
      walk(full, predicate, found);
    } else if (predicate(full)) found.push(full);
  }
  return found;
}

const posix = (value) => value.split(path.sep).join("/");

function rustFiles(root) {
  return [
    ...walk(path.join(root, "src-tauri", "src"), (file) => file.endsWith(".rs")),
    ...walk(path.join(root, "crates"), (file) => file.endsWith(".rs")),
  ];
}

function countMatches(files, pattern) {
  let total = 0;
  for (const file of files) {
    const matches = readFileSync(file, "utf8").match(pattern);
    total += matches ? matches.length : 0;
  }
  return total;
}

function fileLines(root) {
  return Object.fromEntries(TRACKED_FILE_LINES.map((file) => {
    try { return [file, readFileSync(path.join(root, file), "utf8").split("\n").length]; }
    catch { return [file, 0]; }
  }));
}

function measure(root) {
  const rust = rustFiles(root);
  const eslint = JSON.parse(execFileSync(process.execPath, [
    ESLINT_BIN, ".", "-f", "json", "--config", path.join(root, "eslint.config.js"),
  ], {
    cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024,
  }));
  return {
    file_lines: fileLines(root),
    clippy_allow_too_many_arguments: countMatches(rust, /#\[allow\(clippy::too_many_arguments\)\]/g),
    clippy_allow_await_holding_lock: countMatches(rust, /#\[allow\(clippy::await_holding_lock\)\]/g),
    cfg_test_functions: countMatches(rust, /#\[cfg\(test\)\]\s*\n\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s/g),
    ignored_rust_tests: countMatches(rust, /#\[ignore\b/g),
    skipped_e2e_tests: countMatches(
      walk(path.join(root, "e2e"), (file) => file.endsWith(".spec.ts")),
      /\btest\.skip\s*\(\s*(?:true\b|["'`])/g,
    ),
    eslint_warnings: eslint.reduce((sum, file) => sum + file.warningCount, 0),
  };
}

export function compareMetrics(current, base) {
  const over = [];
  const under = [];
  for (const [key, value] of Object.entries(current)) {
    if (key === "file_lines") {
      for (const [file, lines] of Object.entries(value)) {
        const baseline = base.file_lines[file] ?? 0;
        if (lines > baseline) over.push([`${posix(file)} lines`, lines, baseline]);
        else if (lines < baseline) under.push([`${posix(file)} lines`, lines, baseline]);
      }
    } else if (value > base[key]) over.push([key, value, base[key]]);
    else if (value < base[key]) under.push([key, value, base[key]]);
  }
  return { over, under };
}

function addBaseWorktree(base) {
  const tempRoot = mkdtempSync(path.join(os.tmpdir(), "wardian-budget-base-"));
  try {
    execFileSync("git", ["worktree", "add", "--detach", "--quiet", tempRoot, base], { cwd: REPO_ROOT, stdio: "pipe" });
  } catch (error) {
    rmSync(tempRoot, { recursive: true, force: true });
    throw error;
  }
  return tempRoot;
}

function removeBaseWorktree(tempRoot) {
  const dependencyLink = path.join(tempRoot, "node_modules");
  try {
    if (lstatSync(dependencyLink).isSymbolicLink()) unlinkSync(dependencyLink);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  try { execFileSync("git", ["worktree", "remove", "--force", tempRoot], { cwd: REPO_ROOT, stdio: "pipe" }); }
  finally { rmSync(tempRoot, { recursive: true, force: true }); }
}

function linkDependencies(root) {
  if (!readFileSync(path.join(REPO_ROOT, "node_modules", "eslint", "bin", "eslint.js"), "utf8")) {
    throw new Error("current checkout does not have the installed ESLint dependency");
  }
  symlinkSync(
    path.join(REPO_ROOT, "node_modules"),
    path.join(root, "node_modules"),
    process.platform === "win32" ? "junction" : "dir",
  );
}

/**
 * The base worktree runs against this checkout's installed `node_modules`, so
 * the two manifests have to agree about what is installed. They do not have to
 * be byte-identical: comparing the contract fields asks the question this check
 * actually cares about, and lets a script-only difference through.
 */
export function assertDependencyParity(baseManifestText, headManifestText) {
  if (!packageContractsMatch(baseManifestText, headManifestText)) {
    throw new Error("Debt budget gate cannot resolve base dependencies for package.json.");
  }
}

function assertDependencyParityAtRoot(root) {
  for (const file of LINT_POLICY_FILES.slice(1)) {
    assertDependencyParity(
      readFileSync(path.join(root, file), "utf8"),
      readFileSync(path.join(REPO_ROOT, file), "utf8"),
    );
  }
}

export function main(argv = process.argv.slice(2)) {
  const { base: explicitBase } = parseArgs(argv);
  const base = resolveBaseRef(explicitBase);
  assertLintPolicyUnchanged(
    git(["diff", "--name-only", `${base}..HEAD`, "--", ...LINT_POLICY_FILES]).split("\n").filter(Boolean),
    { base: showFile(base, "package.json"), head: showFile("HEAD", "package.json") },
  );
  const current = measure(REPO_ROOT);
  const baseRoot = addBaseWorktree(base);
  let baseline;
  try {
    assertDependencyParityAtRoot(baseRoot);
    linkDependencies(baseRoot);
    baseline = measure(baseRoot);
  }
  finally { removeBaseWorktree(baseRoot); }

  const { over, under } = compareMetrics(current, baseline);
  if (over.length > 0) {
    console.error(`Debt budgets: ${over.length} metric(s) increased against ${base}.\n`);
    for (const [name, value, previous] of over) console.error(`  ${name}: ${value} (base ${previous}, +${value - previous})`);
    process.exitCode = 1;
    return 1;
  }
  console.log(`Debt budgets: all ${Object.keys(current).length} metric group(s) did not increase against ${base}.`);
  for (const [name, value, previous] of under) console.log(`  improved ${name}: ${previous} -> ${value}`);
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try { process.exitCode = main(); }
  catch (error) { console.error(error instanceof Error ? error.message : error); process.exitCode = 1; }
}
