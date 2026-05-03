import { copyFileSync, mkdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const isWindows = process.platform === 'win32';
const exe = isWindows ? 'wardian.exe' : 'wardian';

const result = spawnSync('cargo', ['build', '--release', '-p', 'wardian-cli'], {
  cwd: root,
  stdio: 'inherit',
  shell: isWindows,
});

if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

const source = join(root, 'target', 'release', exe);
const destDir = join(root, 'src-tauri', 'resources', 'bin');
mkdirSync(destDir, { recursive: true });
copyFileSync(source, join(destDir, exe));
