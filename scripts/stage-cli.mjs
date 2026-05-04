import { copyFileSync, mkdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const isWindows = process.platform === 'win32';
const exe = isWindows ? 'wardian-cli.exe' : 'wardian-cli';
const target = process.env.WARDIAN_CLI_TARGET?.trim();
const buildArgs = ['build', '--release', '-p', 'wardian-cli'];

if (target) {
  buildArgs.push('--target', target);
}

const result = spawnSync('cargo', buildArgs, {
  cwd: root,
  stdio: 'inherit',
  shell: isWindows,
});

if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

const source = target
  ? join(root, 'target', target, 'release', exe)
  : join(root, 'target', 'release', exe);
const destDir = join(root, 'src-tauri', 'resources', 'bin');
mkdirSync(destDir, { recursive: true });
copyFileSync(source, join(destDir, exe));
