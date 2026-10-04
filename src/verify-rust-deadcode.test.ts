import { mkdtempSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  analyzeSource,
  collectReferences,
  lexRust,
  definedItems,
  loadCrate,
  productionCfgEnv,
} from '../scripts/lib/rust-source.mjs';
import {
  compareWithBaseline,
  deadCodeFindings,
  pathInsideCopy,
  prepareCopyRoot,
  readBaseline,
  registeredCommands,
  rewriteManifestPaths,
  rewriteVisibility,
  splitCfgGated,
  stripJsComments,
  unreachableItems,
} from '../scripts/verify-rust-deadcode.mjs';

const WINDOWS = productionCfgEnv('win32');
const LINUX = productionCfgEnv('linux');
const analyze = (source: string, env = WINDOWS) => analyzeSource(source, { testOnly: false, inactive: false }, env);

/** The flags of the identifier `name` at its `occurrence`-th appearance. */
function flagsOf(analysis: ReturnType<typeof analyze>, name: string, occurrence = 0) {
  const indexes = analysis.tokens.values.flatMap((value, index) => (value === name ? [index] : []));
  const index = indexes[occurrence];
  return { testOnly: analysis.testOnly[index] === 1, inactive: analysis.inactive[index] === 1 };
}

describe('Rust source index', () => {
  it('drops comments and keeps string bodies as single tokens', () => {
    const tokens = lexRust([
      '/// calls `ghost` in prose',
      '/* nested /* ghost */ still comment */',
      'let url = "http://x//ghost"; let raw = r#"ghost "quoted""#;',
      "let c = '\"'; fn f<'a>(x: &'a str) { real(x) }",
    ].join('\n'));
    const identifiers = tokens.values.filter((_, index) => tokens.kinds[index] === 1);
    expect(identifiers).not.toContain('ghost');
    expect(identifiers).toContain('real');
    expect(tokens.values).toContain('ghost "quoted"');
  });

  it('marks cfg(test), #[test], and other-platform regions', () => {
    const analysis = analyze([
      'fn prod() { shared() }',
      '#[cfg(test)] mod tests { fn t() { shared() } }',
      '#[tokio::test(flavor = "multi_thread")] async fn async_case() { shared() }',
      '#[cfg(unix)] fn unix_only() { shared() }',
      '#[cfg(all(test, windows))] fn windows_test() { shared() }',
      '#[cfg(not(test))] fn not_test() { shared() }',
    ].join('\n'));
    expect(flagsOf(analysis, 'shared', 0)).toEqual({ testOnly: false, inactive: false });
    expect(flagsOf(analysis, 'shared', 1)).toEqual({ testOnly: true, inactive: true });
    expect(flagsOf(analysis, 'shared', 2)).toEqual({ testOnly: true, inactive: true });
    expect(flagsOf(analysis, 'shared', 3)).toEqual({ testOnly: false, inactive: true });
    expect(flagsOf(analysis, 'shared', 4)).toEqual({ testOnly: true, inactive: true });
    expect(flagsOf(analysis, 'shared', 5)).toEqual({ testOnly: false, inactive: false });
    expect(flagsOf(analyze('#[cfg(unix)] fn u() { shared() }', LINUX), 'shared')).toEqual({ testOnly: false, inactive: false });
  });

  it('ends a cfg field region at its comma, not at the end of the struct', () => {
    const analysis = analyze('struct S { #[cfg(test)] hook: Hook, live: Live }');
    expect(flagsOf(analysis, 'Hook').testOnly).toBe(true);
    expect(flagsOf(analysis, 'Live').testOnly).toBe(false);
  });

  it('counts references but not definitions, use declarations, or prose', () => {
    const analysis = analyze([
      'use crate::db::{target, other};',
      'pub fn target() {}',
      '#[serde(default = "crate::defaults::target")] struct S;',
      'fn caller() { println!("{target}"); let _ = "target"; }',
    ].join('\n'));
    const references = collectReferences(analysis, new Set(['target']));
    // One from the serde attribute string, one from the inline format argument.
    expect(references).toHaveLength(2);
  });

  it('lists items with their visibility, skipping test-only code and trait bodies', () => {
    const { items } = definedItems(analyze([
      'pub fn a() {}',
      'pub(crate) fn b() {}',
      'pub const fn c() {}',
      'pub const D: u8 = 1;',
      'struct E<const N: usize>;',
      'impl E<1> { pub async fn f(&self) {} }',
      'impl Display for E<1> { fn fmt(&self) {} }',
      '#[cfg(test)] pub fn g() {}',
      'fn takes(x: impl Fn(u8) -> u8) {}',
    ].join('\n')));
    expect(items.map((item) => `${item.visibility} ${item.kind} ${item.name}`)).toEqual([
      'pub fn a', 'restricted fn b', 'pub fn c', 'pub const D', 'private struct E', 'pub fn f', 'private fn takes',
    ]);
  });

  it('treats unknown cfg predicates as unknown, not compiled out', () => {
    const analysis = analyze('#[cfg(feature = "never-enabled")] fn caller() { orphan() }');
    expect(flagsOf(analysis, 'orphan')).toEqual({ testOnly: false, inactive: false });
  });

  it('follows mod, #[path], and include! the way rustc resolves files', () => {
    const root = path.resolve('/ws/src/lib.rs');
    const files: Record<string, string> = {
      [root]: 'mod a;\n#[path = "custom/b_impl.rs"] mod b;\n#[cfg(test)] mod tests;\ninclude!("inc.rs");',
      [path.resolve('/ws/src/a.rs')]: 'mod child;',
      [path.resolve('/ws/src/a/child.rs')]: 'pub fn from_child() {}',
      [path.resolve('/ws/src/custom/b_impl.rs')]: 'mod nested;',
      [path.resolve('/ws/src/custom/nested.rs')]: 'pub fn nested() {}',
      [path.resolve('/ws/src/tests.rs')]: 'fn helper() {}',
      [path.resolve('/ws/src/inc.rs')]: 'pub fn included() {}',
    };
    const crate = loadCrate(root, WINDOWS, (file: string) => {
      const text = files[path.resolve(file)];
      if (text === undefined) throw new Error(`unexpected read: ${file}`);
      return text;
    }, (file: string) => files[path.resolve(file)] !== undefined);
    expect(crate.unresolved).toEqual([]);
    expect([...crate.files.keys()].map((file) => path.relative(path.resolve('/ws/src'), file).split(path.sep).join('/')).sort())
      .toEqual(['a.rs', 'a/child.rs', 'custom/b_impl.rs', 'custom/nested.rs', 'inc.rs', 'lib.rs', 'tests.rs']);
    expect(crate.files.get(path.resolve('/ws/src/tests.rs'))?.testOnly[0]).toBe(1);
    expect(crate.files.get(path.resolve('/ws/src/a/child.rs'))?.testOnly[0]).toBe(0);
  });
});

