# Chat Markdown Renderer

- **Status:** Proposed
- **Date:** 2026-06-07

## Context

Wardian chat mode renders provider-normalized `AgentChatEvent` records into a
dense supervision surface. Message rendering currently uses a custom lightweight
parser in `src/features/grid/AgentChatView.tsx`. It handles fenced code blocks,
ATX headings, flat ordered and unordered lists, paragraphs, markdown links,
inline code, and bold text.

That first slice made chat mode readable without adding dependencies, but it now
misses markdown that agents routinely emit. GitHub-flavored tables are the most
visible failure: a table such as an email inbox summary is shown as wrapped pipe
text instead of a scannable grid. The same limitation affects task lists,
nested lists, blockquotes, autolinks, strikethrough, and mixed inline formatting.

Chat mode should render common agent-authored markdown with high fidelity while
preserving Wardian's safety and density requirements. Raw terminal mode remains
the authoritative fallback for provider TUIs and PTY-specific behavior; this
spec only changes how already-normalized chat message text is rendered.

## Goals

- Render common GitHub-flavored markdown in agent messages, especially tables.
- Replace the current regex-oriented message parser with a maintained markdown
  pipeline that can parse block and inline markdown consistently.
- Keep message rendering safe: no raw HTML execution, no unsafe URL schemes, no
  unbounded remote media, and no layout-breaking content.
- Preserve Wardian's compact chat-mode visual language and theme-variable
  styling.
- Keep code block copy and syntax-highlighting affordances.
- Add focused tests for markdown features that agents produce in real sessions.

## Non-Goals

- Changing the backend `AgentChatEvent` DTO.
- Changing provider transcript normalization.
- Replacing terminal mode or making chat mode a full provider TUI.
- Supporting arbitrary raw HTML from markdown.
- Rendering remote images inline by default.
- Implementing every CommonMark extension in the first slice.

## Current Gaps

### P0

- **Tables:** Pipe tables, alignment markers, empty cells, and long cell content
  do not render as tables. Escaped pipes inside cells are not handled.
- **Task lists:** `- [ ]` and `- [x]` are rendered as ordinary list text.
- **Nested lists and continuations:** Indentation and wrapped list lines are
  flattened, which can change the meaning of plans and decision trees.
- **Inline composition:** Current inline parsing only handles links, inline
  code, and bold. Italic, strikethrough, nested emphasis, and mixed inline
  formatting are incomplete.

### P1

- **Blockquotes:** Quoted errors, quoted user text, and excerpts render as
  ordinary paragraphs.
- **Horizontal rules:** Standalone `---`, `***`, and `___` render as text.
- **Autolinks and bare URLs:** Only `[label](url)` links become anchors.
- **Markdown escaping:** Escaped punctuation such as `\*`, `\|`, and escaped
  brackets can render incorrectly.
- **Code fence variants:** Tilde fences and some malformed or unterminated
  fences are not handled with predictable fallback behavior.

### P2

- **Images:** Image markdown should be represented safely, but inline remote
  image loading should not be enabled by default because it can leak tracking
  requests, destabilize layout, and consume bandwidth.
- **Footnotes and definition lists:** Useful in long-form analysis, but lower
  frequency in chat supervision.
- **Raw HTML:** Should remain disabled unless there is a separate audited need.

## Decision

Adopt a sanitized GitHub-flavored markdown renderer for chat message bodies.
The preferred frontend stack is:

- `react-markdown` for React rendering;
- `remark-gfm` for GFM tables, task lists, strikethrough, and autolinks;
- a sanitization layer that rejects raw HTML and unsafe attributes;
- Wardian-owned component overrides for table, list, blockquote, link, image,
  code, paragraph, and heading nodes.

The custom parser in `AgentChatView.tsx` should be retired for message prose
once the new renderer covers existing behavior. Code block rendering can keep
using Wardian's current copy button and lightweight syntax highlighting by
adapting the markdown renderer's `code` node override.

If dependency risk blocks the first implementation, the fallback is not to grow
the current parser broadly. The fallback is a narrow P0-only patch for tables,
task lists, and nested list continuations, with the full GFM renderer still
tracked as the target architecture.

## Rendering Requirements

### Tables

Tables render as compact, theme-styled grids inside a horizontal overflow
container. Requirements:

- Header rows use stronger text and subtle themed background.
- Alignment markers map to cell text alignment.
- Empty cells stay visible.
- Long subjects, paths, or email addresses wrap within cells without expanding
  the chat card.
- Wide tables scroll horizontally within the message block instead of forcing
  the whole transcript to overflow.
- Table text remains selectable and included when copying the whole message.

### Lists

Lists render with stable indentation and no layout jumps:

- Ordered, unordered, and nested lists preserve hierarchy.
- Wrapped list item lines remain part of the item.
- Task list checkboxes are visible, disabled, and accessible.
- Adjacent lists of different types remain separate lists.

### Inline Markdown

Inline rendering supports:

- inline code;
- strong emphasis;
- italic emphasis;
- strikethrough;
- safe markdown links;
- bare HTTP, HTTPS, and file URLs where allowed by Wardian policy;
- escaped punctuation and nested inline content.

