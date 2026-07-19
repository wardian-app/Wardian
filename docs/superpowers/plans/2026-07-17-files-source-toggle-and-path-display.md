# Files Source Toggle and Path Display Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an Obsidian-style rendered/source toggle for Markdown previews and show human-readable canonical paths without changing filesystem authority or resource identity.

**Architecture:** A shared path-display utility provides case- and separator-preserving presentation formatting without coupling Files to Explorer's comparison normalization. Renderer definitions may optionally contribute a source presentation; the Files surface owns the mounted tab's ephemeral presentation choice and passes it to the existing preview host without reopening the file resource. The mode bar renders one accessible icon control only when the resolved renderer contributes that presentation.

**Tech Stack:** React 19, TypeScript, Zustand-backed Workbench presentation metadata, lucide-react, Monaco Editor, Vitest, Testing Library, and Playwright.

## Global Constraints

- `Preview`, `Changes`, and `Draft` retain their current lifecycle meanings; source is a presentation inside `Preview`, not a fourth mode.
- Markdown defaults to rendered presentation after mount, restoration, or resource-key change.
- Plain text/source, images, PDFs, unsupported content, and unavailable resources do not expose a redundant source toggle.
- Presentation state is local and ephemeral: do not add it to `FilesSurfaceStateV1`, the Workbench document, Zustand presentation metadata, or backend DTOs.
- Switching presentation must reuse the current `FileResourceSnapshotV1` and `subscription_id`; it may perform the renderer's normal revision-bound text read but must not call `open_file_resource` again.
- Canonical paths remain unchanged for authorization, resource keys, IPC, Open With, Reveal, and Markdown link resolution.
- Display formatting preserves case and separator style while removing only Windows extended-length prefixes.
- All visual styles use Wardian theme variables; the icon control remains keyboard-reachable in narrow panes.
- No backend or native-runtime behavior changes are required; browser E2E is the lowest layer that can prove this UI slice.

---

### Task 1: Separate human-readable path display from path identity

**Files:**
- Create: `src/utils/displayPath.ts`
- Create: `src/utils/displayPath.test.ts`
- Modify: `src/features/files/FilesModeBar.tsx`
- Modify: `src/features/files/FilesModeBar.test.tsx`

**Interfaces:**
- Consumes: backend-owned `FileContentDescriptorV1.canonical_path`; Explorer comparison normalization remains unchanged and independent.
- Produces: `formatExplorerPathForDisplay(path: string): string`, native-separator breadcrumb tokens, and action callbacks that continue receiving the unformatted canonical path.

- [ ] **Step 1: Write failing display-formatter tests**

Create `src/utils/displayPath.test.ts` with exact preservation and prefix-removal cases:

```ts
import {
  formatExplorerPathForDisplay,
} from './displayPath';

describe('formatExplorerPathForDisplay', () => {
  it('removes a Windows extended drive prefix without changing case or separators', () => {
    expect(formatExplorerPathForDisplay('\\\\?\\C:\\Users\\Test\\Repo\\Notes.md'))
      .toBe('C:\\Users\\Test\\Repo\\Notes.md');
    expect(formatExplorerPathForDisplay('//?/C:/Users/Test/Repo/Notes.md'))
      .toBe('C:/Users/Test/Repo/Notes.md');
  });

  it('converts an extended UNC prefix to an ordinary UNC display path', () => {
    expect(formatExplorerPathForDisplay('\\\\?\\UNC\\SERVER\\Share\\Notes.md'))
      .toBe('\\\\SERVER\\Share\\Notes.md');
    expect(formatExplorerPathForDisplay('//?/UNC/SERVER/Share/Notes.md'))
      .toBe('//SERVER/Share/Notes.md');
  });

  it('leaves ordinary Windows, UNC, POSIX, and relative paths unchanged', () => {
    for (const path of [
      'C:\\Users\\Test\\Notes.md',
      '\\\\SERVER\\Share\\Notes.md',
      '/workspace/Notes.md',
      'docs/Notes.md',
    ]) {
      expect(formatExplorerPathForDisplay(path)).toBe(path);
    }
  });
});
```

- [ ] **Step 2: Run the path tests and verify the missing export fails**

