export const FILE_RESOURCE_PROTOCOL = "wardian-resource";

/**
 * Parse a backend renderer ticket URL into the arguments expected by Tauri's
 * `convertFileSrc`. Keeping this parser runtime-neutral lets native E2E prove
 * the same production path without duplicating URL/percent-decoding rules.
 *
 * @param {string} rawUrl
 * @returns {{ path: string, protocol: string } | null}
 */
export function fileResourceUrlConversion(rawUrl) {
  let parsed;
  try {
    parsed = new URL(rawUrl);
  } catch {
    return null;
  }
  if (parsed.protocol !== `${FILE_RESOURCE_PROTOCOL}:` || parsed.hostname !== "localhost") {
    return null;
  }
  let ticketPath;
  try {
    ticketPath = decodeURIComponent(parsed.pathname).replace(/^\/+/, "");
  } catch {
    return null;
  }
  if (!ticketPath) return null;
  return { path: ticketPath, protocol: FILE_RESOURCE_PROTOCOL };
}

/**
 * Convert only Wardian renderer tickets. HTTP(S), blob, data, and test URLs
 * remain untouched so callers never double-convert an already usable URL.
 *
 * @param {string} rawUrl
 * @param {(path: string, protocol: string) => string} convertFileSrc
 */
export function fileResourceUrlForWebview(rawUrl, convertFileSrc) {
  const conversion = fileResourceUrlConversion(rawUrl);
  return conversion
    ? convertFileSrc(conversion.path, conversion.protocol)
    : rawUrl;
}
