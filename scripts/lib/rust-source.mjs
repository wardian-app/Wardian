/**
 * A token-level view of Rust sources for the dead-code gate.
 *
 * This is deliberately not a parser. It strips comments and literals, follows
 * `mod` declarations from each crate root the way rustc resolves files, and
 * marks every token with two facts the gate needs:
 *
 * - `testOnly`: the token is compiled only for tests (`#[cfg(test)]`,
 *   `#[cfg(all(test, ..))]`, `#[test]`, or a module reached through one).
 * - `inactive`: the token is not compiled in a production build on the
 *   platform running the gate (for example `#[cfg(unix)]` code on Windows).
 *
 * Both are conservative approximations. Where the item extent of a cfg
 * attribute is ambiguous, the region ends early, which can only make a caller
 * count as production code, never hide one.
 */
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

export const IDENT = 1;
export const PUNCT = 2;
export const STR = 3;
export const OTHER = 4;

const IDENT_START = /[\p{L}_]/u;
const IDENT_CONT = /[\p{L}\p{N}_]/u;

function countNewlines(text) {
  let count = 0;
  for (let index = text.indexOf("\n"); index !== -1; index = text.indexOf("\n", index + 1)) count += 1;
  return count;
}

/**
 * Split Rust source into identifier, punctuation, and string tokens.
 * Comments (including doc comments) and whitespace are dropped, so a name that
 * appears only in prose is not a reference.
 */
export function lexRust(source) {
  const kinds = [];
  const values = [];
  const lines = [];
  const push = (kind, value, line) => {
    kinds.push(kind);
    values.push(value);
    lines.push(line);
  };
  let index = 0;
  let line = 1;
  const length = source.length;

  const readQuoted = (start) => {
    // `start` is the opening quote. Returns the index after the closing quote.
    let cursor = start + 1;
    while (cursor < length && source[cursor] !== '"') {
      if (source[cursor] === "\\") cursor += 1;
      cursor += 1;
    }
    return Math.min(cursor + 1, length);
  };

  while (index < length) {
    const char = source[index];
    if (char === "\n") {
      line += 1;
      index += 1;
      continue;
    }
    if (char === " " || char === "\t" || char === "\r") {
      index += 1;
      continue;
    }
    if (char === "/" && source[index + 1] === "/") {
      while (index < length && source[index] !== "\n") index += 1;
      continue;
    }
    if (char === "/" && source[index + 1] === "*") {
      let depth = 1;
      index += 2;
      while (index < length && depth > 0) {
        if (source[index] === "/" && source[index + 1] === "*") {
          depth += 1;
          index += 2;
        } else if (source[index] === "*" && source[index + 1] === "/") {
          depth -= 1;
          index += 2;
        } else {
          if (source[index] === "\n") line += 1;
          index += 1;
        }
      }
      continue;
    }

    // String-like literals with an optional b/c/r prefix.
    let prefixEnd = index;
    if (char === "b" || char === "c") prefixEnd += 1;
    if (source[prefixEnd] === "r" && (source[prefixEnd + 1] === '"' || source[prefixEnd + 1] === "#")) {
      let cursor = prefixEnd + 1;
      let hashes = 0;
      while (source[cursor] === "#") {
        hashes += 1;
        cursor += 1;
      }
      if (source[cursor] === '"') {
        const close = `"${"#".repeat(hashes)}`;
        const end = source.indexOf(close, cursor + 1);
        const stop = end === -1 ? length : end + close.length;
        const body = source.slice(cursor + 1, end === -1 ? length : end);
        push(STR, body, line);
        line += countNewlines(body);
        index = stop;
        continue;
      }
      if (prefixEnd === index && hashes === 1 && IDENT_START.test(source[cursor] ?? "")) {
        // Raw identifier `r#name`.
        let end = cursor;
        while (end < length && IDENT_CONT.test(source[end])) end += 1;
        push(IDENT, source.slice(cursor, end), line);
        index = end;
        continue;
      }
    }
    if ((char === "b" || char === "c") && source[index + 1] === '"') {
      const stop = readQuoted(index + 1);
      const body = source.slice(index + 2, stop - 1);
      push(STR, body, line);
      line += countNewlines(body);
      index = stop;
      continue;
    }
    if (char === '"') {
      const stop = readQuoted(index);
      const body = source.slice(index + 1, stop - 1);
      push(STR, body, line);
      line += countNewlines(body);
      index = stop;
      continue;
    }
    if (char === "b" && source[index + 1] === "'") {
      let cursor = index + 2;
      if (source[cursor] === "\\") cursor += 1;
      cursor += 1;
      while (cursor < length && source[cursor] !== "'") cursor += 1;
      push(OTHER, "'", line);
      index = cursor + 1;
      continue;
    }
    if (char === "'") {
      // Character literal or lifetime/label.
      if (source[index + 1] === "\\") {
        let cursor = index + 2;
        while (cursor < length && source[cursor] !== "'") cursor += 1;
        push(OTHER, "'", line);
        index = cursor + 1;
        continue;
      }
      const codePoint = source.codePointAt(index + 1);
      const width = codePoint !== undefined && codePoint > 0xffff ? 2 : 1;
      if (source[index + 1 + width] === "'") {
        push(OTHER, "'", line);
        index += 2 + width;
        continue;
      }
      let cursor = index + 1;
      while (cursor < length && IDENT_CONT.test(source[cursor])) cursor += 1;
      push(OTHER, source.slice(index, cursor), line);
      index = Math.max(cursor, index + 1);
      continue;
    }
    if (char >= "0" && char <= "9") {
      let cursor = index + 1;
      while (cursor < length) {
        const next = source[cursor];
        if (IDENT_CONT.test(next)) cursor += 1;
        else if (next === "." && source[cursor + 1] >= "0" && source[cursor + 1] <= "9") cursor += 1;
        else break;
      }
      push(OTHER, source.slice(index, cursor), line);
      index = cursor;
      continue;
    }
    if (IDENT_START.test(char)) {
      let cursor = index + 1;
      while (cursor < length && IDENT_CONT.test(source[cursor])) cursor += 1;
      push(IDENT, source.slice(index, cursor), line);
      index = cursor;
      continue;
    }
    push(PUNCT, char, line);
    index += 1;
  }
  return { kinds, values, lines, count: kinds.length };
}

