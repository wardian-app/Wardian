import { copyFileSync, mkdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const isWindows = process.platform === 'win32';
const exe = isWindows ? 'wardian-cli.exe' : 'wardian-cli';

export function resolveCliSourcePath({ targetDirectory, target, exe }) {
  return target
    ? join(targetDirectory, target, 'release', exe)
    : join(targetDirectory, 'release', exe);
}

function cargoTargetDirectory(root) {
  const result = spawnSync('cargo', ['metadata', '--format-version=1', '--no-deps'], {
    cwd: root,
    encoding: 'utf8',
    shell: isWindows,
  });

  if (result.status !== 0) {
    return join(root, 'target');
  }

  try {
    return JSON.parse(result.stdout).target_directory || join(root, 'target');
  } catch {
    return join(root, 'target');
  }
}

export function main() {
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
    return result.status ?? 1;
  }

  const source = resolveCliSourcePath({
    targetDirectory: cargoTargetDirectory(root),
    target,
    exe,
  });
  const destDir = join(root, 'src-tauri', 'resources', 'bin');
  mkdirSync(destDir, { recursive: true });
  copyFileSync(source, join(destDir, exe));
  return 0;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exit(main());
}
