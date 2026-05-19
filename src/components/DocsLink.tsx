import type { PropsWithChildren } from "react";
import { ExternalLink } from "lucide-react";

export const DOCS_BASE_URL = "https://docs.wardian.org";

export function docsUrl(path: string): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  return `${DOCS_BASE_URL}${normalizedPath}`;
}

interface DocsLinkProps extends PropsWithChildren {
  path: string;
  className?: string;
  title?: string;
  "data-testid"?: string;
}

export function DocsLink({
  path,
  className = "inline-flex items-center gap-1 rounded-md text-[11px] font-semibold text-muted-neutral hover:text-[var(--color-wardian-accent)] transition-colors",
  title,
  "data-testid": testId,
  children,
}: DocsLinkProps) {
  return (
    <a
      data-testid={testId}
      href={docsUrl(path)}
      target="_blank"
      rel="noreferrer"
      title={title}
      className={className}
    >
      <span>{children}</span>
      <ExternalLink className="h-3 w-3" aria-hidden="true" />
    </a>
  );
}
