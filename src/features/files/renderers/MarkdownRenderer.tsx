import {
  isValidElement,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ComponentProps,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown, { type Components } from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import remarkGfm from "remark-gfm";

import { markdownUrlTransform, safeMarkdownUrl } from "../../grid/markdown/markdownSafety";
import { filePathIdentity, isWindowsAbsoluteFilePath } from "../fileResourceKey";
import type { FileRendererProps } from "../rendererRegistry";
import { useFileResource } from "../useFileResource";

const MARKDOWN_MAX_SIZE_BYTES = 16 * 1024 * 1024;
const MARKDOWN_MAX_LINE_COUNT = 200_000;
const MARKDOWN_SANITIZE_SCHEMA = {
  ...defaultSchema,
  tagNames: [...new Set([...(defaultSchema.tagNames ?? []), "details", "summary"])],
  attributes: {
    ...defaultSchema.attributes,
    "*": [...(defaultSchema.attributes?.["*"] ?? []), "className"],
    div: [...(defaultSchema.attributes?.div ?? []), "align"],
    details: [...(defaultSchema.attributes?.details ?? []), "open"],
    p: [...(defaultSchema.attributes?.p ?? []), "align"],
    img: [
      ...(defaultSchema.attributes?.img ?? []),
      "align",
      "width",
      "height",
      "loading",
      "decoding",
    ],
  },
  protocols: {
    ...defaultSchema.protocols,
    href: [...new Set([...(defaultSchema.protocols?.href ?? []), "file"])],
    src: [...new Set([...(defaultSchema.protocols?.src ?? []), "file"])],
  },
};

type MarkdownAstNode = {
  type?: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: MarkdownAstNode[];
};

function markdownAstText(node: MarkdownAstNode): string {
  if (node.type === "text") return node.value ?? "";
  return node.children?.map(markdownAstText).join("") ?? "";
}

/** Adds deterministic heading anchors during the Markdown transform itself. */
function rehypeMarkdownHeadingIds() {
  return (root: MarkdownAstNode) => {
    const counts = new Map<string, number>();
    const visit = (node: MarkdownAstNode) => {
      if (/^h[1-6]$/.test(node.tagName ?? "")) {
        const base = headingSlug(markdownAstText(node));
        const count = (counts.get(base) ?? 0) + 1;
        counts.set(base, count);
        node.properties = {
          ...node.properties,
          id: count === 1 ? base : `${base}-${count}`,
          tabIndex: -1,
        };
      }
      node.children?.forEach(visit);
    };
    visit(root);
  };
}

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
    const decoded = decodeURIComponent(url.pathname).replace(/^\/([A-Za-z]:[\\/])/, "$1");
    const localPath = url.hostname && url.hostname.toLowerCase() !== "localhost"
      ? `//${url.hostname}${decoded.startsWith("/") ? decoded : `/${decoded}`}`
      : decoded;
    return filePathIdentity(localPath);
  }
  const windowsSource = isWindowsAbsoluteFilePath(sourcePath);
  const normalizedSource = filePathIdentity(sourcePath);
  const targetPath = rawTarget.split(/[?#]/, 1)[0] ?? "";
  if (isWindowsAbsoluteFilePath(targetPath)) return filePathIdentity(targetPath);
  const normalizedTarget = windowsSource ? targetPath.replace(/\\/g, "/") : targetPath;
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
  return rawUrl.startsWith("#") || isLocalTarget(rawUrl)
    ? rawUrl
    : markdownUrlTransform(rawUrl);
}

type MarkdownImageProps = ComponentProps<"img"> & {
  source_path: string;
  resource_request: FileRendererProps["resource_request"];
  client: FileRendererProps["client"];
  lifecycle: FileRendererProps["lifecycle"];
};

function LocalMarkdownImage({
  src,
  alt,
  source_path,
  resource_request,
  client,
  lifecycle,
  ...imageProps
}: MarkdownImageProps) {
  const targetPath = useMemo(
    () => resolveLocalMarkdownTarget(source_path, src ?? ""),
    [source_path, src],
  );
  const request = useMemo(() => ({
    path: targetPath,
    agent_id: resource_request?.agent_id ?? null,
    // Exact picker grants authorize only their selected file, never a sibling image.
    user_file_capability_id: null,
  }), [resource_request?.agent_id, targetPath]);
  const resource = useFileResource(request, client);
  const leaseId = `markdown-image:${useId()}`;
  const [ticketUrl, setTicketUrl] = useState<string | null>(null);
  const [ticketError, setTicketError] = useState<string | null>(null);

  useEffect(() => {
    if (!lifecycle.visible || resource.status !== "ready" || !resource.snapshot) return;
    let active = true;
    const owner = resource.snapshot;
    void client.issueTicket(owner, leaseId).then((ticket) => {
      if (!active) return;
      setTicketUrl(ticket.url);
      setTicketError(null);
    }).catch((cause) => {
      if (active) setTicketError(errorMessage(cause));
    });
    return () => {
      active = false;
      void client.closeRendererLease(owner, leaseId).catch(() => undefined);
    };
  }, [client, leaseId, lifecycle.visible, resource.snapshot, resource.status]);

  if (resource.status === "error" || ticketError) {
    return (
      <span className="files-markdown-image-fallback" role="img" aria-label={alt || "Image unavailable"}>
        {alt || "Image unavailable"}
      </span>
    );
  }
  if (!ticketUrl) {
    return (
      <span className="files-markdown-image-loading" role="status">
        {alt ? `Loading ${alt}…` : "Loading image…"}
      </span>
    );
  }
  return <img {...imageProps} src={ticketUrl} alt={alt ?? ""} loading="lazy" decoding="async" />;
}

function MarkdownImage(props: MarkdownImageProps) {
  const { src, alt, source_path, resource_request, client, lifecycle, ...imageProps } = props;
  const safe = safeMarkdownUrl(src);
  if (!safe) {
    return (
      <span className="files-markdown-image-fallback" role="img" aria-label={alt || "Image unavailable"}>
        {alt || "Image unavailable"}
      </span>
    );
  }
  if (src && isLocalTarget(src)) {
    return (
      <LocalMarkdownImage
        {...imageProps}
        src={src}
        alt={alt}
        source_path={source_path}
        resource_request={resource_request}
        client={client}
        lifecycle={lifecycle}
      />
    );
  }
  return <img {...imageProps} src={safe} alt={alt ?? ""} loading="lazy" decoding="async" />;
}

export default function MarkdownRenderer({
  snapshot,
  client,
  lifecycle,
  buffer_snapshot,
  resource_request,
  on_open_file,
}: FileRendererProps) {
  const articleRef = useRef<HTMLElement>(null);
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retryToken, setRetryToken] = useState(0);
  const retry = useCallback(() => setRetryToken((value) => value + 1), []);
  const sourcePath = snapshot.descriptor.canonical_path;
  const onOpenFileRef = useRef(on_open_file);
  onOpenFileRef.current = on_open_file;
  const embeddedResourceRequest = useMemo<NonNullable<FileRendererProps["resource_request"]>>(
    () => ({
      path: resource_request?.path ?? sourcePath,
      agent_id: resource_request?.agent_id ?? null,
      user_file_capability_id: resource_request?.user_file_capability_id ?? null,
    }),
    [
      resource_request?.agent_id,
      resource_request?.path,
      resource_request?.user_file_capability_id,
      sourcePath,
    ],
  );
  const openFragment = useCallback((fragment: string) => {
    let id: string;
    try { id = decodeURIComponent(fragment); } catch { id = fragment; }
    const candidate = document.getElementById(id);
    const heading = candidate && articleRef.current?.contains(candidate) ? candidate : null;
    heading?.scrollIntoView({ block: "start" });
    heading?.focus({ preventScroll: true });
  }, []);
  const components = useMemo<Components>(() => ({
    a: ({ href, children }) => (
      <SafeLink
        href={href}
        sourcePath={sourcePath}
        onOpenFile={(path) => onOpenFileRef.current(path)}
        onOpenFragment={openFragment}
        onError={setError}
      >
        {children}
      </SafeLink>
    ),
    img: ({ alt, src, node: _node, ...imageProps }) => (
      <MarkdownImage
        {...imageProps}
        src={src}
        alt={alt}
        source_path={sourcePath}
        resource_request={embeddedResourceRequest}
        client={client}
        lifecycle={{ visible: lifecycle.visible }}
      />
    ),
  }), [client, embeddedResourceRequest, lifecycle.visible, openFragment, sourcePath]);

  useEffect(() => {
    if (!lifecycle.visible) return;
    const descriptor = snapshot.descriptor;
    if (!canRenderMarkdown(descriptor)) return;
    if (buffer_snapshot?.resource_id === snapshot.resource_id) {
      setText(buffer_snapshot.text);
      setError(null);
      return;
    }
    let cancelled = false;
    setError(null);
    void client.readText(snapshot).then((resource) => {
      if (!cancelled && resource.revision === snapshot.revision) setText(resource.text);
    }).catch((cause) => {
      if (!cancelled) setError(errorMessage(cause));
    });
    return () => { cancelled = true; };
  }, [buffer_snapshot, client, lifecycle.visible, retryToken, snapshot]);

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
  return (
    <article ref={articleRef} className="files-markdown-renderer">
      <Markdown
        components={components}
        rehypePlugins={[
          rehypeRaw,
          [rehypeSanitize, MARKDOWN_SANITIZE_SCHEMA],
          rehypeMarkdownHeadingIds,
        ]}
        remarkPlugins={[[remarkGfm, { singleTilde: false }]]}
        urlTransform={filesMarkdownUrlTransform}
      >
        {text}
      </Markdown>
    </article>
  );
}