describe('Rust dead-code gate', () => {
  it('rewrites pub items to crate visibility and keeps named entry points public', () => {
    const source = 'pub fn run() {}\npub async fn other() {}\npub struct S { pub field: u8 }\npub(super) fn kept() {}\npub use a::b;';
    expect(rewriteVisibility(source, ['run'])).toBe(
      'pub fn run() {}\npub(crate) async fn other() {}\npub(crate) struct S { pub field: u8 }\npub(super) fn kept() {}\npub(crate) use a::b;',
    );
  });

  it('points manifest paths outside the copied tree back at the checkout', () => {
    const manifestDir = path.resolve('/ws');
    const copied = path.join(manifestDir, 'src-tauri');
    const text = 'portable-pty = { path = "vendor/portable-pty" }\napp = { path = "src-tauri" }';
    const rewritten = rewriteManifestPaths(text, manifestDir, (resolved: string) => resolved.startsWith(copied));
    expect(rewritten).toContain(`path = "${path.join(manifestDir, 'vendor', 'portable-pty').split(path.sep).join('/')}"`);
    expect(rewritten).toContain('path = "src-tauri"');
  });

  it('turns a rustc dead_code diagnostic into one finding per item with its parent type', () => {
    const findings = deadCodeFindings({
      message: 'methods `open` and `close` are never used',
      code: { code: 'dead_code' },
      spans: [
        { is_primary: false, label: 'methods in this implementation', file_name: 'src-tauri/src/a.rs', line_start: 3, text: [{ text: 'impl<T> Store<T> {', highlight_start: 1, highlight_end: 17 }] },
        { is_primary: true, label: null, file_name: 'src-tauri/src/a.rs', line_start: 4, text: [{ text: '    pub(crate) fn open(&self) {}', highlight_start: 19, highlight_end: 23 }] },
        { is_primary: true, label: null, file_name: 'src-tauri/src/a.rs', line_start: 9, text: [{ text: '    pub(crate) fn close(&self) {}', highlight_start: 19, highlight_end: 24 }] },
      ],
    }, (file: string) => file);
    expect(findings.map((finding) => [finding.key, finding.kind, finding.line])).toEqual([
      ['src-tauri/src/a.rs::Store.open', 'method', 4],
      ['src-tauri/src/a.rs::Store.close', 'method', 9],
    ]);
    expect(deadCodeFindings({ message: 'unused import', code: { code: 'unused_imports' }, spans: [] }, (file: string) => file)).toEqual([]);
  });

  it('sets aside a finding only when compiled-out code names it and its type', () => {
    const analysis = analyze([
      '#[cfg(unix)] fn unix_caller(store: &Store) { store.open(); free_helper(); }',
      '#[cfg(unix)] fn unrelated(other: &Other) { other.close(); }',
    ].join('\n'));
    const finding = (key: string, name: string, parent?: string) => ({ key, file: 'a.rs', line: 1, name, parent, kind: 'method', source: 'rustc' });
    const { kept, gated } = splitCfgGated([
      finding('a.rs::Store.open', 'open', 'Store'),
      finding('a.rs::Store.close', 'close', 'Store'),
      finding('a.rs::free_helper', 'free_helper'),
    ], [analysis]);
    expect(gated.map((item) => item.key)).toEqual(['a.rs::Store.open', 'a.rs::free_helper']);
    expect(kept.map((item) => item.key)).toEqual(['a.rs::Store.close']);
  });

  it('finds library items that production cannot reach, including chains and cycles', () => {
    const library = new Map([[path.resolve('/ws/core/lib.rs'), analyze([
      'pub fn used() { helper() }',
      'fn helper() {}',
      'pub fn orphan() {}',
      'fn dead_private_caller() { orphan() }',
      'pub fn ping() { pong() }',
      'pub fn pong() { ping() }',
      'pub struct Shown;',
      'impl Shown { pub fn only_self() -> Shown { Shown } }',
      'impl std::fmt::Display for Shown { fn fmt(&self) { via_trait() } }',
      'fn via_trait() {}',
      'pub fn test_only() {}',
      '#[cfg(test)] mod tests { fn t() { super::test_only() } }',
    ].join('\n'))]]);
    const consumer = new Map([[path.resolve('/ws/app/main.rs'), analyze('fn main() { core::used(); }')]]);
    const dead = unreachableItems(library, new Map([...library, ...consumer]));
    expect(dead.map((item) => item.name).sort()).toEqual([
      'Shown', 'dead_private_caller', 'only_self', 'orphan', 'ping', 'pong', 'test_only',
    ]);
  });

  it('tells braced const-generic arguments from item bodies', () => {
    const root = path.resolve('/ws/core/lib.rs');
    const consumer = new Map([[path.resolve('/ws/app/main.rs'), analyze('fn main() { core::S::<1>; }')]]);
    const library = new Map([[root, analyze([
      'pub struct S<const N: usize>;',
      'impl std::fmt::Display for S<{ 1 }> { fn fmt(&self) { helper() } }',
      'fn helper() {}',
      'pub struct Dead<const N: usize = { 1 }> { field: Field }',
      'pub struct Field;',
    ].join('\n'))]]);
    const dead = unreachableItems(library, new Map([...library, ...consumer]));
    expect(dead.map((item) => item.name).sort()).toEqual(['Dead', 'Field']);
  });

  it('refuses a linked file inside the copy', () => {
    const root = mkdtempSync(path.join(os.tmpdir(), 'rust-deadcode-copy-'));
    const outside = mkdtempSync(path.join(os.tmpdir(), 'rust-deadcode-outside-'));
    try {
      writeFileSync(path.join(outside, 'Cargo.toml'), 'outside');
      try {
        symlinkSync(path.join(outside, 'Cargo.toml'), path.join(root, 'Cargo.toml'), 'file');
      } catch {
        return; // Creating file links needs a privilege some Windows hosts lack.
      }
      expect(pathInsideCopy(root, 'Cargo.toml')).toBeUndefined();
      expect(pathInsideCopy(root, 'src/lib.rs')).toBe(path.join(root, 'src', 'lib.rs'));
    } finally {
      rmSync(root, { recursive: true, force: true });
      rmSync(outside, { recursive: true, force: true });
    }
  });

  it('refuses a copy root that is a link or junction to another directory', () => {
    const targetDir = mkdtempSync(path.join(os.tmpdir(), 'rust-deadcode-target-'));
    const checkout = mkdtempSync(path.join(os.tmpdir(), 'rust-deadcode-checkout-'));
    try {
      expect(prepareCopyRoot(targetDir, 'plain')).toBe(path.join(path.resolve(targetDir), 'rust-deadcode', 'plain'));
      symlinkSync(checkout, path.join(targetDir, 'rust-deadcode', 'linked'), 'junction');
      expect(() => prepareCopyRoot(targetDir, 'linked')).toThrow('refusing to use');

      // A linked parent is refused before its absent child is created in it.
      const otherTarget = mkdtempSync(path.join(os.tmpdir(), 'rust-deadcode-target-'));
      try {
        symlinkSync(checkout, path.join(otherTarget, 'rust-deadcode'), 'junction');
        expect(() => prepareCopyRoot(otherTarget, 'absent')).toThrow('refusing to use');
        expect(readdirSync(checkout)).toEqual([]);
      } finally {
        rmSync(otherTarget, { recursive: true, force: true });
      }
    } finally {
      rmSync(targetDir, { recursive: true, force: true });
      rmSync(checkout, { recursive: true, force: true });
    }
  });

  it('keeps every copy write and delete inside the copy', () => {
    const root = path.resolve('target-test-copy-root');
    expect(pathInsideCopy(root, 'src-tauri/src/lib.rs')).toBe(path.join(root, 'src-tauri', 'src', 'lib.rs'));
    expect(pathInsideCopy(root, '../../../Cargo.toml')).toBeUndefined();
    expect(pathInsideCopy(root, path.resolve('/elsewhere/file.rs'))).toBeUndefined();
    expect(pathInsideCopy(root, 'C:/elsewhere/file.rs')).toBeUndefined();
    expect(pathInsideCopy(root, '')).toBeUndefined();
  });

  it('reads tauri::generate_handler! entries, skipping cfg attributes', () => {
    expect(registeredCommands([
      '.invoke_handler(tauri::generate_handler![',
      '  commands::a::first,',
      '  #[cfg(debug_assertions)]',
      '  commands::debug::debug_second,',
      '  third',
      '])',
    ].join('\n'))).toEqual(['first', 'debug_second', 'third']);
  });

  it('does not count a command name in a comment as an invocation', () => {
    const stripped = stripJsComments([
      '// invoke("line_comment_command") was removed',
      '/* invoke("block_comment_command")',
      '   spans lines */ invoke("live_command");',
      'const url = "https://example.test/path"; // trailing "after_url_command"',
      'const tpl = `/* kept */ ${"template_command"}`;',
    ].join('\n'));
    const names = [...stripped.matchAll(/['"`]([a-z_][a-z0-9_]*)['"`]/g)].map((hit) => hit[1]);
    expect(names).toEqual(['live_command', 'template_command']);
    expect(stripped).toContain('https://example.test/path');
    expect(stripped.split('\n')).toHaveLength(5);
  });

  it('keeps regex literals intact so a quote inside one cannot hide a comment', () => {
    const names = (source: string) =>
      [...stripJsComments(source).matchAll(/['"`]([a-z_][a-z0-9_]*)['"`]/g)].map((hit) => hit[1]);
    expect(names('const quote = /"/; // invoke("dead_command") was removed')).toEqual([]);
    expect(names('const slash = /[/"]/g; /* invoke("block_dead") */')).toEqual([]);
    expect(names('if (x) return /\'/.test(s); // "after_return"')).toEqual([]);
    expect(names('const half = total / 2; invoke("live_after_division"); // "gone"')).toEqual([
      'live_after_division',
    ]);
    expect(names('const ratio = (a) / (b); invoke("live_after_paren");')).toEqual(['live_after_paren']);
  });

  it('follows import aliases to the item they name', () => {
    const library = new Map([[path.resolve('/ws/core/lib.rs'), analyze([
      'pub fn perform_cleanup() {}',
      'pub fn imported_never_called() {}',
      'pub fn test_alias_target() {}',
      'use crate::inner_target as local;',
      'pub fn entry() { local() }',
      'fn inner_target() {}',
    ].join('\n'))]]);
    const consumer = new Map([[path.resolve('/ws/app/main.rs'), analyze([
      'use core::perform_cleanup as cleanup;',
      'use core::imported_never_called as never_called;',
      'fn main() { cleanup(); core::entry(); }',
      '#[cfg(test)] mod tests { use core::test_alias_target as t; fn x() { t() } }',
    ].join('\n'))]]);
    const dead = unreachableItems(library, new Map([...library, ...consumer]));
    expect(dead.map((item) => item.name).sort()).toEqual(['imported_never_called', 'test_alias_target']);
  });

  it('fails new findings and stale entries, but not entries this platform cannot observe', () => {
    const baseline = readBaseline(JSON.stringify({
      groups: [{ reason: 'Held only for Drop.', entries: ['a.rs::kept', 'a.rs::gone', 'win.rs::windows_only'] }],
    }));
    const findings = [
      { key: 'a.rs::kept', file: 'a.rs', line: 1, name: 'kept', kind: 'fn', source: 'rustc' },
      { key: 'b.rs::new_item', file: 'b.rs', line: 2, name: 'new_item', kind: 'fn', source: 'rustc' },
    ];
    const result = compareWithBaseline(findings, baseline, (entry: string) => !entry.startsWith('win.rs'));
    expect(result.added.map((finding) => finding.key)).toEqual(['b.rs::new_item']);
    expect(result.stale).toEqual(['a.rs::gone']);
  });

  it('rejects a baseline group without a reason or with a duplicate entry', () => {
    expect(() => readBaseline(JSON.stringify({ groups: [{ reason: '', entries: ['x'] }] }))).toThrow('needs a reason');
    expect(() => readBaseline(JSON.stringify({
      groups: [{ reason: 'First stated reason.', entries: ['x'] }, { reason: 'Second stated reason.', entries: ['x'] }],
    }))).toThrow('duplicate baseline entry');
  });
});
