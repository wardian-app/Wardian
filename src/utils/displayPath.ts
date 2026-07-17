export const formatExplorerPathForDisplay = (path: string): string => {
  if (/^\\\\\?\\UNC\\/i.test(path)) {
    return `\\\\${path.slice(8)}`;
  }
  if (/^\\\\\?\\(?=[a-z]:[\\/])/i.test(path)) {
    return path.slice(4);
  }
  if (/^\/\/\?\/UNC\//i.test(path)) {
    return `//${path.slice(8)}`;
  }
  if (/^\/\/\?\/(?=[a-z]:[\\/])/i.test(path)) {
    return path.slice(4);
  }
  return path;
};