Run: `npm run test -- --run src/utils/displayPath.test.ts`

Expected: FAIL because `formatExplorerPathForDisplay` is not exported.

- [ ] **Step 3: Implement the display-only formatter**

Create `src/utils/displayPath.ts` with this display-only export:

```ts
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
```

This helper deliberately performs no trimming, lowercasing, slash conversion, canonicalization, or trailing-separator removal.

- [ ] **Step 4: Add failing mode-bar path and action tests**

Extend the `modeBar()` helper in `src/features/files/FilesModeBar.test.tsx` so callers can override the descriptor. Add a test that proves display and action identities diverge only at the presentation boundary:

```tsx
function modeBar(overrides: {
  descriptor?: FileContentDescriptorV1;
  on_open_with?: (path: string) => Promise<void> | void;
  on_reveal?: (path: string) => Promise<void> | void;
} = {}) {
  return (
    <FilesModeBar
      resource_key="file:C:/work/report.pdf"
      state={state}
      descriptor={overrides.descriptor ?? descriptor}
      on_open_with={overrides.on_open_with ?? vi.fn()}
      on_reveal={overrides.on_reveal ?? vi.fn()}
    />
  );
}
```

```tsx
it("hides extended path prefixes while preserving canonical action paths", async () => {
  const user = userEvent.setup();
  const canonicalPath = "\\\\?\\C:\\Work\\Docs\\Report.pdf";
  const onOpenWith = vi.fn();
  const onReveal = vi.fn();
  render(modeBar({
    descriptor: { ...descriptor, canonical_path: canonicalPath },
    on_open_with: onOpenWith,
    on_reveal: onReveal,
  }));

  const breadcrumb = screen.getByRole("navigation", { name: "File location" });
  expect(breadcrumb).toHaveAttribute("title", "C:\\Work\\Docs\\Report.pdf");
  expect(breadcrumb).toHaveTextContent("C:\\Work\\Docs\\Report.pdf");
  expect(breadcrumb).not.toHaveTextContent("\\\\?\\");

  await user.click(screen.getByRole("button", { name: "File actions" }));
  await user.click(screen.getByRole("menuitem", { name: "Open With" }));
  expect(onOpenWith).toHaveBeenCalledWith(canonicalPath);

  await user.click(screen.getByRole("button", { name: "File actions" }));
  await user.click(screen.getByRole("menuitem", { name: "Reveal" }));
  expect(onReveal).toHaveBeenCalledWith(canonicalPath);
});
```

- [ ] **Step 5: Format only visible mode-bar text**

In `src/features/files/FilesModeBar.tsx`, keep separate action and display values:

```ts
import { formatExplorerPathForDisplay } from "../../utils/displayPath";

type BreadcrumbPart = { separator: string; label: string };

function breadcrumbParts(path: string): BreadcrumbPart[] {
  const separator = path.includes("\\") ? "\\" : "/";
  const drive = /^[A-Za-z]:[\\/]/.exec(path)?.[0].slice(0, 2);
  const unc = path.startsWith("\\\\") || path.startsWith("//");
  const rooted = !unc && path.startsWith("/");
  const rest = drive ? path.slice(3) : unc ? path.slice(2) : rooted ? path.slice(1) : path;
  const segments = rest.split(/[\\/]+/).filter(Boolean);

  if (drive) {
    return [
      { separator: "", label: drive },
      ...segments.map((label) => ({ separator, label })),
    ];
  }
  if (unc) {
    return [
      { separator: "", label: separator.repeat(2) },
      ...segments.map((label, index) => ({ separator: index === 0 ? "" : separator, label })),
    ];
  }
  if (rooted) {
    return [
      { separator: "", label: "/" },
      ...segments.map((label, index) => ({ separator: index === 0 ? "" : "/", label })),
    ];
  }
  return segments.map((label, index) => ({ separator: index === 0 ? "" : separator, label }));
}
```

Inside the component use:

```ts
const actionPath = descriptor?.canonical_path ?? resourcePath(resource_key);
const displayPath = formatExplorerPathForDisplay(actionPath);
const parts = useMemo(() => breadcrumbParts(displayPath), [displayPath]);
```

