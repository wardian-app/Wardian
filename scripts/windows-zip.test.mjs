import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { extractWindowsZip } from './windows-zip.mjs';

// A ZIP containing payload.txt, produced by Windows tar, independent of PowerShell.
const zip = Buffer.from('UEsDBBQACAAIAHggc10AAAAAAAAAAAAAAAALACAAcGF5bG9hZC50eHR1eAsAAQQAAAAABAAAAABVVA0AB3S7/mp0u/5qdLv+akvLrCgpLUrl5QIAUEsHCLsfg6ILAAAACQAAAFBLAQIUAxQACAAIAHggc127H4OiCwAAAAkAAAALABgAAAAAAAAAAAC2gQAAAABwYXlsb2FkLnR4dHV4CwABBAAAAAAEAAAAAFVUBQABdLv+alBLBQYAAAAAAQABAFEAAABkAAAAAAA=', 'base64');

test('Windows ZIP extraction ignores inherited PowerShell modules and PATH and reports invalid archives',
  { skip: process.platform !== 'win32' }, (t) => {
    const base = path.join(process.env.WARDIAN_HOME ?? path.join(homedir(), '.wardian'),
      'agents', process.env.WARDIAN_SESSION_ID ?? 'windows-zip-tests', 'workspace', 'temp');
    mkdirSync(base, { recursive: true });
    const root = mkdtempSync(path.join(base, 'zip with spaces-'));
    t.after(() => {
      assert.equal(path.dirname(realpathSync(root)), realpathSync(base));
      assert.ok(path.basename(root).startsWith('zip with spaces-'));
      rmSync(root, { recursive: true, force: true });
    });
    const archive = path.join(root, "archive's name.zip");
    const destination = path.join(root, "unpacked's directory");
    writeFileSync(archive, zip);
    const script = `import { extractWindowsZip } from ${JSON.stringify(new URL('./windows-zip.mjs', import.meta.url).href)}; extractWindowsZip(process.argv[1], process.argv[2]);`;
    const result = spawnSync(process.execPath, ['--input-type=module', '-e', script, archive, destination], {
      env: { ...process.env, PATH: root, PSModulePath: path.join(root, 'incompatible-modules') },
      encoding: 'utf8', windowsHide: true,
    });
    assert.equal(result.status, 0, result.stderr || result.error?.message);
    assert.equal(readFileSync(path.join(destination, 'payload.txt'), 'utf8'), 'fixture\r\n');
    writeFileSync(archive, 'not a ZIP');
    assert.throws(() => extractWindowsZip(archive, destination), /ZIP extraction failed:/);
  });
