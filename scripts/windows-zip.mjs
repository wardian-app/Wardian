import { spawnSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import path from 'node:path';

/** Extract an official ZIP without loading inherited PowerShell modules. */
export function extractWindowsZip(archive, destination) {
  const systemRoot = process.env.SystemRoot || process.env.SYSTEMROOT;
  if (!systemRoot) throw new Error('SystemRoot is required to locate Windows tar.exe');
  mkdirSync(destination, { recursive: true });
  const result = spawnSync(path.join(systemRoot, 'System32', 'tar.exe'),
    ['-xf', archive, '-C', destination], { windowsHide: true, encoding: 'utf8' });
  if (result.error || result.status !== 0) {
    throw new Error(`ZIP extraction failed: ${result.error?.message || result.stderr?.trim() || `tar exited ${result.status}`}`);
  }
}