Render each token as `{part.separator}{part.label}`, set the breadcrumb `title` to `displayPath`, and call `action(actionPath)` in `runAction`. Do not pass `displayPath` to Open With or Reveal.

- [ ] **Step 6: Run focused tests and commit the display boundary**

Run: `npm run test -- --run src/utils/displayPath.test.ts src/features/files/FilesModeBar.test.tsx`

Expected: PASS, including ordinary comparison normalization, exact display formatting, native separators, and canonical callback arguments.

```bash
git add src/utils/displayPath.ts src/utils/displayPath.test.ts src/features/files/FilesModeBar.tsx src/features/files/FilesModeBar.test.tsx
git commit -m "fix(files): present readable canonical paths"
```

### Task 2: Add renderer-contributed source presentation

**Files:**
- Modify: `src/features/files/rendererRegistry.ts`
- Modify: `src/features/files/rendererRegistry.test.ts`
- Modify: `src/features/files/FilePreview.tsx`
- Modify: `src/features/files/FilesModeBar.tsx`
- Modify: `src/features/files/FilesSurface.tsx`
- Modify: `src/features/files/FilesSurface.css`
- Modify: `src/features/files/FilesSurface.test.tsx`

**Interfaces:**
- Consumes: the existing lazy renderer factory, `FileResourceSnapshotV1`, `MonacoTextRenderer`, Files lifecycle visibility, and the mode-bar action area.
- Produces: `FilePreviewPresentation`, optional `FileRendererDefinition.source`, ephemeral per-mounted-surface presentation state, and the `View source` / `View rendered` icon control.

- [ ] **Step 1: Write failing registry capability tests**

Extend `src/features/files/rendererRegistry.test.ts` to assert the production registry contributes source only for Markdown:

```ts
it("contributes Monaco source presentation only for Markdown", () => {
  const markdown = defaultRendererRegistry.resolve(descriptor({
    renderer_kind: "markdown",
    mime_type: "text/markdown",
    encoding: "utf-8",
  }));
  const text = defaultRendererRegistry.resolve(descriptor({
    renderer_kind: "text",
    mime_type: "text/plain",
    encoding: "utf-8",
  }));
  const pdf = defaultRendererRegistry.resolve(descriptor());

  expect(markdown.source).toBeDefined();
  expect(markdown.source?.create_renderer).toBeTypeOf("function");
  expect(text.source).toBeUndefined();
  expect(pdf.source).toBeUndefined();
});
```

Also construct a definition with `source: { render: EmptyRenderer }` and assert registry construction rejects the missing retry factory. This keeps rejected lazy imports retryable in both presentations.

```ts
const invalidSource = {
  ...renderer("markdown"),
  source: { render: EmptyRenderer },
} as unknown as FileRendererDefinition;
expect(() => new RendererRegistry([
  invalidSource,
  renderer("unsupported"),
])).toThrow(/markdown.*source.*create_renderer/i);
```

- [ ] **Step 2: Run the registry test and verify source capability is absent**

Run: `npm run test -- --run src/features/files/rendererRegistry.test.ts`

Expected: FAIL because `FileRendererDefinition` has no `source` presentation.

- [ ] **Step 3: Define a reusable presentation factory and register Markdown source**

Refactor `src/features/files/rendererRegistry.ts` around these exact interfaces:

```ts
export type FilePreviewPresentation = "rendered" | "source";

export type FileRendererPresentationDefinition = {
  render: LazyExoticComponent<ComponentType<FileRendererProps>>;
  create_renderer: () => LazyExoticComponent<ComponentType<FileRendererProps>>;
};

export type FileRendererDefinition = FileRendererPresentationDefinition & {
  renderer_id: string;
  matches: (descriptor: FileContentDescriptorV1) => boolean;
  capabilities: {
    preview: boolean;
    changes: "line" | "version" | "none";
    draft: boolean;
    annotations: "line_range" | "spatial" | "general";
  };
  source?: FileRendererPresentationDefinition;
};
```

Create presentations through one helper and extend `rendererDefinition` with an optional loader:

```ts
type RendererLoader = () => Promise<{ default: ComponentType<FileRendererProps> }>;

function rendererPresentation(load: RendererLoader): FileRendererPresentationDefinition {
  const createRenderer = () => lazy(load);
  return { render: createRenderer(), create_renderer: createRenderer };
}

function rendererDefinition(
  renderer_id: string,
  capabilities: FileRendererDefinition["capabilities"],
  load: RendererLoader,
  loadSource?: RendererLoader,
): FileRendererDefinition {
  return {
    renderer_id,
    matches: (descriptor) => (
      descriptor.renderer_kind === renderer_id
      || rendererIdForValidatedMime(descriptor) === renderer_id
    ),
    capabilities,
    ...rendererPresentation(load),
    source: loadSource ? rendererPresentation(loadSource) : undefined,
  };
}
```

Validate and freeze `definition.source` in the registry constructor just as the primary factory is validated:

```ts
if (definition.source && typeof definition.source.create_renderer !== "function") {
  throw new Error(`renderer ${definition.renderer_id} source requires a create_renderer factory`);
}
this.#definitionsById.set(definition.renderer_id, Object.freeze({
  ...definition,
  capabilities: Object.freeze({ ...definition.capabilities }),
  source: definition.source ? Object.freeze({ ...definition.source }) : undefined,
}));
```

Register Markdown as:

```ts
rendererDefinition("markdown", {
  preview: true,
  changes: "line",
  draft: true,
  annotations: "line_range",
}, () => import("./renderers/MarkdownRenderer"), () => import("./renderers/MonacoTextRenderer"))
```

Do not add source factories to text, image, PDF, unsupported, HTML, or SVG definitions.

- [ ] **Step 4: Add failing Files surface interaction and lifecycle tests**

In `src/features/files/FilesSurface.test.tsx`, add a lazy `SourceRenderer` test component and let the local `definition()` helper accept it as an optional source presentation:

```tsx
const SourceRenderer = lazy(async () => ({
  default: () => <div data-testid="source-renderer">Source</div>,
}));

function definition(
  renderer_id: string,
  renderComponent: FileRendererDefinition["render"] = PreviewRenderer,
  sourceComponent?: FileRendererDefinition["render"],
): FileRendererDefinition {
  return {
    renderer_id,
    matches: ({ renderer_kind }) => renderer_kind === renderer_id,
    capabilities: {
      preview: true,
      changes: renderer_id === "pdf" ? "version" : "none",
      draft: false,
      annotations: "general",
    },
    render: renderComponent,
    create_renderer: () => renderComponent,
    source: sourceComponent
      ? { render: sourceComponent, create_renderer: () => sourceComponent }
      : undefined,
  };
}
```

Cover all of these behaviors in explicit tests:

```tsx
it("switches Markdown between rendered and source presentations", async () => {
  const user = userEvent.setup();
  useFileResourceMock.mockReturnValue({
    status: "ready",
    snapshot: snapshot(descriptor({
      display_name: "notes.md",
      extension: "md",
      mime_type: "text/markdown",
      encoding: "utf-8",
      renderer_kind: "markdown",
      line_count: 2,
      capabilities: { preview: true, changes: true, draft: true, stream: false },
    })),
    error: null,
    retry: vi.fn(),
  });
  const markdownRegistry = new RendererRegistry([
    definition("markdown", PreviewRenderer, SourceRenderer),
    definition("unsupported", UnsupportedPreview),
  ]);
  render(<FilesSurface {...props({ registry: markdownRegistry })} />);

  const viewSource = screen.getByRole("button", { name: "View source" });
  expect(viewSource).toHaveAttribute("aria-pressed", "false");
  expect(viewSource).toHaveAttribute("title", "View source");
  fireEvent.click(viewSource);
  expect(await screen.findByTestId("source-renderer")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "View rendered" }))
    .toHaveAttribute("aria-pressed", "true");
  const viewRendered = screen.getByRole("button", { name: "View rendered" });
  viewRendered.focus();
  expect(viewRendered).toHaveFocus();
  await user.keyboard("{Enter}");
  expect(await screen.findByTestId("preview-renderer")).toBeInTheDocument();
});
```

Import `userEvent` from `@testing-library/user-event`. The click into source and keyboard activation back to rendered together prove both pointer and native-button keyboard behavior.

Add separate assertions that:

```tsx
expect(screen.queryByRole("button", { name: /View (source|rendered)/ })).toBeNull();
```

