import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

/**
 * Run the complete compiler-admission suite before the Vitest worker pool starts.
 * The injectable process/output boundaries let Node contracts exercise failures
 * without rerunning the whole suite or changing its production command.
 * @param {object} [options]
 * @param {typeof spawnSync} [options.spawn]
 * @param {{ write: (chunk: string | Uint8Array) => unknown }} [options.stdout]
 * @param {{ write: (chunk: string | Uint8Array) => unknown }} [options.stderr]
 * @returns {number} A successful child status or a nonzero failure status.
 */
export function runCompilerInputGuardTests({ spawn = spawnSync, stdout = process.stdout, stderr = process.stderr } = {}) {
  const program = process.execPath;
  const args = ['--test', resolve('scripts/compiler-input-guard.test.mjs')];
  const cwd = process.cwd();
  const started = process.hrtime.bigint();
  let result;
  try {
    result = spawn(program, args, { cwd, encoding: 'utf8', timeout: 30_000 });
  } catch (error) {
    result = { error, status: null, signal: null, pid: null, stdout: '', stderr: '' };
  }

  // Preserve captured partial output even when spawnSync reports a timeout.
  if (result.stdout) stdout.write(result.stdout);
  if (result.stderr) stderr.write(result.stderr);
  const error = result.error;
  const errorContext = error === undefined ? null : {
    name: error?.name ?? typeof error,
    message: error?.message ?? String(error),
    ...Object.fromEntries(Object.getOwnPropertyNames(Object(error)).map((key) => [key, Object.getOwnPropertyDescriptor(Object(error), key)?.value])),
  };
  stderr.write(`COMPILER_GUARD_SUITE_RESULT ${JSON.stringify({
    program, args, cwd, encoding: 'utf8', timeout_ms: 30_000,
    elapsed_ms: Number(process.hrtime.bigint() - started) / 1e6,
    pid: result.pid ?? null, status: result.status, signal: result.signal,
    error: errorContext,
    stdout_bytes: Buffer.byteLength(result.stdout ?? '', 'utf8'),
    stderr_bytes: Buffer.byteLength(result.stderr ?? '', 'utf8'),
  })}\n`);

  if (error !== undefined || result.signal != null) return 1;
  if (result.status === 0) return 0;
  return Number.isInteger(result.status) && result.status > 0 && result.status < 256 ? result.status : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.exitCode = runCompilerInputGuardTests();
}
