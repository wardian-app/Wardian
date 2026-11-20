import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { test } from 'node:test';

const root = new URL('../', import.meta.url);
const manifest = readFileSync(new URL('rust-toolchain.toml', root), 'utf8');
const channel = manifest.match(/^channel = "(\d+\.\d+\.\d+)"$/m)?.[1];

test('the repository pins an exact Rust release with lint and format components', () => {
  assert.ok(channel, 'channel must be an exact stable release');
  assert.match(manifest, /^profile = "minimal"$/m);
  assert.match(manifest, /^components = \["clippy", "rustfmt"\]$/m);
});

for (const file of readdirSync(new URL('.github/workflows/', root)).filter((name) => /\.ya?ml$/.test(name))) {
  const workflow = readFileSync(new URL(`.github/workflows/${file}`, root), 'utf8');
  const jobs = workflow.split(/^ {2}[\w-]+:[\t ]*\r?$/m).slice(1);
  for (const [index, job] of jobs.entries()) {
    const installers = [...job.matchAll(/uses: dtolnay\/rust-toolchain@([^\s]+)/g)];
    if (installers.length === 0) continue;
    test(`${file} Rust job ${index + 1} installs and reports the pinned compiler`, () => {
      for (const installer of installers) assert.equal(installer[1], channel);
      assert.match(job, /run: rustc --version\s*$/m);
      assert.doesNotMatch(job, /^\s+toolchain:/m, 'do not override the pinned installer');
    });
  }
}