for PDF, plain text without a source contribution, unsupported, and unavailable snapshots. Rerender one Markdown Files surface hidden and visible with the same `resource_key` and assert source returns; then rerender with a different `resource_key` and matching snapshot and assert it returns to rendered. Assert a failing source renderer stays inside `RendererErrorBoundary`, and toggling back to rendered clears that presentation-local error.

- [ ] **Step 5: Run the surface tests and verify the UI contract is missing**

Run: `npm run test -- --run src/features/files/rendererRegistry.test.ts src/features/files/FilesSurface.test.tsx src/features/files/FilesModeBar.test.tsx`

Expected: FAIL because the Files surface has no presentation state or icon control.

- [ ] **Step 6: Make FilePreview presentation-aware without acquiring resources**

Add `presentation: FilePreviewPresentation` to `FilePreviewProps`. Resolve an unsupported source request back to rendered and choose only the renderer factory:

```ts
const definition = registry.resolve(snapshot.descriptor);
const activePresentation = presentation === "source" && definition.source
  ? "source"
  : "rendered";
const createRenderer = activePresentation === "source"
  ? definition.source!.create_renderer
  : definition.create_renderer;
const Renderer = useMemo(
  () => createRenderer(),
  [createRenderer, resetToken],
);
```

Include `activePresentation` in the `RendererErrorBoundary` key so switching away from a failed source renderer remounts only the local presentation boundary:

```tsx
key={`${snapshot.resource_id}@${snapshot.revision}:${activePresentation}`}
```

Continue passing the same `snapshot`, `client`, lifecycle, and action callbacks. Do not invoke `useFileResource`, `client.open`, or `client.close` from `FilePreview`.

- [ ] **Step 7: Own ephemeral presentation state at the Files surface boundary**

In `FilesSurface`, use an outer state record so a normal hidden/visible transition preserves the selection while a resource change resets immediately:

```ts
type PreviewPresentationState = {
  resource_key: string;
  presentation: FilePreviewPresentation;
};

const [previewState, setPreviewState] = useState<PreviewPresentationState>({
  resource_key: props.resource_key,
  presentation: "rendered",
});
const previewPresentation = previewState.resource_key === props.resource_key
  ? previewState.presentation
  : "rendered";

useEffect(() => {
  setPreviewState({ resource_key: props.resource_key, presentation: "rendered" });
}, [props.resource_key]);

const setPreviewPresentation = useCallback((presentation: FilePreviewPresentation) => {
  setPreviewState({ resource_key: props.resource_key, presentation });
}, [props.resource_key]);
```

Pass `previewPresentation` and `setPreviewPresentation` into `ActiveFilesSurface`. Resolve `sourceAvailable` from the ready snapshot's registry definition, pass it to the mode bar, and pass the presentation into `FilePreview`. The suspended branch passes `source_available={false}` so no misleading control is shown while descriptor authority is unavailable; the outer state remains intact for the same resource.

- [ ] **Step 8: Add the Obsidian-style icon control and responsive action group**

In `FilesModeBar.tsx`, import `BookOpen` and `Pencil` from `lucide-react` and add props:

```ts
preview_presentation: FilePreviewPresentation;
source_available: boolean;
on_preview_presentation_change: (presentation: FilePreviewPresentation) => void;
```

Compute the label and icon once, then insert the button immediately before the existing `.files-overflow` element. Wrap that new button and the existing overflow element in `<div className="files-header-actions">`; the overflow trigger, metadata, menu items, refs, and keyboard handlers remain byte-for-byte unchanged.

```tsx
const sourceActive = preview_presentation === "source";
const sourceLabel = sourceActive ? "View rendered" : "View source";
const SourceIcon = sourceActive ? BookOpen : Pencil;

{source_available ? (
  <button
    type="button"
    className="files-presentation-toggle"
    aria-label={sourceLabel}
    aria-pressed={sourceActive}
    title={sourceLabel}
    onClick={() => on_preview_presentation_change(sourceActive ? "rendered" : "source")}
  >
    <SourceIcon size={15} aria-hidden="true" />
  </button>
) : null}
```

Add themed styles in `FilesSurface.css`:

