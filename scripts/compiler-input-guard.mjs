import { homedir } from 'node:os';
import { lstatSync, readFileSync, realpathSync } from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const INVENTORY_ENV = 'WARDIAN_PROTECTED_INPUT_MANIFESTS';

function unknown(reason) {
  throw new Error(`Compiler input guard: cannot derive writable roots: ${reason}`);
}

function stat(file) {
  try { return lstatSync(file); } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

function absolute(value, cwd) {
  if (typeof value !== 'string' || !value.trim() || /[\0\r\n]/.test(value)) unknown('invalid path');
  // Lexical normalization could erase a junction/symlink before .. is resolved.
  if (value.split(/[\\/]/).includes('..')) unknown('parent traversal in path');
  if (process.platform === 'win32') {
    // Drive-relative paths, device namespaces, ADS and Win32 name normalization
    // cannot safely be treated as ordinary path segments.
    if (/^[A-Za-z]:(?:[^\\/]|$)/.test(value) || /^\\\\[?.]\\/.test(value)
      || value.replace(/^[A-Za-z]:/, '').includes(':')
      || value.split(/[\\/]/).some((part) => part !== '.' && part !== '..' && /[. ]$/.test(part))) {
      unknown('ambiguous Windows path');
    }
  }
  return path.resolve(cwd, value);
}

/** Resolve links in existing ancestors without creating a missing target tree. */
export function canonicalPath(value, cwd = process.cwd()) {
  const resolved = absolute(value, cwd);
  let ancestor = resolved;
  const suffix = [];
  while (!stat(ancestor)) {
    const parent = path.dirname(ancestor);
    if (parent === ancestor) unknown('no existing path ancestor');
    suffix.unshift(path.basename(ancestor));
    ancestor = parent;
  }
  const actual = realpathSync.native(ancestor); // dangling links and access errors fail closed
  if (suffix.length && !stat(actual)?.isDirectory()) unknown('path ancestor is not a directory');
  const canonical = path.join(actual, ...suffix);
  return process.platform === 'win32' ? canonical.toLowerCase() : canonical;
}

/** Only explicitly supplied manifests define protected inputs; never discover them. */
export function readProtectedInputs(env = process.env) {
  if (env[INVENTORY_ENV] === undefined) return null;
  let manifests;
  try { manifests = JSON.parse(env[INVENTORY_ENV]); } catch { unknown('inventory carrier must be a JSON array'); }
  if (!Array.isArray(manifests) || manifests.length === 0) unknown('inventory carrier must be a nonempty JSON array');
  return manifests.flatMap((manifest) => {
    if (typeof manifest !== 'string' || !path.isAbsolute(manifest)) unknown('manifest paths must be absolute');
    const inventory = JSON.parse(readFileSync(manifest, 'utf8'));
    const files = Array.isArray(inventory) ? inventory : inventory?.files;
    if (!Array.isArray(files)) unknown('manifest must be an array or contain files[]');
    return files.map((file) => {
      if (typeof file?.path !== 'string' || !path.isAbsolute(file.path)) unknown('protected paths must be absolute');
      return canonicalPath(file.path);
    });
  });
}

function stringValue(text) {
  const literal = text.match(/^'([^'\r\n]*)'\s*(?:#.*)?$/);
  if (literal) return literal[1];
  const basic = text.match(/^("(?:[^"\\]|\\.)*")\s*(?:#.*)?$/);
  if (!basic) unknown('unsupported path value in Cargo config');
  try { return JSON.parse(basic[1]); } catch { unknown('unsupported Cargo string escape'); }
}