/** Index of the matching bracket for every `(`, `[`, `{` and its closer. */
function matchBrackets(tokens) {
  const match = new Int32Array(tokens.count).fill(-1);
  const stack = [];
  const pairs = { ")": "(", "]": "[", "}": "{" };
  for (let index = 0; index < tokens.count; index += 1) {
    if (tokens.kinds[index] !== PUNCT) continue;
    const value = tokens.values[index];
    if (value === "(" || value === "[" || value === "{") {
      stack.push(index);
    } else if (pairs[value]) {
      // Unbalanced input (for example inside macro_rules! patterns) is
      // tolerated by unwinding to the nearest opener of the same kind.
      let depth = stack.length - 1;
      while (depth >= 0 && tokens.values[stack[depth]] !== pairs[value]) depth -= 1;
      if (depth < 0) continue;
      const open = stack[depth];
      stack.length = depth;
      match[open] = index;
      match[index] = open;
    }
  }
  return match;
}

/** Parse the token range of a cfg predicate into a small expression tree. */
function parseCfg(tokens, start, end) {
  let cursor = start;
  const parseOne = () => {
    if (cursor >= end || tokens.kinds[cursor] !== IDENT) return { op: "unknown" };
    const name = tokens.values[cursor];
    cursor += 1;
    if (tokens.values[cursor] === "=" && tokens.kinds[cursor + 1] === STR) {
      const value = tokens.values[cursor + 1];
      cursor += 2;
      return { op: "kv", name, value };
    }
    if (tokens.values[cursor] === "(") {
      cursor += 1;
      const args = [];
      while (cursor < end && tokens.values[cursor] !== ")") {
        args.push(parseOne());
        if (tokens.values[cursor] === ",") cursor += 1;
        else if (tokens.values[cursor] !== ")") cursor += 1;
      }
      cursor += 1;
      return { op: name, args };
    }
    return { op: "flag", name };
  };
  return parseOne();
}

/**
 * Evaluate a cfg expression in three-valued logic (true, false, undefined).
 * `env` maps a predicate to a value or undefined when it is not known.
 */
export function evalCfg(expr, env) {
  switch (expr.op) {
    case "all": {
      let result = true;
      for (const arg of expr.args) {
        const value = evalCfg(arg, env);
        if (value === false) return false;
        if (value === undefined) result = undefined;
      }
      return result;
    }
    case "any": {
      let result = false;
      for (const arg of expr.args) {
        const value = evalCfg(arg, env);
        if (value === true) return true;
        if (value === undefined) result = undefined;
      }
      return result;
    }
    case "not": {
      const value = evalCfg(expr.args[0] ?? { op: "unknown" }, env);
      return value === undefined ? undefined : !value;
    }
    case "flag":
      return env.flag(expr.name);
    case "kv":
      return env.kv(expr.name, expr.value);
    default:
      return undefined;
  }
}