```css
.files-header-actions {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 2px;
}

.files-presentation-toggle {
  display: inline-grid;
  width: 28px;
  height: 26px;
  place-items: center;
  border: 0;
  border-radius: 4px;
  background: transparent;
  color: var(--color-wardian-text-muted-neutral);
}

.files-presentation-toggle:hover,
.files-presentation-toggle:focus-visible,
.files-presentation-toggle[aria-pressed="true"] {
  background: var(--color-wardian-card-bg-muted);
  color: var(--color-wardian-text);
}
```

In the `max-width: 220px` container rule, place `.files-header-actions` in grid column 2/row 1 in place of `.files-overflow`. Keep `.files-overflow` positioned relative so its menu remains pane-bound.

- [ ] **Step 9: Run focused tests and commit the source presentation**

Run: `npm run test -- --run src/features/files/rendererRegistry.test.ts src/features/files/FilesSurface.test.tsx src/features/files/FilesModeBar.test.tsx src/features/files/renderers/MonacoTextRenderer.test.tsx src/features/files/renderers/MarkdownRenderer.test.tsx`

Expected: PASS. Markdown exposes one icon toggle; all other renderers omit it; presentation survives hidden/visible transitions, resets on resource change, and renderer failures stay local.

```bash
git add src/features/files
git commit -m "feat(files): toggle markdown source presentation"
```

### Task 3: Prove the interaction, document it, and attach visual evidence

**Files:**
- Modify: `e2e/tests/files-surface-foundation.spec.ts`
- Create: `e2e/screenshots/files-source-toggle/2026-07-17/markdown-source-toggle.png`
- Modify: `docs/guide/explorer.md`
- Modify: `docs/guide/workbench.md`
- Modify: `docs/developer/workbench-surfaces.md`

**Interfaces:**
- Consumes: the mocked Workbench Files IPC resource stream, existing Markdown fixture, Workbench tab lifecycle, and PR #673.
- Produces: browser proof of subscription reuse and state lifetime, a feature-specific screenshot, user guidance, developer ownership documentation, and an embedded HTTPS PR image.

- [ ] **Step 1: Add a browser acceptance scenario for Markdown source**

Add this scenario to `e2e/tests/files-surface-foundation.spec.ts` using the existing `bootFilesWorkbench`, `filesTab`, `ALPHA_PATH`, and `BETA_PATH` helpers:

```ts
test("switches Markdown source without reopening the resource and preserves it per tab", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const ipc = await bootFilesWorkbench(page);
  const alphaRow = page.getByRole("treeitem", { name: "alpha.md" });
  const betaRow = page.getByRole("treeitem", { name: "beta.md" });

  await alphaRow.dblclick();
  await expect(page.getByRole("heading", { name: "Alpha document" })).toBeVisible();
  const openCount = (await ipc.calls("open_file_resource")).length;

  const viewSource = page.getByRole("button", { name: "View source" });
  await expect(viewSource).toHaveAttribute("aria-pressed", "false");
  await viewSource.click();
  await expect(page.getByTestId("monaco-text-renderer")).toBeVisible();
  await expect(page.getByRole("button", { name: "View rendered" }))
    .toHaveAttribute("aria-pressed", "true");
  expect((await ipc.calls("open_file_resource")).length).toBe(openCount);

  await betaRow.dblclick();
  await expect(page.getByRole("heading", { name: "Beta document" })).toBeVisible();
  await filesTab(page, ALPHA_PATH).click();
  await expect(page.getByTestId("monaco-text-renderer")).toBeVisible();
  await expect(page.getByRole("button", { name: "View rendered" })).toBeVisible();

  const surface = page.getByTestId("files-surface");
  await surface.evaluate((element) => {
    element.style.width = "220px";
    element.style.maxWidth = "220px";
    element.style.flex = "0 0 220px";
  });
  const toggle = page.getByRole("button", { name: "View rendered" });
  await toggle.focus();
  await expect(toggle).toBeFocused();
  const [toggleBox, surfaceBox] = await Promise.all([toggle.boundingBox(), surface.boundingBox()]);
  expect(toggleBox).not.toBeNull();
  expect(surfaceBox).not.toBeNull();
  expect(toggleBox!.x).toBeGreaterThanOrEqual(surfaceBox!.x - 0.5);
  expect(toggleBox!.x + toggleBox!.width).toBeLessThanOrEqual(surfaceBox!.x + surfaceBox!.width + 0.5);

  await page.screenshot({
    path: path.resolve(
      "e2e/screenshots/files-source-toggle/2026-07-17/markdown-source-toggle.png",
    ),
    fullPage: true,
  });
});
```