// This is deliberately a conservative reader, not a partial general TOML parser.
// Unsupported syntax that could affect output placement must deny admission.
function configRoots(text, base) {
  const roots = {};
  let section = '';
  for (const original of text.split(/\r?\n/)) {
    const line = original.trim();
    if (!line || line.startsWith('#')) continue;
    const header = line.match(/^\[([\w.-]+)\]\s*(?:#.*)?$/);
    if (header) { section = header[1]; continue; }
    if (line.startsWith('[')) unknown('unsupported Cargo config table');
    const entry = line.match(/^([\w.-]+)\s*=\s*(.*)$/);
    if (!entry) unknown('unsupported Cargo config syntax');
    const key = section ? `${section}.${entry[1]}` : entry[1];
    if (key === 'build.target-dir' || key === 'build.build-dir') {
      const value = stringValue(entry[2]);
      if (/[{}]/.test(value)) unknown('templated Cargo output directory');
      roots[key] = absolute(value, base);
    } else if (/(?:target-dir|build-dir|include|rustflags|rustdocflags|rustc|rustdoc|^env(?:\.|$)|^build$)/.test(key)) {
      unknown(`unsupported Cargo placement setting ${key}`);
    }
  }
  return roots;
}

function readConfig(directory, base) {
  // Cargo gives the extensionless spelling precedence when both exist.
  const legacy = path.join(directory, 'config');
  const file = stat(legacy) ? legacy : path.join(directory, 'config.toml');
  return stat(file) ? configRoots(readFileSync(file, 'utf8'), base) : {};
}

function cargoConfig(cwd, env) {
  const cargoHome = absolute(env.CARGO_HOME ?? path.join(homedir(), '.cargo'), cwd);
  let config = readConfig(cargoHome, path.dirname(cargoHome));
  const ancestors = [];
  for (let dir = cwd; ; dir = path.dirname(dir)) {
    ancestors.unshift(dir);
    if (path.dirname(dir) === dir) break;
  }
  for (const dir of ancestors) config = { ...config, ...readConfig(path.join(dir, '.cargo'), dir) };
  return config;
}

function defaultTarget(cwd, manifest) {
  if (manifest && path.basename(manifest) !== 'Cargo.toml') unknown('nonstandard manifest path');
  let dir = manifest ? path.dirname(absolute(manifest, cwd)) : cwd;
  for (; ; dir = path.dirname(dir)) {
    const file = path.join(dir, 'Cargo.toml');
    if (stat(file)) {
      const text = readFileSync(file, 'utf8');
      // A root workspace is determinate. Member/package discovery requires Cargo's
      // full manifest semantics; do not guess when no explicit output root exists.
      const firstContent = text.replace(/^(?:\s*#.*\r?\n|\s*\r?\n)*/, '');
      if (/^\[workspace\][ \t]*(?:#.*)?(?:\r?\n|$)/.test(firstContent)) return path.join(dir, 'target');
      unknown('package default target requires an explicit target directory');
    }
    if (path.dirname(dir) === dir) unknown('no workspace manifest or explicit target directory');
  }
}

/** Derive Cargo target/build trees without launching Cargo, rustc or metadata. */
export function compilerWritableRoots(args, cwd = process.cwd(), env = process.env) {
  cwd = absolute(cwd, process.cwd());
  const physicalCwd = canonicalPath(cwd);
  if (physicalCwd !== (process.platform === 'win32' ? cwd.toLowerCase() : cwd)) unknown('aliased compiler cwd config hierarchy');
  for (const [key, value] of Object.entries(env)) {
    if (value && /^(?:RUSTFLAGS|RUSTDOCFLAGS|CARGO_ENCODED_RUST(?:DOC)?FLAGS|RUSTC(?:_WRAPPER|_WORKSPACE_WRAPPER)?|RUSTDOC|CARGO_BUILD_RUST.*|CARGO_TARGET_.*_RUST.*)$/.test(key.toUpperCase())) {
      unknown(`unsupported compiler environment ${key}`);
    }
  }
  let target;
  let manifest;
  let subcommand;
  const overrides = [];
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === '--') {
      if (args.slice(index + 1).some((arg) => /^(?:--out-dir|--emit|--output|-o|-C|-Z)/.test(arg))) unknown('forwarded compiler output option');
      break;
    }
    if (argument.startsWith('+') && !subcommand) continue;
    const option = argument.split('=')[0];
    if (['--target-dir', '--manifest-path', '--config'].includes(option)) {
      const value = argument.includes('=') ? argument.slice(option.length + 1) : args[++index];
      if (!value) unknown(`missing ${option} value`);
      if (option === '--target-dir') target = absolute(value, cwd);
      if (option === '--manifest-path') manifest = value;
      if (option === '--config') overrides.push(value);
    } else if (/^(?:-C|-Z|--build-dir|--out-dir|--artifact-dir|--lockfile-path)/.test(argument)) {
      unknown(`unsupported Cargo option ${option}`);
    } else if (!argument.startsWith('-') && !subcommand) {
      subcommand = argument;
    }
  }
  if (!['build', 'check', 'clippy', 'test', 'doc', 'fmt', 'metadata'].includes(subcommand)) unknown('unsupported Cargo subcommand');
  let config = cargoConfig(cwd, env);
  // CARGO_TARGET_DIR is a special Cargo variable, not just a generic config
  // environment override. Do not guess which target wins across conflicting
  // explicit --config and special environment settings.
  if (!target && env.CARGO_TARGET_DIR !== undefined && env.CARGO_BUILD_TARGET_DIR !== undefined
    && canonicalPath(env.CARGO_TARGET_DIR, cwd) !== canonicalPath(env.CARGO_BUILD_TARGET_DIR, cwd)) {
    unknown('conflicting Cargo target environment settings');
  }
  for (const key of ['CARGO_BUILD_TARGET_DIR', 'CARGO_TARGET_DIR']) {
    if (env[key] !== undefined) config['build.target-dir'] = absolute(env[key], cwd);
  }
  if (env.CARGO_BUILD_BUILD_DIR !== undefined) {
    if (/[{}]/.test(env.CARGO_BUILD_BUILD_DIR)) unknown('templated Cargo build directory');
    config['build.build-dir'] = absolute(env.CARGO_BUILD_BUILD_DIR, cwd);
  }
  for (const value of overrides) {
    let override;
    if (value.includes('=')) override = configRoots(value, cwd);
    else {
      const file = absolute(value, cwd);
      override = configRoots(readFileSync(file, 'utf8'), path.dirname(path.dirname(file)));
    }
    if (!target && env.CARGO_TARGET_DIR !== undefined && override['build.target-dir'] !== undefined
      && canonicalPath(env.CARGO_TARGET_DIR, cwd) !== canonicalPath(override['build.target-dir'])) {
      unknown('conflicting --config target and CARGO_TARGET_DIR');
    }
    config = { ...config, ...override };
  }
  target ??= config['build.target-dir'] ?? defaultTarget(cwd, manifest);
  return [...new Set([target, config['build.build-dir'] ?? target].map((root) => canonicalPath(root)))];
}

/** Deny before spawn if any scheduler-protected file is within a writable tree. */
export function assertCompilerAdmission({ program, args, cwd = process.cwd(), env = process.env }) {
  const protectedInputs = readProtectedInputs(env);
  if (protectedInputs === null) return { guarded: false, writable_roots: [] };
  const name = path.basename(program).toLowerCase().replace(/\.(?:exe|cmd)$/, '');
  let cargoArgs;
  if (name === 'cargo') cargoArgs = args;
  else if (name === 'npm' && args.join(' ') === 'run check:rust-deadcode') {
    // The pinned script gets metadata from the original cwd, then explicitly
    // passes metadata.target_directory to check --target-dir. The source-copy
    // cwd is a child of that tree, not the compiler target argument.
    cargoArgs = ['metadata'];
  }
  else unknown('unsupported compiler launcher');
  const roots = compilerWritableRoots(cargoArgs, cwd, env);
  for (const root of roots) {
    for (const input of protectedInputs) {
      const relative = path.relative(root, input);
      if (relative === '' || (!path.isAbsolute(relative) && relative !== '..' && !relative.startsWith(`..${path.sep}`))) {
        throw new Error(`Compiler input guard: protected input overlaps writable tree: ${input} (root: ${root})`);
      }
    }
  }
  return { guarded: true, writable_roots: roots };
}

function shellWords(command) {
  // Restrict admission to literal commands. Shell expansion or control flow can
  // change cwd/env/argv after admission, so those forms cannot be certified.
  if (/[\r\n$`%&|;<>^!]/.test(command)) unknown('shell expansion or compound command');
  const words = [];
  const expression = /"([^"]*)"|'([^']*)'|([^\s"']+)/gy;
  let index = 0;
  while (index < command.length) {
    if (/\s/.test(command[index])) { index += 1; continue; }
    expression.lastIndex = index;
    const match = expression.exec(command);
    if (!match || (process.platform === 'win32' && match[2] !== undefined)) unknown('unsupported shell quoting');
    if (process.platform !== 'win32' && match[3]?.includes('\\')) unknown('unquoted shell escape');
    words.push(match[1] ?? match[2] ?? match[3]);
    index = expression.lastIndex;
    if (index < command.length && !/\s/.test(command[index])) unknown('concatenated shell tokens');
  }
  return words;
}

/** Admission boundary for literal commands in the normal local CI runner. */
export function assertVerificationAdmission(command, options = {}) {
  const env = options.env ?? process.env;
  if (env[INVENTORY_ENV] === undefined) return;
  const [program, ...args] = shellWords(command);
  // These CI-owned npm checks do not start Rust compilation. The nested
  // rust-deadcode launch reaches the shared metadata-target admission below.
  const nonCompiler = /^npm run (?:typecheck|lint|test|build|check:(?:workbench-cutover|test-reachability|deadcode(?::production)?|budgets|page-fixtures)|docs:(?:check-llms|build))$/;
  if (nonCompiler.test(command)) return;
  return assertCompilerAdmission({ program, args, ...options, env });
}

/** Shared CLI for a scheduler wrapper: --cwd <cwd> --program <exe> -- <argv>. */
export function main(argv = process.argv.slice(2)) {
  const separator = argv.indexOf('--');
  if (separator !== 4 || argv[0] !== '--cwd' || argv[2] !== '--program') {
    throw new Error('Usage: node scripts/compiler-input-guard.mjs --cwd <cwd> --program <exe> -- <argv>');
  }
  const result = assertCompilerAdmission({ cwd: argv[1], program: argv[3], args: argv.slice(5) });
  console.log(JSON.stringify(result));
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try { process.exitCode = main(); } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
