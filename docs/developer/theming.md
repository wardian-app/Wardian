# Semantic Theme Engine

Wardian features a robust, semantic theme engine that supports **Dark**, **Light**, and **System** modes. The system is built using CSS Variables and Tailwind CSS v4, ensuring high performance and ease of customization.

## 🏛️ Architecture

### 1. The Variable Layer
Located in `src/styles/App.css`, the theme is defined using a series of CSS variables prefixed with `--color-wardian-*`.

- **Structural Variables**: Define the background, card, and sidebar colors (`--color-wardian-bg`, `--color-wardian-sidebar-primary`).
- **Semantic Variables**: Define colors for system statuses (`--color-wardian-success`, `--color-wardian-processing`, `--color-wardian-error`).
- **Typography Variables**: Define colors for primary, muted, and bright text.

### 2. The Theme Switcher
The `App.tsx` component manages the active theme state and persists it to `localStorage`.
- **System Sync**: Uses `window.matchMedia("(prefers-color-scheme: light)")` to automatically switch between light and dark modes when the "System" theme is selected.
- **Data Attribute**: The active theme is applied to the `<html>` element via the `data-theme` attribute (e.g., `<html data-theme="dark">`).

### Dark Mode Palette Philosophy
Dark mode uses a **neutral gray scale** modeled on modern IDEs (VS Code Dark Modern): pure gray surfaces spread across a deliberate elevation ramp (`#141414` sidebars → `#191919` background → `#212121` cards, terminals inset at `#1a1a1a`) with no green or blue tints, and text tones chosen for AA+ contrast (`#ececec` primary). Brand identity in dark mode is carried by a bright gold accent (`#f2c14e`) and the semantic status colors, not by tinted surfaces. Light mode retains the warm parchment palette. Terminal ANSI colors live in `src/features/terminal/terminalThemes.ts`.

## 🎨 Design Tokens

### Semantic Statuses
Always use these tokens for status indicators to ensure consistency across themes:
| Status | Variable | Default (Dark) | Light Mode |
|--------|----------|----------------|------------|
| Idle | `--color-wardian-success` | `#10b981` | `#059669` |
| Processing | `--color-wardian-processing` | `#22d3ee` | `#0891b2` |
| Warning | `--color-wardian-warning` | `#f59e0b` | `#d97706` |
| Error | `--color-wardian-error` | `#ef4444` | `#dc2626` |
| Off | `--color-wardian-off` | `#6e7681` | `#4b5563` |

### Workflow Colors
Specific colors are reserved for the Visual Builder nodes:
- **Agent Node**: `--color-workflow-agent` (#3b82f6)
- **Command Node**: `--color-workflow-command` (#10b981)
- **Logic Node**: `--color-workflow-logic` (#f59e0b)
- **Comm Node**: `--color-workflow-comm` (#8b5cf6)

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
2. Provide a light-mode override in the `[data-theme='light']` block.
3. (Optional) Create a mapped Tailwind utility class for ease of use.
