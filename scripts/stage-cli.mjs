import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const isWindows = process.platform === 'win32';
const exe = isWindows ? 'wardian-cli.exe' : 'wardian-cli';

function repoRoot() {
  return resolve(fileURLToPath(new URL('..', import.meta.url)));
}

export function resolveCliSourcePath({ targetDirectory, target, profile, exe }) {
  const profileDir = profile === 'release' ? 'release' : 'debug';
  return target
    ? join(targetDirectory, target, profileDir, exe)
    : join(targetDirectory, profileDir, exe);
}

export function resolveDevResourcePath({ targetDirectory, target, exe }) {
  return target
    ? join(targetDirectory, target, 'debug', 'resources', 'bin', exe)
    : join(targetDirectory, 'debug', 'resources', 'bin', exe);
}

function parseProfile(argv) {
  const profileIndex = argv.indexOf('--profile');
  const profile = profileIndex === -1 ? 'release' : argv[profileIndex + 1]?.trim();

  if (profile !== 'release' && profile !== 'dev') {
    throw new Error("Unsupported CLI stage profile. Use '--profile release' or '--profile dev'.");
  }

  return profile;
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
  const root = repoRoot();
  let profile;
  try {
    profile = parseProfile(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    return 1;
  }

  const target = process.env.WARDIAN_CLI_TARGET?.trim();
  const buildArgs = ['build', '-p', 'wardian-cli'];

  if (profile === 'release') {
    buildArgs.push('--release');
  }

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
    profile,
    exe,
  });
  const destDir = join(root, 'src-tauri', 'resources', 'bin');
  mkdirSync(destDir, { recursive: true });
  copyFileSync(source, join(destDir, exe));

  if (profile === 'dev') {
    const devResource = resolveDevResourcePath({
      targetDirectory: cargoTargetDirectory(root),
      target,
      exe,
    });
    mkdirSync(dirname(devResource), { recursive: true });
    copyFileSync(source, devResource);
  }

  return 0;
}

function isDirectRun() {
  try {
    return process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
  } catch {
    return false;
  }
}

if (isDirectRun()) {
  process.exit(main());
}