const PLATFORM_OS = { win32: "windows", linux: "linux", darwin: "macos" };

/**
 * cfg environment of a non-test production build on `platform`.
 *
 * Only the predicates the gate can prove are answered; anything else (cargo
 * features, target_arch, custom cfgs) is unknown, so code behind it is never
 * treated as compiled out.
 */
export function productionCfgEnv(platform = process.platform) {
  const windows = platform === "win32";
  return {
    flag(name) {
      if (name === "test") return false;
      if (name === "debug_assertions") return true;
      if (name === "windows") return windows;
      if (name === "unix") return !windows;
      return undefined;
    },
    kv(name, value) {
      if (name === "target_os") return value === (PLATFORM_OS[platform] ?? platform);
      if (name === "target_family") return value === (windows ? "windows" : "unix");
      return undefined;
    },
  };
}

/** cfg environment where only `test` is known (false): any platform, any feature. */
const NON_TEST_ENV = {
  flag(name) {
    if (name === "test") return false;
    if (name === "debug_assertions") return true;
    return undefined;
  },
  kv() {
    return undefined;
  },
};

const ITEM_KEYWORDS = new Set([
  "fn", "impl", "mod", "trait", "struct", "enum", "union", "type", "static", "const", "use", "let",
  "macro_rules", "extern",
]);
const ITEM_QUALIFIERS = new Set(["pub", "async", "unsafe", "default", "crate", "super", "self", "in"]);

/**
 * End (exclusive) of the item, field, statement, or arm that starts at
 * `start`, skipping any further outer attributes first.
 */
export function itemEnd(tokens, match, start, limit = tokens.count) {
  let cursor = start;
  while (cursor < limit && tokens.values[cursor] === "#" && tokens.values[cursor + 1] === "[" && match[cursor + 1] > 0) {
    cursor = match[cursor + 1] + 1;
  }
  // Commas end fields, variants, and match arms, but not items whose header
  // can contain a top-level comma (`where A: X, B: Y`).
  let probe = cursor;
  while (probe < limit) {
    const value = tokens.values[probe];
    if (value === "(" && tokens.values[probe - 1] === "pub") probe = match[probe] + 1;
    else if (ITEM_QUALIFIERS.has(value) || tokens.kinds[probe] === STR) probe += 1;
    else break;
  }
  const commaEnds = !ITEM_KEYWORDS.has(tokens.values[probe]);
  for (let index = cursor; index < limit; index += 1) {
    if (tokens.kinds[index] !== PUNCT) continue;
    const value = tokens.values[index];
    if (value === "(" || value === "[") {
      if (match[index] < 0) return index;
      index = match[index];
    } else if (value === "{") {
      if (match[index] < 0) return index;
      return match[index] + 1;
    } else if (value === ")" || value === "]" || value === "}") {
      return index;
    } else if (value === ";" || (commaEnds && value === ",")) {
      return index + 1;
    }
  }
  return limit;
}

const TEST_ATTRIBUTES = new Set(["test", "bench"]);

/** Last path segment of an attribute: `tokio::test(...)` -> `test`. */
function attributeName(tokens, match, open, close) {
  let last = close - 1;
  if (tokens.values[last] === ")" && match[last] > open) last = match[last] - 1;
  // Only a bare path (`a::b::test`) names the attribute.
  for (let index = open + 1; index < last; index += 1) {
    if (tokens.kinds[index] !== IDENT && tokens.values[index] !== ":") return undefined;
  }
  return tokens.kinds[last] === IDENT ? tokens.values[last] : undefined;
}

/**
 * Lex a file and mark cfg regions.
 *
 * @param {string} source
 * @param {{ testOnly: boolean, inactive: boolean }} inherited flags of the module declaration
 * @param {ReturnType<typeof productionCfgEnv>} env production cfg of the current platform
 */
