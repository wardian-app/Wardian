import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { assertVerificationAdmission } from './compiler-input-guard.mjs';
import { cargoInvocation, withRustCache } from './rust-build-cache.mjs';

const WORKFLOW_PATH = resolve('.github/workflows/ci.yml');
const CATEGORIES = new Set(['frontend', 'backend', 'docs']);

/**
 * Read the local-verification contract directly from the CI workflow.
 * Only single-line run steps are eligible; this keeps the command boundary
 * literal and avoids trying to interpret shell blocks as YAML or JavaScript.
 */
export function readVerificationPlan(workflowText) {
  const lines = workflowText.split(/\r?\n/);
  const plan = [];
  for (let index = 0; index < lines.length; index += 1) {
    const marker = lines[index].match(/^([ \t]*)#\s*local-verify:\s*(\S+)\s*$/);
    if (!marker) continue;

    const runLine = lines[index + 1]?.match(/^([ \t]*)run:[ \t]*(\S.*)$/);
    const command = runLine?.[2].trim();
    // YAML block scalars start with | or >, followed by optional chomping,
    // indentation, and comments. None is a literal single-line command.
    if (!runLine || runLine[1] !== marker[1] || /^[|>]/.test(command)) {
      throw new Error(`local-verify marker at workflow line ${index + 1} must precede a single-line run step`);
    }
    const category = marker[2];
    if (!CATEGORIES.has(category)) throw new Error(`unknown local-verify category: ${category}`);
    plan.push({ category, command, workflowLine: index + 2 });
    index += 1;
  }
  if (plan.length === 0) throw new Error('CI workflow contains no local-verify steps');
  return plan;
}

/** Reject invalid or repeated category options before reading or executing CI. */
export function parseArgs(argv) {
  let only = null;
  let list = false;
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === '--list') {
      list = true;
    } else if (argument === '--only' || argument.startsWith('--only=')) {
      if (only !== null) throw new Error('--only may only be specified once');
      const category = argument === '--only' ? argv[++index] : argument.slice('--only='.length);
      if (!CATEGORIES.has(category)) throw new Error(`--only must be one of: ${[...CATEGORIES].join(', ')}`);
      only = category;
    } else {
      throw new Error(`unknown argument: ${argument}`);
    }
  }
  return { list, only };
}

export function selectPlan(plan, only) {
  return only ? plan.filter((step) => step.category === only) : plan;
}

function execute(command) {
  let result;
  if (command.startsWith('cargo ') && process.env.WARDIAN_RUST_CACHE_TARGET) {
    const invocation = cargoInvocation(command.slice(6).split(/\s+/));
    result = spawnSync(invocation.program, invocation.args, { cwd: invocation.cwd, env: invocation.env, stdio: 'inherit', windowsHide: true });
  } else {
    assertVerificationAdmission(command, { cwd: process.cwd(), env: process.env });
    result = spawnSync(command, { cwd: process.cwd(), shell: true, stdio: 'inherit', windowsHide: true });
  }
  if (result.error) throw result.error;
  if (result.status !== 0) {
    console.error(`FAILED: ${command}`);
    process.exitCode = result.status ?? 1;
    return false;
  }
  return true;
}

export function main(argv = process.argv.slice(2)) {
  const { list, only } = parseArgs(argv);
  const plan = selectPlan(readVerificationPlan(readFileSync(WORKFLOW_PATH, 'utf8')), only);
  if (plan.length === 0) throw new Error(`no local-verify steps selected for ${only}`);
  if (list) {
    for (const step of plan) console.log(`${step.category}\t${step.command}`);
    return 0;
  }
  const run = () => {
    for (const step of plan) {
      console.log(`\n>>> ${step.command}`);
      if (!execute(step.command)) return process.exitCode;
    }
    return 0;
  };
  return plan.some((step) => step.category === 'backend') ? withRustCache(run) : run();
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    process.exitCode = main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
