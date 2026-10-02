/**
 * Fails when Rust production code gains an item nothing in production uses.
 *
 * `check:deadcode` (knip) covers TypeScript. rustc's own dead_code lint never
 * fires for a `pub` item in a library crate, because a library's public items
 * are its API, so an uncalled `pub fn` in `src-tauri` or `wardian-core` passed
 * every gate (#1085). Three checks close that gap; tests never count as
 * callers in any of them:
 *
 * 1. App crate (rustc). The app library has no external consumers except its
 *    own binary, so a temporary copy of the workspace rewrites every `pub`
 *    item in it to `pub(crate)`, keeps the entry points its binary calls
 *    public, and runs `cargo check` (non-test). Whatever rustc then reports as
 *    dead_code is unused in production. A reported item whose name is used in
 *    code compiled out on this platform (for example `#[cfg(unix)]` on
 *    Windows) is set aside as a cfg-gated caller rather than reported.
 * 2. Shared library crates (token search). `wardian-core` is consumed by the
 *    app and the CLI, so rustc cannot see across it. An item there is dead
 *    when production code cannot reach it by name (see unreachableItems).
 * 3. Tauri commands. Every command registered in `generate_handler!` must be
 *    invoked by name from production code. `debug_*` commands exist for the
 *    native E2E suite and may be invoked only from e2e, e2e-native, or scripts.
 *
 * Existing findings live in scripts/rust-deadcode-baseline.json, grouped by
 * the reason each is kept. A new finding fails; so does a baseline entry that
 * no longer matches anything (`--prune` removes those).
 *
 * The workspace copy lives under the cargo target directory and is synced in
 * place, so repeated runs reuse cargo's incremental state. The checkout itself
 * is never written to.
 *
 * Usage:
 *   node scripts/verify-rust-deadcode.mjs [--prune] [--verbose]
 */
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

import {
  IDENT,
  PUNCT,
  STR,
  collectReferences,
  definedItems,
  inactiveRegionAt,
  lexRust,
  loadCrate,
  productionCfgEnv,
} from "./lib/rust-source.mjs";

const REPO_ROOT = process.cwd();
const BASELINE_PATH = path.join(REPO_ROOT, "scripts", "rust-deadcode-baseline.json");
/** The package whose library is checked by rustc after the visibility rewrite. */
const APP_PACKAGE = "Wardian";
const COMMAND_PREFIX = "tauri-command::";
const FRONTEND_DIRS = ["src"];
const TOOLING_DIRS = ["e2e", "e2e-native", "scripts"];
const SKIPPED_DIRS = new Set(["node_modules", "target", "dist", "test-results", "screenshots", ".vitepress"]);
const TEST_FILE = /\.(test|spec)\.[cm]?[jt]sx?$|(^|\/)(__tests__|__mocks__|test)\//;
const PUB_ITEM = /\bpub (?=(?:async|unsafe|const|extern|fn|struct|enum|static|trait|type|mod|use|union) )/g;

const toPosix = (file) => file.split(path.sep).join("/");
const relative = (file) => toPosix(path.relative(REPO_ROOT, file));

function parseArgs(argv) {
  const options = { prune: false, verbose: false };
  for (const argument of argv) {
    if (argument === "--prune") options.prune = true;
    else if (argument === "--verbose") options.verbose = true;
    else throw new Error(`unknown argument: ${argument}`);
  }
  return options;
}

// ---------------------------------------------------------------------------
// Baseline

/**
 * Validate the baseline and return its entries.
 * @returns {Map<string, string>} entry -> reason
 */
export function readBaseline(text) {
  const baseline = JSON.parse(text);
  if (!Array.isArray(baseline.groups)) throw new Error("baseline must have a groups array");
  const entries = new Map();
  for (const group of baseline.groups) {
    if (typeof group.reason !== "string" || group.reason.trim().length < 10) {
      throw new Error(`baseline group needs a reason: ${JSON.stringify(group).slice(0, 120)}`);
    }
    for (const entry of group.entries ?? []) {
      if (entries.has(entry)) throw new Error(`duplicate baseline entry: ${entry}`);
      entries.set(entry, group.reason);
    }
  }
  return entries;
}