export function analyzeSource(source, inherited, env) {
  const tokens = lexRust(source);
  const match = matchBrackets(tokens);
  const testOnly = new Uint8Array(tokens.count).fill(inherited.testOnly ? 1 : 0);
  const inactive = new Uint8Array(tokens.count).fill(inherited.inactive ? 1 : 0);
  // Production regions compiled out on this platform, as [start, end) ranges.
  const inactiveRegions = inherited.inactive && !inherited.testOnly ? [[0, tokens.count]] : [];
  const mark = (start, end, isTest, isInactive) => {
    if (isTest) testOnly.fill(1, start, end);
    if (isInactive) inactive.fill(1, start, end);
    if (isInactive && !isTest) inactiveRegions.push([start, end]);
  };

  for (let index = 0; index < tokens.count; index += 1) {
    if (tokens.values[index] !== "#" || tokens.kinds[index] !== PUNCT) continue;
    const inner = tokens.values[index + 1] === "!";
    const open = inner ? index + 2 : index + 1;
    if (tokens.values[open] !== "[" || match[open] < 0) continue;
    const close = match[open];
    const head = tokens.values[open + 1];
    let isTest = false;
    let isInactive = false;
    if (head === "cfg" && tokens.values[open + 2] === "(") {
      const expr = parseCfg(tokens, open + 3, match[open + 2]);
      isTest = evalCfg(expr, NON_TEST_ENV) === false;
      isInactive = evalCfg(expr, env) === false;
    } else if (!inner && TEST_ATTRIBUTES.has(attributeName(tokens, match, open, close))) {
      // #[test], #[tokio::test], #[tokio::test(flavor = "multi_thread")]
      isTest = true;
      isInactive = true;
    }
    if (!isTest && !isInactive) continue;
    if (inner) {
      // An inner attribute applies to the enclosing module or file.
      let parentOpen = -1;
      for (let back = index - 1; back >= 0; back -= 1) {
        if (tokens.values[back] === "{" && match[back] > index) {
          parentOpen = back;
          break;
        }
      }
      if (parentOpen === -1) mark(0, tokens.count, isTest, isInactive);
      else mark(parentOpen, match[parentOpen] + 1, isTest, isInactive);
    } else {
      mark(index, itemEnd(tokens, match, close + 1), isTest, isInactive);
    }
    index = close;
  }
  return { tokens, match, testOnly, inactive, inactiveRegions };
}

/** The innermost inactive production region containing token `index`. */
export function inactiveRegionAt(analysis, index) {
  let best;
  for (const region of analysis.inactiveRegions) {
    if (index >= region[0] && index < region[1] && (!best || region[1] - region[0] < best[1] - best[0])) best = region;
  }
  return best;
}

/** The `#[path = "..."]` attribute among the attributes ending right before `index`. */
function precedingPathAttribute(tokens, match, index) {
  let cursor = index - 1;
  // Skip visibility: `pub`, `pub(crate)`, `pub(in path)`.
  if (tokens.values[cursor] === ")" && match[cursor] > 0 && tokens.values[match[cursor] - 1] === "pub") {
    cursor = match[cursor] - 2;
  } else if (tokens.values[cursor] === "pub") {
    cursor -= 1;
  }
  while (cursor > 0 && tokens.values[cursor] === "]" && match[cursor] > 0 && tokens.values[match[cursor] - 1] === "#") {
    const open = match[cursor];
    if (tokens.values[open + 1] === "path" && tokens.values[open + 2] === "=" && tokens.kinds[open + 3] === STR) {
      return tokens.values[open + 3];
    }
    cursor = open - 2;
  }
  return undefined;
}

/**
 * Load every source file of a crate target by following `mod` declarations
 * from its root, as rustc does. Files reached only through a test-only or
 * inactive module inherit that state.
 *
 * @param {string} rootFile absolute path of the crate root (lib.rs, main.rs, ...)
 * @param {ReturnType<typeof productionCfgEnv>} env
 * @param {(file: string) => string} [read]
 * @param {(file: string) => boolean} [exists]
 * @returns {{ files: Map<string, ReturnType<typeof analyzeSource>>, unresolved: string[] }}
 */