Links keep Wardian's current safety posture. Allowed URL protocols are `http:`,
`https:`, and `file:`. Unsafe links render as plain text rather than clickable
anchors. Safe links open through Wardian's native URL opener instead of relying
on webview `_blank` navigation.

### Blocks

Block rendering supports:

- ATX headings with compact chat-scale typography;
- blockquotes with a muted left border and themed text;
- horizontal rules as subtle dividers;
- fenced code blocks with copy action and existing syntax highlighting;
- inline code with compact monospace styling;
- paragraphs with preserved intentional line breaks where the markdown parser
  represents them.

### Images

Image markdown renders as a safe attachment-style placeholder by default:

- show the alt text when present;
- show the sanitized URL as a clickable link only when the protocol is allowed;
- do not fetch or display the remote image inline;
- do not reserve large image dimensions in the chat transcript.

A future spec may allow trusted local or explicitly approved image previews.

## Architecture

Add a focused markdown rendering boundary, for example:

```text
src/features/grid/markdown/
- ChatMarkdown.tsx
- markdownComponents.tsx
- markdownSafety.ts
- ChatMarkdown.test.tsx
```

`AgentChatView` should delegate message prose rendering to this boundary:

```tsx
<ChatMarkdown source={messageText} />
```

The boundary owns:

- markdown parser configuration;
- component overrides;
- URL sanitization;
- image placeholder behavior;
- table overflow behavior;
- tests for markdown rendering.

`AgentChatView` continues to own transcript rows, activity blocks, approval
controls, lazy rendering, message copy actions, and provider event grouping.

## Styling

Markdown components must use theme variables or themed utility classes, not
hardcoded Tailwind color tokens. The renderer should match chat mode's dense
supervision style:

- compact margins between blocks;
- no decorative nested cards inside message bubbles;
- stable table and code block dimensions;
- predictable wrapping for long tokens;
- no viewport-width font scaling;
- no text overflow outside message containers.

## Security

The renderer must be safe for untrusted provider output:

- Raw HTML is not rendered.
- Unsafe URL schemes such as `javascript:`, `data:`, and `vbscript:` are not
  clickable.
- Link text is rendered as React text, not HTML.
- Images do not fetch remote resources by default.
- Markdown parsing failures fall back to readable text rather than blank output.

## Accessibility

- Tables use semantic table elements.
- Task-list checkboxes are disabled and have accessible labels where possible.
- Links have visible focus states inherited from the design system.
- Code copy buttons keep descriptive labels.
- Blockquotes and horizontal rules do not hide meaningful text from screen
  readers.

## Testing

Add focused frontend tests for:

- the provided email-summary table rendering as a semantic table;
- table alignment, empty cells, escaped pipes, and long-cell overflow;
- nested ordered and unordered lists;
- task lists with checked and unchecked items;
- blockquotes and horizontal rules;
- italic, bold, strikethrough, inline code, and nested inline formatting;
- safe links, bare autolinks, `file:` links, and unsafe link fallback;
- image markdown rendering as a safe placeholder;
- code fences preserving copy and syntax-highlighting behavior;
- markdown parser failure fallback.

Existing `AgentChatView` tests should be updated only where they assert current
parser internals. Transcript loading, activity rows, approvals, and lazy
rendering tests should continue to prove their existing behavior.

Browser E2E is appropriate for a representative chat-rendering smoke test with
the mock provider. Native E2E is not required unless an implementation claims
PTY or provider-runtime behavior changed.

Frontend PRs must capture a feature-specific screenshot under
`e2e/screenshots/chat-markdown-renderer/<timestamp>/` and embed a hosted image
in the PR description.

## Acceptance Examples

The synthetic email summary table must render as a table with four columns:
`#`, `Subject`, `From`, and `Received`. Rows with empty subjects remain visually
aligned, and long subjects wrap inside the `Subject` cell.

```markdown
| # | Subject | From | Received |
|---|---------|------|----------|
| 1 | RE: Synthetic compliance question for TEST-123 | Alex Reviewer | 2026-06-06 |
| 8 | | Morgan Coordinator | 2026-06-05 |
```

Nested plan markdown must preserve hierarchy:

```markdown
1. Inspect renderer
   - Confirm table support
   - Confirm link safety
2. Add tests
   - [x] Existing markdown behavior
   - [ ] GFM table coverage
```

Unsafe links must remain plain text:

```markdown
Safe: [docs](https://example.test)
Unsafe: [run](javascript:alert)
```

Image markdown must not fetch remote content:

```markdown
![diagram](https://example.test/diagram.png)
```

It should render as an attachment/link placeholder rather than an inline image.

## Rollout

1. Add the markdown rendering boundary and dependencies.
2. Port existing message markdown behavior to `ChatMarkdown`.
3. Add P0/P1 markdown tests before removing the old parser path.
4. Replace message block rendering in `AgentChatView`.
5. Verify chat activity rendering, message copy, code copy, and approval rows.
6. Capture screenshot evidence for the PR.
7. Update `docs/guide/grid.md` if user-visible markdown support changes.

## Open Questions

- Should `file:` links continue to be allowed everywhere in chat markdown, or
  should they be limited to known workspace paths in a later hardening slice?
- Should markdown rendering be shared with the remote mobile chat view in the
  same implementation, or should the desktop grid chat renderer migrate first
  and the PWA follow after parity is proven?
