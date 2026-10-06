import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { cacheLayout, main as cacheMain } from './rust-build-cache.mjs';

const VERSION = '0.18.0';

/** Validate an official checksum sidecar before extracting an executable. */
export function verifyDownload(bytes, checksum) {
  const expected = checksum.trim().split(/\s+/)[0];
  const actual = createHash('sha256').update(bytes).digest('hex');
  if (!/^[a-fA-F0-9]{64}$/.test(expected) || actual !== expected.toLowerCase()) throw new Error('Official sccache archive checksum mismatch');
  return actual;
}

async function download(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`Official sccache download failed: HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

export async function main(argv = process.argv.slice(2)) {
  if (argv.length === 0) return cacheMain(['setup']);
  if (argv.join(' ') !== '--install') throw new Error('Use rust:cache:setup [--install]');
  if (process.platform !== 'win32' || process.arch !== 'x64') throw new Error('Automatic official ZIP setup supports Windows x64; use the documented pinned release on other hosts');
  const layout = cacheLayout();
  const tools = path.join(layout.root, 'tools');
  // Do not write through cache-root junctions, or replace any existing installation.
  const expectedRoot = path.resolve(layout.root);
  const { canonicalPath, readProtectedInputs } = await import('./compiler-input-guard.mjs');
  if (canonicalPath(expectedRoot) !== expectedRoot.toLowerCase()) throw new Error('Linked cache root refused');
  for (const input of readProtectedInputs() ?? []) {
    const relative = path.relative(canonicalPath(expectedRoot), input);
    if (relative === '' || (!relative.startsWith('..') && !path.isAbsolute(relative))) throw new Error('Installer root overlaps protected input');
  }
  mkdirSync(tools, { recursive: true });
  const attempt = mkdtempSync(path.join(tools, `sccache-${VERSION}-`));
  const asset = `sccache-v${VERSION}-x86_64-pc-windows-msvc.zip`;
  const release = `https://github.com/mozilla/sccache/releases/download/v${VERSION}/`;
  const [bytes, checksum] = await Promise.all([download(release + asset), download(release + asset + '.sha256')]);
  const archiveHash = verifyDownload(bytes, checksum.toString('utf8'));
  const archive = path.join(attempt, asset);
  writeFileSync(archive, bytes);
  const extracted = path.join(attempt, 'unpacked');
  const script = 'Expand-Archive -LiteralPath $env:WARDIAN_SCCACHE_ARCHIVE -DestinationPath $env:WARDIAN_SCCACHE_UNPACK';
  const result = spawnSync('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script], {
    env: { ...process.env, WARDIAN_SCCACHE_ARCHIVE: archive, WARDIAN_SCCACHE_UNPACK: extracted }, windowsHide: true, encoding: 'utf8' });
  if (result.error || result.status !== 0) throw new Error('Official ZIP extraction failed; attempt retained for inspection');
  const folder = readdirSync(extracted).find((name) => name === asset.slice(0, -4));
  const executable = folder && path.join(extracted, folder, 'sccache.exe');
  if (!executable || !existsSync(executable)) throw new Error('Official archive has unexpected executable layout');
  const installed = path.join(attempt, 'sccache.exe');
  copyFileSync(executable, installed);
  writeFileSync(path.join(attempt, 'provenance.json'), JSON.stringify({ version: VERSION, url: release + asset, archive_sha256: archiveHash,
    executable_sha256: createHash('sha256').update(readFileSync(installed)).digest('hex') }, null, 2));
  const previous = process.env.WARDIAN_SCCACHE;
  process.env.WARDIAN_SCCACHE = installed;
  try { return cacheMain(['setup']); } finally {
    if (previous === undefined) delete process.env.WARDIAN_SCCACHE;
    else process.env.WARDIAN_SCCACHE = previous;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try { process.exitCode = await main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