export function loadCrate(rootFile, env, read = (file) => readFileSync(file, "utf8"), exists = existsSync) {
  const files = new Map();
  const unresolved = [];
  const rootPath = path.resolve(rootFile);
  // `ownDir` is where child `mod x;` files live; `includeDir` is where
  // `include!` and `#[path]` resolve (the directory of the declaring file).
  const queue = [{ file: rootPath, ownDir: path.dirname(rootPath), testOnly: false, inactive: false }];
  while (queue.length > 0) {
    const entry = queue.shift();
    if (files.has(entry.file)) continue;
    const analysis = analyzeSource(read(entry.file), entry, env);
    files.set(entry.file, analysis);
    const { tokens, match } = analysis;
    const dir = path.dirname(entry.file);
    const ownDir = entry.ownDir;
    const inlineStack = [];
    for (let index = 0; index < tokens.count; index += 1) {
      while (inlineStack.length > 0 && index > inlineStack.at(-1).close) inlineStack.pop();
      if (
        tokens.values[index] === "include"
        && tokens.values[index + 1] === "!"
        && tokens.values[index + 2] === "("
        && tokens.kinds[index + 3] === STR
      ) {
        // Textual inclusion: the file is part of this module.
        const included = path.resolve(dir, tokens.values[index + 3]);
        if (exists(included)) {
          queue.push({
            file: included,
            ownDir,
            testOnly: analysis.testOnly[index] === 1,
            inactive: analysis.inactive[index] === 1,
          });
        } else if (!analysis.testOnly[index]) {
          unresolved.push(`${entry.file}: include!(${tokens.values[index + 3]})`);
        }
        continue;
      }
      if (tokens.values[index] !== "mod" || tokens.kinds[index] !== IDENT || tokens.kinds[index + 1] !== IDENT) continue;
      const name = tokens.values[index + 1];
      const next = tokens.values[index + 2];
      if (next === "{" && match[index + 2] > 0) {
        inlineStack.push({ name, close: match[index + 2] });
        continue;
      }
      if (next !== ";") continue;
      const inlineNames = inlineStack.map((frame) => frame.name);
      const explicit = precedingPathAttribute(tokens, match, index);
      const candidates = explicit
        ? [{ file: path.join(inlineNames.length > 0 ? path.join(ownDir, ...inlineNames) : dir, explicit), modRs: true }]
        : [
            { file: path.join(ownDir, ...inlineNames, `${name}.rs`), modRs: false },
            { file: path.join(ownDir, ...inlineNames, name, "mod.rs"), modRs: true },
          ];
      const found = candidates.find((candidate) => exists(candidate.file));
      if (!found) {
        if (!analysis.testOnly[index] && !analysis.inactive[index]) unresolved.push(`${entry.file}: mod ${name}`);
        continue;
      }
      const childFile = path.resolve(found.file);
      queue.push({
        file: childFile,
        ownDir: found.modRs ? path.dirname(childFile) : path.join(path.dirname(childFile), path.basename(childFile, ".rs")),
        testOnly: analysis.testOnly[index] === 1,
        inactive: analysis.inactive[index] === 1,
      });
    }
  }
  return { files, unresolved };
}