/**
 * Split findings into those the baseline does not cover and baseline entries
 * that matched nothing. `isCheckable(entry)` lets platform-specific entries
 * that this run could not have observed stay out of the stale list.
 */
export function compareWithBaseline(findings, baseline, isCheckable = () => true) {
  const found = new Set(findings.map((finding) => finding.key));
  const added = findings.filter((finding) => !baseline.has(finding.key));
  const stale = [...baseline.keys()].filter((entry) => !found.has(entry) && isCheckable(entry));
  return { added, stale };
}

function pruneBaseline(text, stale) {
  const baseline = JSON.parse(text);
  const remove = new Set(stale);
  for (const group of baseline.groups) group.entries = group.entries.filter((entry) => !remove.has(entry));
  baseline.groups = baseline.groups.filter((group) => group.entries.length > 0);
  return `${JSON.stringify(baseline, null, 2)}\n`;
}

// ---------------------------------------------------------------------------
// Workspace model

function cargoMetadata() {
  const output = execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: REPO_ROOT,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return JSON.parse(output);
}

const PRODUCTION_KINDS = new Set(["lib", "rlib", "staticlib", "cdylib", "dylib", "proc-macro", "bin"]);

/** Load the production source of every workspace package, keyed by file. */
function loadWorkspace(metadata, env) {
  const files = new Map();
  const packages = [];
  const unresolved = [];
  for (const pkg of metadata.packages) {
    const targets = pkg.targets.filter((target) => target.kind.some((kind) => PRODUCTION_KINDS.has(kind)));
    const lib = targets.find((target) => !target.kind.includes("bin"));
    const packageFiles = new Set();
    for (const target of targets) {
      const crate = loadCrate(target.src_path, env);
      unresolved.push(...crate.unresolved);
      for (const [file, analysis] of crate.files) {
        files.set(file, analysis);
        packageFiles.add(file);
      }
    }
    packages.push({
      name: pkg.name,
      dir: path.dirname(pkg.manifest_path),
      lib,
      bins: targets.filter((target) => target.kind.includes("bin")),
      files: packageFiles,
    });
  }
  return { files, packages, unresolved };
}

// ---------------------------------------------------------------------------
// Check 1: app crate through rustc

/**
 * Paths outside the workspace members that the build reads. tauri-build
 * validates the `../scripts/*` bundle-resource glob in tauri.conf.json.
 */
const BUILD_SUPPORT_PATHS = ["scripts"];

