import { describe, expect, it } from 'vitest';
import { normalizeExplorerPathForCompare } from './pathUtils';

describe('normalizeExplorerPathForCompare', () => {
  it('normalizes Windows verbatim drive paths reported with backslashes', () => {
    expect(normalizeExplorerPathForCompare('\\\\?\\C:\\Users\\Test\\Repo', false)).toBe('c:/users/test/repo');
  });

  it('normalizes Windows verbatim UNC paths reported with backslashes', () => {
    expect(normalizeExplorerPathForCompare('\\\\?\\UNC\\SERVER\\share\\Repo', false)).toBe('//server/share/repo');
  });

  it('compares drive-letter paths case-insensitively on Windows hosts', () => {
    expect(normalizeExplorerPathForCompare('C:/Users/Test/Repo', true)).toBe('c:/users/test/repo');
  });

  it('preserves POSIX root and significant trailing spaces', () => {
    expect(normalizeExplorerPathForCompare('/', false)).toBe('/');
    expect(normalizeExplorerPathForCompare('/tmp/name ', false)).toBe('/tmp/name ');
  });

  it('preserves non-Windows paths that resemble verbatim or drive-letter paths', () => {
    expect(normalizeExplorerPathForCompare('//server/share', false)).toBe('//server/share');
    expect(normalizeExplorerPathForCompare('//?/tmp/project', false)).toBe('//?/tmp/project');
    expect(normalizeExplorerPathForCompare('//?/UNC/server/share', false)).toBe('//?/UNC/server/share');
    expect(normalizeExplorerPathForCompare('C:/Case/Sensitive', false)).toBe('C:/Case/Sensitive');
  });
});
