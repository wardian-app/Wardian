import { useRef } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { MarkdownDocument, isLocalMarkdownTarget } from '../files/renderers/MarkdownRenderer';
import { safeMarkdownUrl } from '../grid/markdown/markdownSafety';
import '../files/FilesSurface.css';

/** Preview the draft without changing its source or granting filesystem access. */
export function LibraryMarkdown({ value }: { value: string }) {
    const article = useRef<HTMLElement>(null);
    const frontmatter = /^(?:\uFEFF)?---\r?\n[\s\S]*?\r?\n(?:---|\.\.\.)(?:\r?\n|$)/.exec(value)?.[0];
    const body = frontmatter ? value.slice(frontmatter.length) : value;
    return (
        <article ref={article} className="files-markdown-renderer library-markdown" data-testid="library-markdown-preview">
            {frontmatter && (
                <details className="library-frontmatter">
                    <summary>Document metadata</summary>
                    <pre>{frontmatter.trim()}</pre>
                </details>
            )}
            {body.trim() ? <MarkdownDocument text={body} components={{
                a: ({ href, children }) => {
                    const safe = safeMarkdownUrl(href);
                    if (!safe) return <span>{children}</span>;
                    if (href?.startsWith('#')) return (
                        <a href={href} onClick={(event) => {
                            event.preventDefault();
                            let id = href.slice(1);
                            try { id = decodeURIComponent(id); } catch { /* Use the literal fragment. */ }
                            const heading = Array.from(article.current?.querySelectorAll<HTMLElement>('[id]') ?? [])
                                .find((candidate) => candidate.id === id);
                            heading?.scrollIntoView({ block: 'start' });
                            heading?.focus({ preventScroll: true });
                        }}>{children}</a>
                    );
                    if (href && isLocalMarkdownTarget(href)) return <span title={`Local file: ${href}. Open the library folder to inspect it.`}>{children}</span>;
                    return <a href={safe} target="_blank" rel="noreferrer" onClick={(event) => {
                        event.preventDefault();
                        void openUrl(safe).catch(() => window.open(safe, '_blank', 'noopener,noreferrer'));
                    }}>{children}</a>;
                },
                img: ({ src, alt }) => {
                    const safe = safeMarkdownUrl(src);
                    return safe && src && !isLocalMarkdownTarget(src)
                        ? <img src={safe} alt={alt ?? ''} loading="lazy" decoding="async" />
                        : <span className="files-markdown-image-fallback">{alt || 'Local image — open the library folder to view'}</span>;
                },
            }} /> : <p className="text-muted">No instructions yet. Choose Edit to add content.</p>}
        </article>
    );
}
