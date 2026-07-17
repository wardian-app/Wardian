export const FILE_RESOURCE_PROTOCOL: "wardian-resource";

export function fileResourceUrlConversion(rawUrl: string): {
  path: string;
  protocol: typeof FILE_RESOURCE_PROTOCOL;
} | null;

export function fileResourceUrlForWebview(
  rawUrl: string,
  convertFileSrc: (path: string, protocol: string) => string,
): string;