const DEFINITION_KEYWORDS = new Set(["fn", "struct", "enum", "trait", "type", "const", "static", "union", "mod"]);
const IDENT_WORD = /[A-Za-z_][A-Za-z0-9_]*/g;
const FORMAT_ARG = /\{([A-Za-z_][A-Za-z0-9_]*)/g;

/**
 * Every reference to one of `names` in an analysed file: identifier tokens that
 * are not the name in a definition (`fn name`), not inside a `use`
 * declaration, plus names inside string literals that Rust resolves by name
 * (inline format arguments, and any identifier in an attribute string such as
 * `#[serde(default = "path::to::fn")]`).
 *
 * @returns {Array<{ name: string, index: number, testOnly: boolean, inactive: boolean }>}
 */
export function collectReferences(analysis, names) {
  const { tokens, match, testOnly, inactive } = analysis;
  const found = [];
  let useEnd = -1;
  let attributeEnd = -1;
  for (let index = 0; index < tokens.count; index += 1) {
    const kind = tokens.kinds[index];
    const value = tokens.values[index];
    if (kind === PUNCT && value === "#") {
      const open = tokens.values[index + 1] === "!" ? index + 2 : index + 1;
      if (tokens.values[open] === "[" && match[open] > attributeEnd) attributeEnd = match[open];
      continue;
    }
    if (kind === IDENT && value === "use" && index > useEnd) {
      let cursor = index + 1;
      while (cursor < tokens.count && tokens.values[cursor] !== ";") {
        if (tokens.values[cursor] === "{" && match[cursor] > 0) cursor = match[cursor];
        cursor += 1;
      }
      useEnd = cursor;
      continue;
    }
    if (index <= useEnd) continue;
    const flags = { testOnly: testOnly[index] === 1, inactive: inactive[index] === 1 };
    if (kind === IDENT) {
      if (!names.has(value)) continue;
      if (index > 0 && tokens.kinds[index - 1] === IDENT && DEFINITION_KEYWORDS.has(tokens.values[index - 1])) continue;
      found.push({ name: value, index, ...flags });
    } else if (kind === STR) {
      const pattern = index < attributeEnd ? IDENT_WORD : FORMAT_ARG;
      pattern.lastIndex = 0;
      for (let hit = pattern.exec(value); hit; hit = pattern.exec(value)) {
        const name = hit[1] ?? hit[0];
        if (names.has(name)) found.push({ name, index, ...flags });
      }
    }
  }
  return found;
}

const NODE_KEYWORDS = new Set(["fn", "struct", "enum", "trait", "type", "const", "static", "union"]);
/** Tokens after which `impl` starts an item rather than an `impl Trait` type. */
const ITEM_BOUNDARY = new Set(["}", ";", "{", "]", "unsafe", "default"]);
const NAME_FOLLOWERS = {
  fn: new Set(["(", "<"]),
  struct: new Set(["{", "<", "(", ";", "where"]),
  enum: new Set(["{", "<", "where"]),
  union: new Set(["{", "<", "where"]),
  trait: new Set(["{", "<", ":", "where"]),
  type: new Set(["=", "<", ":", ";", "where"]),
  const: new Set([":"]),
  static: new Set([":"]),
};
const FN_QUALIFIERS = new Set(["fn", "unsafe", "async", "extern"]);

function within(ranges, index) {
  return ranges.some(([start, end]) => index >= start && index < end);
}

/**
 * The item structure the name graph needs.
 *
 * - `items`: every fn, struct, enum, trait, type alias, const, static, and
 *   union outside test-only code and outside trait bodies or trait impls, with
 *   its token extent and visibility (`pub`, `restricted` for `pub(..)`, or
 *   `private`).
 * - `implHeaders`: `impl ... {` headers. A type named there (`impl Foo`) is
 *   not a use of it.
 * - `opaque`: bodies of `trait` definitions and `impl Trait for Type`. Their
 *   methods are reached through the trait, not by name, so the names they use
 *   count as used.
 */
export function definedItems(analysis) {
  const { tokens, match, testOnly } = analysis;
  const { kinds, values } = tokens;
  const implHeaders = [];
  const opaque = [];
  for (let index = 0; index < tokens.count; index += 1) {
    if (kinds[index] !== IDENT) continue;
    if (values[index] === "impl" && (index === 0 || ITEM_BOUNDARY.has(values[index - 1]))) {
      let cursor = index + 1;
      let isTraitImpl = false;
      while (cursor < tokens.count && values[cursor] !== "{" && values[cursor] !== ";") {
        if ((values[cursor] === "(" || values[cursor] === "[") && match[cursor] > cursor) cursor = match[cursor];
        else if (values[cursor] === "for" && kinds[cursor] === IDENT && values[cursor + 1] !== "<") isTraitImpl = true;
        cursor += 1;
      }
      implHeaders.push([index, cursor]);
      if (isTraitImpl && values[cursor] === "{" && match[cursor] > cursor) opaque.push([cursor, match[cursor] + 1]);
    } else if (values[index] === "trait" && kinds[index + 1] === IDENT) {
      let cursor = index + 2;
      while (cursor < tokens.count && values[cursor] !== "{" && values[cursor] !== ";") cursor += 1;
      if (values[cursor] === "{" && match[cursor] > cursor) opaque.push([cursor, match[cursor] + 1]);
    }
  }

  const items = [];
  for (let index = 0; index < tokens.count; index += 1) {
    const kind = values[index];
    if (kinds[index] !== IDENT || !NODE_KEYWORDS.has(kind) || kinds[index + 1] !== IDENT || testOnly[index]) continue;
    const name = values[index + 1];
    if (name === "_" || (kind === "const" && FN_QUALIFIERS.has(name))) continue;
    if (!NAME_FOLLOWERS[kind].has(values[index + 2])) continue;
    if ((kind === "const" || kind === "static") && (values[index - 1] === "<" || values[index - 1] === ",")) continue;
    if (within(opaque, index)) continue;
    let back = index - 1;
    while (back >= 0 && (["async", "unsafe", "const", "extern", "default"].includes(values[back]) || kinds[back] === STR)) back -= 1;
    let visibility = "private";
    if (values[back] === "pub") visibility = "pub";
    else if (values[back] === ")" && match[back] > 0 && values[match[back] - 1] === "pub") visibility = "restricted";
    items.push({ name, kind, line: tokens.lines[index + 1], start: index, end: itemEnd(tokens, match, index), visibility });
  }
  return { items, implHeaders, opaque };
}
