import {
  formatExplorerPathForDisplay,
} from './displayPath';

describe('formatExplorerPathForDisplay', () => {
  it('removes a Windows extended drive prefix without changing case or separators', () => {
    expect(formatExplorerPathForDisplay('\\\\?\\C:\\Users\\Test\\Repo\\Notes.md'))
      .toBe('C:\\Users\\Test\\Repo\\Notes.md');
    expect(formatExplorerPathForDisplay('//?/C:/Users/Test/Repo/Notes.md'))
      .toBe('C:/Users/Test/Repo/Notes.md');
  });

  it('converts an extended UNC prefix to an ordinary UNC display path', () => {
    expect(formatExplorerPathForDisplay('\\\\?\\UNC\\SERVER\\Share\\Notes.md'))
      .toBe('\\\\SERVER\\Share\\Notes.md');
    expect(formatExplorerPathForDisplay('//?/UNC/SERVER/Share/Notes.md'))
      .toBe('//SERVER/Share/Notes.md');
  });

  it('leaves ordinary Windows, UNC, POSIX, and relative paths unchanged', () => {
    for (const path of [
      'C:\\Users\\Test\\Notes.md',
      '\\\\SERVER\\Share\\Notes.md',
      '/workspace/Notes.md',
      'docs/Notes.md',
    ]) {
      expect(formatExplorerPathForDisplay(path)).toBe(path);
    }
  });
});
