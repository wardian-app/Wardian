---
colors:
  primary: "#926a09"
  primary-hover: "#7d5a07"
  background: "#fcfaf5"
  surface: "#f3f4f6"
  text: "#111827"
  text-muted: "#445d44"
  text-muted-neutral: "#4b5563"
  border: "#e5e7eb"
  border-heavy: "#d1d5db"
  
  # Semantic Statuses
  success: "#059669"
  processing: "#0891b2"
  warning: "#d97706"
  error: "#dc2626"
  off: "#4b5563"
  headless: "#8b5cf6"

  # Workflow Semantics
  workflow-agent: "#3b82f6"
  workflow-command: "#10b981"
  workflow-logic: "#f59e0b"
  workflow-comm: "#8b5cf6"

typography:
  families:
    sans: "Inter, system-ui, sans-serif"
    mono: "'JetBrains Mono', 'Cascadia Code', monospace"
  styles:
    label-small:
      fontSize: "11px"
      fontWeight: 700
      letterSpacing: "0.05em"
    tab:
      fontSize: "12px"
      fontWeight: 500
    telemetry:
      fontSize: "10px"
      fontWeight: 600
      fontFamily: "{typography.families.mono}"

layout:
  sidebar-primary: "64px"
  sidebar-content: "260px"
  sidebar-secondary: "240px"
  topbar-height: "40px"
  grid-gap: "12px"

shapes:
  radius-sm: "5px"
  radius-md: "6px"
  radius-lg: "8px"

components:
  agent-card:
    backgroundColor: "{colors.surface}"
    borderRadius: "{shapes.radius-md}"
    border: "1px solid {colors.border}"
  sidebar-rail:
    backgroundColor: "{colors.surface}"
    width: "{layout.sidebar-primary}"
  tab-active:
    color: "{colors.text}"
    borderTop: "2px solid {colors.primary}"
---

# DESIGN

## Overview

Wardian is a high-performance "Integrated Agent Environment" designed for developers and AI orchestrators. Its visual identity is built on the **"Habitat"** metaphor—a living, structured space where autonomous agents evolve and operate.

### Brand Personality
- **Tactile**: Physical-first organization using drag-and-drop grids and visible telemetry.
- **High-Tech / Omniscient**: A command-center view of multiple agent minds in real-time.
- **Ecological / Transparent**: "Markdown-as-Truth" ensuring the system's state is always inspectable.

## Colors

The palette is optimized for clarity and technical power. **Light mode** is the primary default to maximize legibility during long-running sessions, with a robust gold accent representing high-value orchestration.

- **Primary**: `{colors.primary}` is used for active states, focus rings, and critical orchestration controls.
- **Background**: A warm, paper-like neutral `{colors.background}` reduces eye strain.
- **Status Colors**: Emerald for Idle, Cyan for Processing, Amber for Action Required, and Red for Errors.

## Typography

Wardian uses a dual-font system to distinguish between human-readable content and machine telemetry.

- **Sans-Serif**: Used for all UI controls, labels, and documentation.
- **Monospace**: Reserved for `{typography.styles.telemetry}`, terminal outputs, and raw agent logs.

## Layout

The interface follows a strict **Three-Zone Layout** to manage cognitive load:
1. **Left (Control)**: Persistent icon-rail for navigation.
2. **Center (Habitat)**: The primary workspace for agent cards, nodes, and canvases.
3. **Right (Roster)**: Searchable list of active agents and their health status.

## Elevation & Depth

Wardian avoids heavy skeuomorphism, favoring **Layered Flatness**. Depth is communicated through subtle borders and background shifts rather than large shadows.

- **Level 0**: Base background `{colors.background}`.
- **Level 1**: Cards and Sidebars `{colors.surface}`.
- **Level 2**: Popovers and Context Menus, using `{colors.border-heavy}` for definition.

## Shapes

Geometric precision reinforces the "Integrated Environment" feel.
- **Component Radii**: `{shapes.radius-md}` is the standard for cards and rows.
- **Action Radii**: `{shapes.radius-sm}` for buttons and input fields.
- **Container Radii**: `{shapes.radius-lg}` for context menus and floating overlays.

## Components

### Agent Cards
The primary unit of the Habitat. Cards must visually communicate status through the top border or a dedicated status indicator using semantic colors.

### Sidebar Rail
The "Control" zone. It must remain thin (`{layout.sidebar-primary}`) to maximize the usable workspace while providing immediate access to global features.

## Do's and Don'ts

### Do
- Use **Direct Visual Manipulation** (drag-and-drop) as the primary interaction mode.
- Maintain **High Signal-to-Noise** ratio; every pixel of telemetry must be legible.
- Support **Keyboard Sovereignty** via command palettes.

### Don't
- Use generic chat interface patterns (e.g., standard message bubbles).
- Use "obtuse" CLI-only patterns that hide system state.
- Hardcode colors; always reference tokens to ensure accessibility and theme support.
