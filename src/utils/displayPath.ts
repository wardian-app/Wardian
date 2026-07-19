export const formatExplorerPathForDisplay = (path: string): string => {
  let displayPath = path;
  if (/^\\\\\?\\UNC\\/i.test(path)) {
    displayPath = `\\\\${path.slice(8)}`;
  } else if (/^\\\\\?\\(?=[a-z]:[\\/])/i.test(path)) {
    displayPath = path.slice(4);
  } else if (/^\/\/\?\/UNC\//i.test(path)) {
    displayPath = `\\\\${path.slice(8)}`;
  } else if (/^\/\/\?\/(?=[a-z]:[\\/])/i.test(path)) {
    displayPath = path.slice(4);
  }

  const windowsPath = /^[a-z]:[\\/]/i.test(displayPath)
    || /^(?:\\\\|\/\/)[^\\/]+[\\/][^\\/]+(?:[\\/]|$)/.test(displayPath);
  return windowsPath ? displayPath.replace(/\//g, '\\') : displayPath;
};
