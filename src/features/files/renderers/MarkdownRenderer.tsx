import {
  createElement,
  isValidElement,
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { markdownUrlTransform, safeMarkdownUrl } from "../../grid/markdown/markdownSafety";
import type { FileRendererProps } from "../rendererRegistry";

const MARKDOWN_MAX_SIZE_BYTES = 16 * 1024 * 1024;
const MARKDOWN_MAX_LINE_COUNT = 200_000;

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function canRenderMarkdown(descriptor: FileRendererProps["snapshot"]["descriptor"]) {
  return (descriptor.renderer_kind === "markdown" || descriptor.mime_type.trim().toLowerCase() === "text/markdown")
    && descriptor.encoding === "utf-8"
    && descriptor.capabilities.preview
    && descriptor.unavailable_reason === null
    && descriptor.size_bytes <= MARKDOWN_MAX_SIZE_BYTES
    && descriptor.line_count !== null
    && descriptor.line_count <= MARKDOWN_MAX_LINE_COUNT;
}

function isLocalTarget(raw: string) {
  return raw.startsWith("file:")
    || /^[A-Za-z]:[\\/]/.test(raw)
    || raw.startsWith("/")
    || raw.startsWith("./")
    || raw.startsWith("../")
    || (!/^[A-Za-z][A-Za-z0-9+.-]*:/.test(raw) && !raw.startsWith("#"));
}

function windowsPathRoot(path: string): string | null {
  const verbatimUnc = path.match(/^\/\/\?\/UNC\/([^/]+)\/([^/]+)(?:\/|$)/i);
  if (verbatimUnc) return `//?/UNC/${verbatimUnc[1]}/${verbatimUnc[2]}`;
  const verbatimDrive = path.match(/^\/\/\?\/([A-Za-z]:)(?:\/|$)/);
  if (verbatimDrive) return `//?/${verbatimDrive[1]}`;
  const unc = path.match(/^\/\/([^/?][^/]*)\/([^/]+)(?:\/|$)/);
  if (unc) return `//${unc[1]}/${unc[2]}`;
  return path.match(/^([A-Za-z]:)(?:\/|$)/)?.[1] ?? null;
}

function pathRootSegmentCount(path: string) {
  if (/^\/\/\?\/UNC\/[^/]+\/[^/]+(?:\/|$)/i.test(path)) return 6;
  if (/^\/\/\?\/[A-Za-z]:(?:\/|$)/.test(path)) return 4;
  if (/^\/\/[^/?][^/]*\/[^/]+(?:\/|$)/.test(path)) return 4;
  if (/^[A-Za-z]:(?:\/|$)/.test(path) || path.startsWith("/")) return 1;
  return 0;
}

export function resolveLocalMarkdownTarget(sourcePath: string, rawTarget: string) {
  if (rawTarget.startsWith("file:")) {
    const url = new URL(rawTarget);
    const decoded = decodeURIComponent(url.pathname).replace(/^\/([A-Za-z]:\/)/, "$1");
    const normalized = decoded.replace(/\\/g, "/");
    return url.hostname && url.hostname.toLowerCase() !== "localhost"
      ? `//${url.hostname}${normalized.startsWith("/") ? normalized : `/${normalized}`}`
      : normalized;
  }
  const normalizedSource = sourcePath.replace(/\\/g, "/");
  const normalizedTarget = rawTarget.replace(/\\/g, "/").split(/[?#]/, 1)[0] ?? "";
  if (/^[A-Za-z]:\//.test(normalizedTarget)) {
    return normalizedTarget;
  }
  if (normalizedTarget.startsWith("/")) {
    const windowsRoot = windowsPathRoot(normalizedSource);
    return windowsRoot ? `${windowsRoot}${normalizedTarget}` : normalizedTarget;
  }
  const sourceParts = normalizedSource.split("/");
  const rootSegmentCount = pathRootSegmentCount(normalizedSource);
  sourceParts.pop();
  for (const part of normalizedTarget.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (sourceParts.length > rootSegmentCount) sourceParts.pop();
    } else {
      sourceParts.push(part);
    }
  }
  return sourceParts.join("/");
}

function SafeLink({
  href,
  children,
  sourcePath,
  onOpenFile,
  onOpenFragment,
  onError,
}: {
  href?: string;
  children: ReactNode;
  sourcePath: string;
  onOpenFile: (path: string) => Promise<void> | void;
  onOpenFragment: (fragment: string) => void;
  onError: (message: string) => void;
}) {
  const safe = safeMarkdownUrl(href);
  if (!safe) return <span>{children}</span>;
  const local = href ? isLocalTarget(href) : false;
  const fragment = href?.startsWith("#") ?? false;
  const openTrustedTarget = () => {
    if (href?.startsWith("#")) {
      onOpenFragment(href.slice(1));
    } else if (local && href) {
      try {
        void Promise.resolve(onOpenFile(resolveLocalMarkdownTarget(sourcePath, href)))
          .catch((cause) => onError(errorMessage(cause)));
      } catch (cause) {
        onError(errorMessage(cause));
      }
    } else {
      void openUrl(safe).catch(() => window.open(safe, "_blank", "noopener,noreferrer"));
    }
  };
  const activate = (event: MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault();
    event.stopPropagation();
    openTrustedTarget();
  };
  const activateFromKeyboard = (event: KeyboardEvent<HTMLAnchorElement>) => {
    if ((!local && !fragment) || (event.key !== "Enter" && event.key !== " ")) return;
    event.preventDefault();
    event.stopPropagation();
    openTrustedTarget();
  };
  return (
    <a
      href={local || fragment ? undefined : safe}
      role={local || fragment ? "link" : undefined}
      tabIndex={local || fragment ? 0 : undefined}
      onClick={activate}
      onKeyDown={activateFromKeyboard}
      onAuxClick={(event) => {
        if (!local && !fragment) return;
        if (event.button === 1) activate(event);
        else event.preventDefault();
      }}
      onContextMenu={(event) => {
        if (!local && !fragment) return;
        event.preventDefault();
        event.stopPropagation();
      }}
      rel="noreferrer"
      target={local || fragment ? undefined : "_blank"}
    >
      {children}
    </a>
  );
}

function headingText(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(headingText).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return headingText(node.props.children);
  return "";
}

function headingSlug(node: ReactNode) {
  return headingText(node)
    .normalize("NFKD")
    .toLocaleLowerCase()
    .trim()
    .replace(/[^\p{Letter}\p{Number}\s-]/gu, "")
    .replace(/[\s-]+/g, "-")
    .replace(/^-|-$/g, "") || "section";
}

function filesMarkdownUrlTransform(rawUrl: string) {
  return rawUrl.startsWith("#") ? rawUrl : markdownUrlTransform(rawUrl);
}

export default function MarkdownRenderer({
  snapshot,
  client,
  lifecycle,
  on_open_file,
}: FileRendererProps) {
  const articleRef = useRef<HTMLElement>(null);
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retryToken, setRetryToken] = useState(0);
  const retry = useCallback(() => setRetryToken((value) => value + 1), []);

  useEffect(() => {
    if (!lifecycle.visible) return;
    const descriptor = snapshot.descriptor;
    if (!canRenderMarkdown(descriptor)) return;
    let cancelled = false;
    setText(null);
    setError(null);
    void client.readText(snapshot).then((resource) => {
      if (!cancelled && resource.revision === snapshot.revision) setText(resource.text);
    }).catch((cause) => {
      if (!cancelled) setError(errorMessage(cause));
    });
    return () => { cancelled = true; };
  }, [client, lifecycle.visible, retryToken, snapshot]);

  if (!lifecycle.visible) {
    return <div className="files-resource-state" role="status">Markdown preview suspended.</div>;
  }
  if (!canRenderMarkdown(snapshot.descriptor)) {
    return (
      <div className="files-resource-state" role="status">
        {snapshot.descriptor.unavailable_reason ?? "markdown_preview_unavailable"}
      </div>
    );
  }
  if (error) {
    return (
      <section className="files-resource-state" role="alert">
        <h2>Markdown preview unavailable</h2>
        <p>{error}</p>
        <button type="button" onClick={retry}>Retry</button>
      </section>
    );
  }
  if (text === null) return <div className="files-resource-state" role="status">Loading Markdown…</div>;
  const headingCounts = new Map<string, number>();
  const renderHeading = (tag: "h1" | "h2" | "h3" | "h4" | "h5" | "h6", children: ReactNode) => {
    const base = headingSlug(children);
    const count = (headingCounts.get(base) ?? 0) + 1;
    headingCounts.set(base, count);
    const id = count === 1 ? base : `${base}-${count}`;
    return createElement(tag, { id, tabIndex: -1 }, children);
  };
  const openFragment = (fragment: string) => {
    let id: string;
    try { id = decodeURIComponent(fragment); } catch { id = fragment; }
    const candidate = document.getElementById(id);
    const heading = candidate && articleRef.current?.contains(candidate) ? candidate : null;
    heading?.scrollIntoView({ block: "start" });
    heading?.focus({ preventScroll: true });
  };
  return (
    <article ref={articleRef} className="files-markdown-renderer">
      <Markdown
        components={{
          h1: ({ children }) => renderHeading("h1", children),
          h2: ({ children }) => renderHeading("h2", children),
          h3: ({ children }) => renderHeading("h3", children),
          h4: ({ children }) => renderHeading("h4", children),
          h5: ({ children }) => renderHeading("h5", children),
          h6: ({ children }) => renderHeading("h6", children),
          a: ({ href, children }) => (
            <SafeLink
              href={href}
              sourcePath={snapshot.descriptor.canonical_path}
              onOpenFile={on_open_file}
              onOpenFragment={openFragment}
              onError={setError}
            >
              {children}
            </SafeLink>
          ),
          img: ({ alt, src }) => {
            const safe = safeMarkdownUrl(src);
            return <span className="files-markdown-image-link">{alt || "Image"}{safe ? `: ${safe}` : ""}</span>;
          },
        }}
        remarkPlugins={[[remarkGfm, { singleTilde: false }]]}
        skipHtml
        urlTransform={filesMarkdownUrlTransform}
      >
        {text}
      </Markdown>
    </article>
  );
}
