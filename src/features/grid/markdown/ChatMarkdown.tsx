import Markdown from "react-markdown";
import type { Components } from "react-markdown";
import type { CSSProperties, MouseEvent } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import remarkGfm from "remark-gfm";
import { CodePanel } from "../chatCode";
import { CopyIconButton } from "../chatCopy";
import { markdownUrlTransform, safeMarkdownUrl } from "./markdownSafety";

interface ChatMarkdownProps {
  source: string;
}

function tableTextAlign(align: string | null | undefined): CSSProperties | undefined {
  return align === "left" || align === "center" || align === "right" || align === "justify" ? { textAlign: align as CSSProperties["textAlign"] } : undefined;
}

function tableAlignFromNode(node: unknown): string | null {
  if (!node || typeof node !== "object" || !("properties" in node)) return null;
  const properties = (node as { properties?: Record<string, unknown> }).properties;
  const align = properties?.align;
  return typeof align === "string" ? align : null;
}

function openMarkdownUrl(event: MouseEvent<HTMLAnchorElement>) {
  event.preventDefault();
  event.stopPropagation();
  const href = event.currentTarget.href;
  void openUrl(href).catch(() => {
    window.open(href, "_blank", "noopener,noreferrer");
  });
}

const components: Components = {
  a({ href, children }) {
    const safeUrl = safeMarkdownUrl(href);
    if (!safeUrl) return <span>{children}</span>;
    return (
      <a
        className="break-all font-medium text-[var(--color-wardian-accent)] underline decoration-[color-mix(in_srgb,var(--color-wardian-accent),transparent_55%)] underline-offset-2"
        href={safeUrl}
        onClick={openMarkdownUrl}
        rel="noreferrer"
        target="_blank"
      >
        {children}
      </a>
    );
  },
  blockquote({ children }) {
    return <blockquote className="border-l-2 border-wardian-light pl-3 text-muted-neutral">{children}</blockquote>;
  },
  code({ className, children }) {
    const raw = String(children).replace(/\n$/, "");
    const language = /language-([A-Za-z0-9_-]+)/.exec(className ?? "")?.[1] ?? "text";
    if (!className) {
      return <code className="rounded bg-[var(--color-wardian-sidebar-primary)] px-1 py-0.5 text-[12px]">{children}</code>;
    }
    return (
      <div className="relative">
        <div className="absolute right-1.5 top-1.5 z-10">
          <CopyIconButton label="Copy code block" value={raw} />
        </div>
        <CodePanel content={raw} language={language} className="pr-9" />
      </div>
    );
  },
  del({ children }) {
    return <del className="text-muted-neutral">{children}</del>;
  },
  h1({ children }) {
    return <div className="mt-1 text-[14px] font-bold leading-5 text-primary">{children}</div>;
  },
  h2({ children }) {
    return <div className="mt-1 text-[14px] font-bold leading-5 text-primary">{children}</div>;
  },
  h3({ children }) {
    return <div className="mt-1 text-[13px] font-bold leading-5 text-primary">{children}</div>;
  },
  h4({ children }) {
    return <div className="mt-1 text-[13px] font-bold leading-5 text-primary">{children}</div>;
  },
  h5({ children }) {
    return <div className="mt-1 text-[13px] font-bold leading-5 text-primary">{children}</div>;
  },
  h6({ children }) {
    return <div className="mt-1 text-[13px] font-bold leading-5 text-primary">{children}</div>;
  },
  hr() {
    return <hr className="border-0 border-t border-wardian-light" />;
  },
  img({ alt, src }) {
    const safeUrl = safeMarkdownUrl(src);
    return (
      <span className="inline-flex max-w-full flex-wrap items-center gap-1 rounded border border-wardian-light bg-[var(--color-wardian-card-bg-muted)] px-1.5 py-0.5 text-[12px] leading-5 text-muted-neutral">
        <span>{alt?.trim() || "Image"}</span>
        {safeUrl ? (
          <a className="break-all text-[var(--color-wardian-accent)] underline underline-offset-2" href={safeUrl} onClick={openMarkdownUrl} rel="noreferrer" target="_blank">
            {safeUrl}
          </a>
        ) : null}
      </span>
    );
  },
  input(props) {
    const label = props.checked ? "Completed task item" : "Incomplete task item";
    return <input {...props} aria-label={label} className="mr-1 align-middle accent-[var(--color-wardian-accent)]" disabled type="checkbox" />;
  },
  li({ children }) {
    return <li className="break-words">{children}</li>;
  },
  ol({ children }) {
    return <ol className="list-decimal space-y-1 pl-5 marker:text-muted-neutral">{children}</ol>;
  },
  p({ children }) {
    return <p className="break-words">{children}</p>;
  },
  pre({ children }) {
    return <>{children}</>;
  },
  strong({ children }) {
    return <strong className="font-semibold text-primary">{children}</strong>;
  },
  table({ children }) {
    return (
      <div className="overflow-x-auto rounded border border-wardian-light">
        <table className="min-w-full border-collapse text-left text-[12px] leading-5">{children}</table>
      </div>
    );
  },
  tbody({ children }) {
    return <tbody className="divide-y divide-wardian-light">{children}</tbody>;
  },
  td({ align, children, node }) {
    return (
      <td className="max-w-[320px] whitespace-normal break-words px-2 py-1 align-top text-primary" style={tableTextAlign(align ?? tableAlignFromNode(node))}>
        {children}
      </td>
    );
  },
  th({ align, children, node }) {
    return (
      <th
        className="max-w-[320px] whitespace-normal break-words bg-[var(--color-wardian-card-bg-muted)] px-2 py-1 align-top font-semibold text-primary"
        style={tableTextAlign(align ?? tableAlignFromNode(node))}
        scope="col"
      >
        {children}
      </th>
    );
  },
  thead({ children }) {
    return <thead className="border-b border-wardian-light">{children}</thead>;
  },
  ul({ children }) {
    return <ul className="list-disc space-y-1 pl-5 marker:text-muted-neutral">{children}</ul>;
  },
};

export function ChatMarkdown({ source }: ChatMarkdownProps) {
  return (
    <div className="space-y-2 text-[13px] leading-5 text-primary">
      <Markdown components={components} remarkPlugins={[[remarkGfm, { singleTilde: false }]]} skipHtml urlTransform={markdownUrlTransform}>
        {source}
      </Markdown>
    </div>
  );
}
