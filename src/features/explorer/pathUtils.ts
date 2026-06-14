const isWindowsExplorerHost = (): boolean => {
  if (typeof navigator === 'undefined') {
    return false;
  }

  return /win/i.test(navigator.platform);
};

const stripTrailingSeparators = (path: string): string => {
  if (path === '/' || path === '//' || /^[a-z]:\/$/i.test(path)) {
    return path;
  }

  return path.replace(/\/+$/g, '');
};

const looksLikeWindowsPath = (path: string, isWindowsHost: boolean): boolean => (
  isWindowsHost
  || /^\\\\\?\\/.test(path)
  || /^[a-z]:\\/i.test(path)
  || /^\\\\[^\\]/.test(path)
);

export const normalizeExplorerPathForCompare = (
  path: string,
  isWindowsHost = isWindowsExplorerHost(),
): string => {
  const isWindowsPath = looksLikeWindowsPath(path, isWindowsHost);
  let normalized = isWindowsPath ? path.replace(/\\/g, '/') : path;

  if (isWindowsPath) {
    normalized = normalized
      .replace(/^\/\/\?\/UNC\//i, '//')
      .replace(/^\/\/\?\/([a-z]:\/)/i, '$1');
  }

  normalized = stripTrailingSeparators(normalized);

  return isWindowsPath ? normalized.toLowerCase() : normalized;
};