The existing image and PDF scenarios continue proving those renderers have only their own controls.

- [ ] **Step 2: Run the browser acceptance scenario**

Run: `npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/files-surface-foundation.spec.ts --grep "switches Markdown source"`

Expected: PASS after Task 2, with no extra `open_file_resource` call during the presentation switch and with the feature screenshot created.

- [ ] **Step 3: Document the presentation boundary**

Update `docs/guide/explorer.md` under **Preview Controls** with these user-facing facts:

```markdown
Rendered Markdown includes a compact **View source** icon beside the file
actions. It switches the current Preview presentation to the read-only Monaco
source view; the reading icon switches back to rendered Markdown. This does not
create another tab or file subscription. Plain text is already source, while
images and PDFs keep their media-specific controls.

Windows paths are shown without the internal `\\?\` extended-length prefix.
Wardian still retains and authorizes the original canonical path behind the
displayed breadcrumb.
```

Update `docs/guide/workbench.md` under **Files Previews** to distinguish rendered/source presentation from the unavailable `Changes` and `Draft` lifecycles. Update `docs/developer/workbench-surfaces.md` under **Files foundation state and runtime** to record that renderer-contributed source presentation is ephemeral React state, never serialized, and never changes subscription ownership.

- [ ] **Step 4: Run the complete project validation gates**

From the repository root run:

```bash
npm run lint
npm run test
npm run build
npx playwright test --config e2e/playwright.workbench.config.ts e2e/tests/files-surface-foundation.spec.ts
```

Expected: all frontend checks and the complete Files browser suite pass.

From `src-tauri` run:

```bash
cargo check
cargo clippy --all-targets -- -D warnings
cargo test -- --test-threads=1
```

Expected: the unchanged backend remains green under every required project gate. Native E2E is not run because this slice changes no Tauri IPC, filesystem authority, or PTY behavior.

- [ ] **Step 5: Inspect the visual result and repository integrity**

Open `e2e/screenshots/files-source-toggle/2026-07-17/markdown-source-toggle.png` and verify the reading control is aligned with File actions, the Monaco source fills the pane, the breadcrumb remains legible, and the 220px pane does not clip the control. The Windows-prefix behavior is proved by the platform-independent unit test because this browser fixture uses a POSIX path.

Run:

```bash
git diff --check
git status --short
git diff --name-only
```

Expected: no whitespace errors; only the planned Files, Explorer path utility, E2E, documentation, and screenshot files are changed. Inspect the diff for credentials, `.env` content, local absolute paths, generated build output, and unrelated user changes; none may be committed.

- [ ] **Step 6: Commit documentation and browser evidence**

```bash
git add e2e/tests/files-surface-foundation.spec.ts e2e/screenshots/files-source-toggle/2026-07-17/markdown-source-toggle.png docs/guide/explorer.md docs/guide/workbench.md docs/developer/workbench-surfaces.md
git commit -m "test(files): prove markdown source presentation"
```

- [ ] **Step 7: Push and embed the feature screenshot in PR #673**

Run:

```bash
git push origin feat/files-surface-foundation
```

Then, in PowerShell, preserve the existing PR body and append the commit-bound HTTPS evidence:

```powershell
$evidenceCommit = git rev-parse HEAD
$imageUrl = "https://raw.githubusercontent.com/wardian-app/Wardian/$evidenceCommit/e2e/screenshots/files-source-toggle/2026-07-17/markdown-source-toggle.png"
$currentBody = gh pr view 673 --json body --jq .body
$updatedBody = "$currentBody`n`n## Markdown source presentation`n`n![Markdown source presentation toggle]($imageUrl)"
gh pr edit 673 --body $updatedBody
gh pr view 673 --json url,body
```

Expected: PR #673 contains the representative image as an embedded HTTPS Markdown image, not merely a local path, and the frontend screenshot check can detect it.
