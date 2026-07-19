import {
  formatExplorerPathForDisplay,
} from './displayPath';

describe('formatExplorerPathForDisplay', () => {
  it('removes a Windows extended drive prefix and uses Windows display separators', () => {
    expect(formatExplorerPathForDisplay('\\\\?\\C:\\Users\\Test\\Repo\\Notes.md'))
      .toBe('C:\\Users\\Test\\Repo\\Notes.md');
    expect(formatExplorerPathForDisplay('//?/C:/Users/Test/Repo/Notes.md'))
      .toBe('C:\\Users\\Test\\Repo\\Notes.md');
  });

  it('converts extended and slash-style UNC paths to ordinary Windows display paths', () => {
    expect(formatExplorerPathForDisplay('\\\\?\\UNC\\SERVER\\Share\\Notes.md'))
      .toBe('\\\\SERVER\\Share\\Notes.md');
    expect(formatExplorerPathForDisplay('//?/UNC/SERVER/Share/Notes.md'))
      .toBe('\\\\SERVER\\Share\\Notes.md');
    expect(formatExplorerPathForDisplay('//SERVER/Share/Notes.md'))
      .toBe('\\\\SERVER\\Share\\Notes.md');
  });

  it('normalizes ordinary Windows drive separators while preserving POSIX and relative paths', () => {
    expect(formatExplorerPathForDisplay('C:/Users/Test/Notes.md'))
      .toBe('C:\\Users\\Test\\Notes.md');
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