function listCopiedFiles(memberDirs) {
  const roots = ["Cargo.toml", "Cargo.lock", ".cargo", ...BUILD_SUPPORT_PATHS, ...memberDirs];
  const output = execFileSync(
    "git",
    ["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", ...roots],
    { cwd: REPO_ROOT, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  return [...new Set(output.split("\0").filter(Boolean))].filter((file) => existsSync(path.join(REPO_ROOT, file)));
}

/**
 * Point manifest `path = "..."` entries that leave the copied tree (for
 * example `[patch]` paths into vendor/) back at the checkout, so those
 * packages keep their identity and their cached build.
 */
export function rewriteManifestPaths(text, manifestDir, isCopied) {
  return text.replace(/(\bpath\s*=\s*")([^"]+)(")/g, (whole, open, value, close) => {
    const resolved = path.resolve(manifestDir, value);
    if (isCopied(resolved)) return whole;
    return `${open}${toPosix(resolved)}${close}`;
  });
}

/** Make every `pub` item crate-visible, except the entry points in `keepPublic`. */
export function rewriteVisibility(text, keepPublic = []) {
  let result = text.replace(PUB_ITEM, "pub(crate) ");
  for (const name of keepPublic) {
    result = result.replace(new RegExp(`\\bpub\\(crate\\) ((?:async )?fn ${name}\\b)`), "pub $1");
  }
  return result;
}

/** Library items the package's own binaries call through the crate name. */
function externalRoots(pkg, libName) {
  const roots = new Set();
  const pattern = new RegExp(`\\b${libName}::(\\w+)`, "g");
  for (const bin of pkg.bins) {
    const text = readFileSync(bin.src_path, "utf8");
    for (const hit of text.matchAll(pattern)) roots.add(hit[1]);
  }
  return [...roots];
}

/**
 * Resolve a repository-relative path inside the copy, or undefined when it
 * would land anywhere else (absolute, `..` traversal, or through a link that
 * leaves the copy). Every write and delete in the copy goes through this.
 */
export function pathInsideCopy(copyRoot, file) {
  if (typeof file !== "string" || file.length === 0 || path.isAbsolute(file) || /^[A-Za-z]:/.test(file)) return undefined;
  const root = path.resolve(copyRoot);
  const target = path.resolve(root, file);
  const offset = path.relative(root, target);
  if (offset === "" || offset.startsWith("..") || path.isAbsolute(offset)) return undefined;
  // The deepest existing ancestor must also resolve inside the copy, so a
  // junction or symlink inside the copy cannot redirect the operation.
  if (!existsSync(root)) return target;
  let existing = path.dirname(target);
  while (existing.length > root.length && !existsSync(existing)) existing = path.dirname(existing);
  const realRoot = realpathSync(root);
  const realExisting = realpathSync(existing);
  const realOffset = path.relative(realRoot, realExisting);
  if (realOffset.startsWith("..") || path.isAbsolute(realOffset)) return undefined;
  return target;
}

function syncCopy(copyRoot, files, transform) {
  mkdirSync(copyRoot, { recursive: true });
  const ledgerPath = path.join(copyRoot, ".rust-deadcode-files.json");
  let previous = [];
  try {
    const parsed = existsSync(ledgerPath) ? JSON.parse(readFileSync(ledgerPath, "utf8")) : [];
    if (Array.isArray(parsed)) previous = parsed;
  } catch {
    // A corrupt ledger only means stale copy files may survive one run.
  }
  let written = 0;
  for (const file of files) {
    const destination = pathInsideCopy(copyRoot, file);
    if (!destination) throw new Error(`refusing to copy a path outside the copy: ${file}`);
    const source = readFileSync(path.join(REPO_ROOT, file));
    const content = transform(file, source);
    if (existsSync(destination)) {
      const current = readFileSync(destination);
      if (Buffer.isBuffer(content) ? current.equals(content) : current.toString("utf8") === content) continue;
    }
    mkdirSync(path.dirname(destination), { recursive: true });
    writeFileSync(destination, content);
    written += 1;
  }
  const keep = new Set(files);
  for (const file of previous) {
    if (keep.has(file)) continue;
    const stale = pathInsideCopy(copyRoot, file);
    if (stale) rmSync(stale, { force: true });
  }
  writeFileSync(ledgerPath, JSON.stringify(files));
  return written;
}

/** Turn one rustc dead_code diagnostic into findings (one per primary span). */
export function deadCodeFindings(message, toRepoPath) {
  if (message?.code?.code !== "dead_code") return [];
  // "function `x` is never used", "methods `a` and `b` are never used",
  // "multiple fields are never read" -> "function", "method", "field".
  const kind = message.message
    .split("`")[0]
    .replace(/^multiple /, "")
    .replace(/ (?:is|are) never .*$/, "")
    .trim()
    .replace(/s$/, "");
  let parent;
  for (const span of message.spans) {
    if (span.is_primary || !span.label) continue;
    if (!/in this (struct|enum|union|implementation|trait|variant)/.test(span.label)) continue;
    const text = span.text?.[0];
    if (!text) continue;
    const highlighted = text.text.slice(text.highlight_start - 1, text.highlight_end - 1);
    const implMatch = highlighted.match(/^impl\s*(?:<[^{]*?>)?\s+(?:[^{]*?\bfor\s+)?(?:\w+::)*([A-Za-z_]\w*)/);
    parent = implMatch ? implMatch[1] : highlighted.match(/[A-Za-z_]\w*/)?.[0];
  }
  return message.spans
    .filter((span) => span.is_primary)
    .map((span) => {
      const text = span.text?.[0];
      const name = text ? text.text.slice(text.highlight_start - 1, text.highlight_end - 1) : "?";
      const file = toRepoPath(span.file_name);
      const qualified = parent && parent !== name ? `${parent}.${name}` : name;
      return {
        key: `${file}::${qualified}`,
        file,
        line: span.line_start,
        name,
        parent: parent && parent !== name ? parent : undefined,
        kind,
        source: "rustc",
      };
    });
}

function runRustcPass(metadata, workspace, options) {
  const app = workspace.packages.find((pkg) => pkg.name === APP_PACKAGE);
  if (!app?.lib) throw new Error(`workspace package ${APP_PACKAGE} with a library target not found`);
  const libDir = path.dirname(app.lib.src_path);
  const libName = app.lib.name;
  const keepPublic = externalRoots(app, libName);
  const binFiles = new Set(app.bins.map((bin) => path.resolve(bin.src_path)));

  const workspaceRoot = metadata.workspace_root;
  const memberDirs = workspace.packages.map((pkg) => toPosix(path.relative(workspaceRoot, pkg.dir)));
  const copiedRoots = memberDirs.map((dir) => path.join(workspaceRoot, dir));
  const isCopied = (resolved) =>
    copiedRoots.some((dir) => resolved === dir || resolved.startsWith(dir + path.sep));
  const hash = createHash("sha1").update(workspaceRoot).digest("hex").slice(0, 12);
  const copyRoot = path.join(metadata.target_directory, "rust-deadcode", hash);

  const files = listCopiedFiles(memberDirs);
  const libRoot = libDir + path.sep;
  const written = syncCopy(copyRoot, files, (file, source) => {
    const absolute = path.join(workspaceRoot, file);
    if (path.basename(file) === "Cargo.toml") {
      return rewriteManifestPaths(source.toString("utf8"), path.dirname(absolute), isCopied);
    }
    if (file.endsWith(".rs") && absolute.startsWith(libRoot) && !binFiles.has(absolute)) {
      const keep = absolute === path.resolve(app.lib.src_path) ? keepPublic : [];
      return rewriteVisibility(source.toString("utf8"), keep);
    }
    return source;
  });

  const args = [
    "check",
    "--workspace",
    "--lib",
    "--message-format=json",
    "--manifest-path",
    path.join(copyRoot, "Cargo.toml"),
    "--target-dir",
    metadata.target_directory,
  ];
  if (options.verbose) console.log(`rustc pass: ${written} file(s) synced into ${copyRoot}`);
  const result = spawnSync("cargo", args, {
    cwd: copyRoot,
    encoding: "utf8",
    maxBuffer: 512 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error) throw result.error;
  const messages = result.stdout
    .split(/\r?\n/)
    .filter((line) => line.startsWith("{"))
    .map((line) => JSON.parse(line));
  if (result.status !== 0) {
    for (const message of messages) {
      if (message.reason === "compiler-message" && message.message.level === "error") {
        process.stderr.write(message.message.rendered);
      }
    }
    process.stderr.write(result.stderr.split(/\r?\n/).slice(-20).join("\n"));
    throw new Error("cargo check of the visibility-rewritten copy failed");
  }
  const appDirInCopy = path.join(copyRoot, toPosix(path.relative(workspaceRoot, libDir)));
  const findings = [];
  for (const message of messages) {
    if (message.reason !== "compiler-message") continue;
    const toRepoPath = (fileName) => {
      const absolute = path.resolve(copyRoot, fileName);
      return toPosix(path.relative(copyRoot, absolute));
    };
    for (const finding of deadCodeFindings(message.message, toRepoPath)) {
      if (path.resolve(copyRoot, finding.file).startsWith(appDirInCopy + path.sep)) findings.push(finding);
    }
  }
  return dedupe(findings);
}

function dedupe(findings) {
  const seen = new Map();
  for (const finding of findings) if (!seen.has(finding.key)) seen.set(finding.key, finding);
  return [...seen.values()];
}

/**
 * Set aside rustc findings whose name is used in production code that this
 * platform compiles out: rustc could not see that caller.
 */
export function splitCfgGated(findings, analyses) {
  const names = new Set(findings.map((finding) => finding.name));
  const gated = new Set();
  for (const analysis of analyses) {
    for (const reference of collectReferences(analysis, names)) {
      if (!reference.inactive || reference.testOnly) continue;
      const region = inactiveRegionAt(analysis, reference.index);
      if (!region) continue;
      for (const finding of findings) {
        if (finding.name !== reference.name) continue;
        // A method, field, or variant name is too common to match alone
        // (`new`, `id`, `contains`): the gated code must also name its type.
        if (finding.parent && !regionNames(analysis, region, finding.parent)) continue;
        gated.add(finding);
      }
    }
  }
  return {
    kept: findings.filter((finding) => !gated.has(finding)),
    gated: findings.filter((finding) => gated.has(finding)),
  };
}

function regionNames(analysis, [start, end], name) {
  for (let index = start; index < end; index += 1) {
    if (analysis.tokens.kinds[index] === IDENT && analysis.tokens.values[index] === name) return true;
  }
  return false;
}

// ---------------------------------------------------------------------------
// Check 2: shared library crates through token search

/**
 * Items of one library crate that production code cannot reach by name.
 *
 * Every item of the crate (any visibility) is a node. A reference from
 * production code outside the crate, or from crate code outside any node
 * (trait impls, macro bodies), is a root. A reference inside a node is an
 * edge from that node. Items whose name no root reaches are dead, so a chain
 * or cycle of items that only call each other is dead as a whole.
 *
 * @param {Map<string, object>} libraryFiles analyses of the crate's production files
 * @param {Map<string, object>} allFiles analyses of every production file in the workspace
 */
export function unreachableItems(libraryFiles, allFiles) {
  const structure = new Map();
  const nodes = [];
  for (const [file, analysis] of libraryFiles) {
    const defined = definedItems(analysis);
    structure.set(file, defined);
    for (const item of defined.items) nodes.push({ ...item, file });
  }
  const names = new Set(nodes.map((node) => node.name));
  const nodesByName = new Map();
  const nodesByFile = new Map();
  const edges = new Map();
  for (const node of nodes) {
    edges.set(node, new Set());
    if (!nodesByName.has(node.name)) nodesByName.set(node.name, []);
    nodesByName.get(node.name).push(node);
    if (!nodesByFile.has(node.file)) nodesByFile.set(node.file, []);
    nodesByFile.get(node.file).push(node);
  }

  const roots = new Set();
  for (const [file, analysis] of allFiles) {
    if (libraryFiles.has(file)) continue;
    for (const reference of collectReferences(analysis, names)) if (!reference.testOnly) roots.add(reference.name);
  }
  for (const [file, analysis] of libraryFiles) {
    const { implHeaders } = structure.get(file);
    const fileNodes = (nodesByFile.get(file) ?? []).sort((left, right) => left.start - right.start);
    const references = collectReferences(analysis, names)
      .filter((reference) => !reference.testOnly)
      .filter((reference) => !implHeaders.some(([start, end]) => reference.index >= start && reference.index < end))
      .sort((left, right) => left.index - right.index);
    // Nodes nest (a fn inside a fn), so the innermost open node owns a reference.
    const open = [];
    let next = 0;
    for (const reference of references) {
      while (next < fileNodes.length && fileNodes[next].start <= reference.index) {
        const node = fileNodes[next];
        next += 1;
        while (open.length > 0 && open.at(-1).end <= node.start) open.pop();
        open.push(node);
      }
      while (open.length > 0 && open.at(-1).end <= reference.index) open.pop();
      const owner = open.at(-1);
      if (owner) edges.get(owner).add(reference.name);
      else roots.add(reference.name);
    }
  }

  const live = new Set();
  const queue = [...roots];
  while (queue.length > 0) {
    const name = queue.pop();
    if (live.has(name)) continue;
    live.add(name);
    for (const node of nodesByName.get(name) ?? []) {
      for (const target of edges.get(node)) if (!live.has(target)) queue.push(target);
    }
  }
  return nodes.filter((node) => !live.has(node.name));
}

function sharedLibraryFindings(workspace) {
  const app = workspace.packages.find((pkg) => pkg.name === APP_PACKAGE);
  const shared = workspace.packages.filter((pkg) => pkg !== app && pkg.lib);
  const findings = [];
  for (const pkg of shared) {
    const libraryFiles = new Map([...pkg.files].map((file) => [file, workspace.files.get(file)]));
    for (const item of unreachableItems(libraryFiles, workspace.files)) {
      const file = relative(item.file);
      findings.push({ key: `${file}::${item.name}`, file, line: item.line, name: item.name, kind: item.kind, source: pkg.name });
    }
  }
  return dedupe(findings);
}

// ---------------------------------------------------------------------------
// Check 3: Tauri commands

/** Command function names listed in `tauri::generate_handler![...]`. */
export function registeredCommands(libSource) {
  const tokens = lexRust(libSource);
  const commands = [];
  for (let index = 0; index < tokens.count; index += 1) {
    if (tokens.values[index] !== "generate_handler" || tokens.values[index + 1] !== "!") continue;
    let depth = 0;
    let last;
    for (let cursor = index + 2; cursor < tokens.count; cursor += 1) {
      const value = tokens.values[cursor];
      const kind = tokens.kinds[cursor];
      if (kind === PUNCT && value === "#") {
        // Skip an attribute such as #[cfg(debug_assertions)].
        let bracket = 0;
        for (cursor += 1; cursor < tokens.count; cursor += 1) {
          if (tokens.values[cursor] === "[") bracket += 1;
          else if (tokens.values[cursor] === "]" && --bracket === 0) break;
        }
        continue;
      }
      if (kind === PUNCT && value === "[") depth += 1;
      else if (kind === PUNCT && value === "]") {
        depth -= 1;
        if (depth === 0) {
          if (last) commands.push(last);
          break;
        }
      } else if (kind === PUNCT && value === ",") {
        if (last) commands.push(last);
        last = undefined;
      } else if (kind === IDENT) last = value;
    }
  }
  return commands;
}

function walk(dir, found = []) {
  if (!existsSync(dir)) return found;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (SKIPPED_DIRS.has(entry.name)) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, found);
    else if (/\.[cm]?[jt]sx?$/.test(entry.name)) found.push(full);
  }
  return found;
}

const QUOTED_NAME = /['"`]([a-z_][a-z0-9_]*)['"`]/g;

function quotedNames(files) {
  const names = new Set();
  for (const file of files) {
    for (const hit of readFileSync(file, "utf8").matchAll(QUOTED_NAME)) names.add(hit[1]);
  }
  return names;
}

function commandFindings(workspace) {
  const app = workspace.packages.find((pkg) => pkg.name === APP_PACKAGE);
  const commands = registeredCommands(readFileSync(app.lib.src_path, "utf8"));
  const production = quotedNames(
    FRONTEND_DIRS.flatMap((dir) => walk(path.join(REPO_ROOT, dir))).filter((file) => !TEST_FILE.test(relative(file))),
  );
  // Rust code that names a command as a string (for example a remote
  // gateway allowlist) is a production caller too.
  for (const analysis of workspace.files.values()) {
    for (let index = 0; index < analysis.tokens.count; index += 1) {
      if (analysis.tokens.kinds[index] === STR && !analysis.testOnly[index]) production.add(analysis.tokens.values[index]);
    }
  }
  const tooling = quotedNames(TOOLING_DIRS.flatMap((dir) => walk(path.join(REPO_ROOT, dir))));
  return commands
    .filter((name) => !production.has(name) && !(name.startsWith("debug_") && tooling.has(name)))
    .map((name) => ({
      key: `${COMMAND_PREFIX}${name}`,
      file: relative(app.lib.src_path),
      line: 0,
      name,
      kind: "tauri command",
      source: "commands",
    }));
}

// ---------------------------------------------------------------------------

/**
 * Whether this run could have observed a baseline entry: an entry naming an
 * item the current platform compiles out is neither reported nor stale here.
 */
function checkableOnThisPlatform(entry, workspace) {
  if (entry.startsWith(COMMAND_PREFIX)) return true;
  const [file, qualified] = entry.split("::");
  const name = qualified?.split(".").pop();
  const analysis = workspace.files.get(path.resolve(REPO_ROOT, file));
  if (!analysis || !name) return true;
  let sawName = false;
  for (let index = 0; index < analysis.tokens.count; index += 1) {
    if (analysis.tokens.kinds[index] !== IDENT || analysis.tokens.values[index] !== name) continue;
    sawName = true;
    if (!analysis.inactive[index]) return true;
  }
  return !sawName;
}

export function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  const started = Date.now();
  const timings = [];
  const time = (label, run) => {
    const begin = Date.now();
    const value = run();
    timings.push(`${label} ${((Date.now() - begin) / 1000).toFixed(1)}s`);
    return value;
  };

  const metadata = time("metadata", cargoMetadata);
  const workspace = time("index", () => loadWorkspace(metadata, productionCfgEnv()));
  if (workspace.unresolved.length > 0) {
    throw new Error(`could not resolve module files:\n  ${workspace.unresolved.join("\n  ")}`);
  }
  const app = workspace.packages.find((pkg) => pkg.name === APP_PACKAGE);
  const rustc = time("rustc", () => runRustcPass(metadata, workspace, options));
  const { kept, gated } = splitCfgGated(rustc, [...app.files].map((file) => workspace.files.get(file)));
  const shared = time("shared", () => sharedLibraryFindings(workspace));
  const commands = time("commands", () => commandFindings(workspace));
  const findings = [...kept, ...shared, ...commands];

  const baselineText = readFileSync(BASELINE_PATH, "utf8");
  const baseline = readBaseline(baselineText);
  const { added, stale } = compareWithBaseline(findings, baseline, (entry) => checkableOnThisPlatform(entry, workspace));

  if (options.verbose) {
    for (const finding of gated) console.log(`cfg-gated caller: ${finding.key}`);
    for (const finding of findings) {
      console.log(`${baseline.has(finding.key) ? "baseline" : "NEW     "} ${finding.key}`);
    }
  }
  console.log(
    `Rust dead code: ${findings.length} finding(s), ${baseline.size} baselined, ${gated.length} with a cfg-gated caller `
      + `(${timings.join(", ")}, total ${((Date.now() - started) / 1000).toFixed(1)}s)`,
  );

  if (options.prune && stale.length > 0) {
    writeFileSync(BASELINE_PATH, pruneBaseline(baselineText, stale));
    console.log(`Pruned ${stale.length} stale baseline entr${stale.length === 1 ? "y" : "ies"}.`);
  } else if (stale.length > 0) {
    console.error(`\n${stale.length} baseline entr${stale.length === 1 ? "y no longer matches" : "ies no longer match"} anything. Remove ${stale.length === 1 ? "it" : "them"}, or run with --prune:`);
    for (const entry of stale) console.error(`  ${entry}`);
  }
  if (added.length > 0) {
    console.error(`\n${added.length} Rust item(s) have no production caller (tests do not count):`);
    for (const finding of added) {
      const where = finding.line ? `${finding.file}:${finding.line}` : finding.file;
      console.error(`  ${where}  ${finding.kind} \`${finding.name}\``);
      console.error(`    baseline entry: "${finding.key}"`);
    }
    console.error(
      "\nDelete the item, call it from production code, or (with a stated reason) add the entry to "
        + "scripts/rust-deadcode-baseline.json. See docs/developer/ci-verification.md#rust-dead-code.",
    );
  }
  return added.length > 0 || (stale.length > 0 && !options.prune) ? 1 : 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    process.exitCode = main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
