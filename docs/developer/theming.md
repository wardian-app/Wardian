# Semantic Theme Engine

Wardian features a robust, semantic theme engine that supports **Dark**, **Light**, and **System** modes. The system is built using CSS Variables and Tailwind CSS v4, ensuring high performance and ease of customization.

## 🏛️ Architecture

### 1. The Variable Layer
Located in `src/styles/App.css`, the theme is defined using a series of CSS variables prefixed with `--color-wardian-*`.

- **Structural Variables**: Define the background, card, and sidebar colors (`--color-wardian-bg`, `--color-wardian-sidebar-primary`).
- **Semantic Variables**: Define colors for system statuses (`--color-wardian-success`, `--color-wardian-processing`, `--color-wardian-error`).
- **Typography Variables**: Define colors for primary, muted, and bright text.
- **Terminal Variables**: Define the embedded xterm palette (`--color-wardian-terminal-*`) for terminal background, foreground, cursor, selection, and normal/bright ANSI colors.

### 2. The Theme Switcher
The `App.tsx` component manages the active theme state and persists it to `localStorage`.
- **System Sync**: Uses `window.matchMedia("(prefers-color-scheme: light)")` to automatically switch between light and dark modes when the "System" theme is selected.
- **Data Attribute**: The active theme is applied to the `<html>` element via the `data-theme` attribute (e.g., `<html data-theme="dark">`).

## 🎨 Design Tokens

### Semantic Statuses
Always use these tokens for status indicators to ensure consistency across themes:
| Status | Variable | Default (Dark) | Light Mode |
|--------|----------|----------------|------------|
| Idle | `--color-wardian-success` | `#10b981` | `#059669` |
| Processing | `--color-wardian-processing` | `#22d3ee` | `#0891b2` |
| Warning | `--color-wardian-warning` | `#f59e0b` | `#d97706` |
| Error | `--color-wardian-error` | `#ef4444` | `#dc2626` |
| Off | `--color-wardian-off` | `#4b5563` | `#4b5563` |

### Workflow Colors
Specific colors are reserved for the Visual Builder nodes:
- **Agent Node**: `--color-workflow-agent` (#3b82f6)
- **Command Node**: `--color-workflow-command` (#10b981)
- **Logic Node**: `--color-workflow-logic` (#f59e0b)
- **Comm Node**: `--color-workflow-comm` (#8b5cf6)

### Terminal Colors
Agent terminals and the bottom user terminal consume xterm colors through
`src/features/terminal/terminalTheme.ts`. Keep terminal rendering theme-aware by
adding terminal colors as `--color-wardian-terminal-*` variables first, then
mapping them into the xterm theme. Do not hardcode ANSI colors in terminal
components.

## 🛠️ Developer Usage

### Using Theme Variables
When building new components, **NEVER** use hardcoded hex values or standard Tailwind colors (e.g., `text-blue-500`). Instead, use the theme variables or the mapped utility classes:

```tsx
// ✅ Correct
<div className="bg-wardian-card text-primary border-wardian-border">...</div>

// ❌ Incorrect
<div className="bg-gray-900 text-white border-gray-700">...</div>
```

### Extending the Theme
To add a new theme color:
1. Define the variable in the `@theme` block in `App.css`.
2. Provide a dark-mode override in the `[data-theme='dark']` block when the
   color differs from the light/default palette.
3. (Optional) Create a mapped Tailwind utility class for ease of use.
