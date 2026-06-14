export const normalizeExplorerPathForCompare = (path: string): string => {
  const normalized = path
    .trim()
    .replace(/\\/g, '/')
    .replace(/^\/\/\?\/UNC\//i, '//')
    .replace(/^\/\/\?\//, '')
    .replace(/\/+$/g, '');

  return /^[a-z]:\//i.test(normalized) ? normalized.toLowerCase() : normalized;
};
