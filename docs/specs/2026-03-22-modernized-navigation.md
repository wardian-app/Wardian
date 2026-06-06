# Modernized Top-Bar & Sidebar Navigation

* **Status:** Implemented
* **Date:** 2026-03-17

## Context and Problem Statement
Wardian's current navigation is "nested" within the main content area, and sidebar controls use inconsistent floating arrows. To elevate the application to a "Pro-Tool" status (similar to Obsidian or VS Code), we need a global navigation layer that remains consistent across all views.

## Proposed Decision
We will implement a **Global Top Bar** and refactor the sidebar toggling system.

### 1. The Global Top Bar (`src/layout/TopBar.tsx`)
*   **Structure**: A fixed-height (e.g., 40-48px) bar at the very top of the window.
*   **Left Section**: Workspace branding and (optionally) the primary Sidebar Toggle.
*   **Center Section**: The primary **View Switcher** (Grid, Dashboard, Library, etc.). This ensures the user's primary navigation target is always in the same physical location.
*   **Right Section**:
    *   **Telemetry**: Real-time CPU/MEM usage and active agent count.
    *   **Secondary Sidebar Toggle**: For the Agent Watchlist.

### 2. Sidebar Control Refactor
*   **Icons**: Replace "Arrow" buttons with standard "Panel" icons.
*   **States**:
    *   **Left Sidebar**: Controls the visibility of both the `SidebarIconRail` and the `SidebarContentPane`.
    *   **Right Sidebar**: Controls the `AgentWatchlist`.
*   **Interactions**: Use CSS transitions for a "slide-and-fade" effect, ensuring the workspace feels fluid.

### 3. Layout Integration (`App.tsx`)
The `App.tsx` layout will shift to:
```jsx
<div className="h-screen flex flex-col">
  <TopBar />
  <div className="flex-1 flex overflow-hidden">
    <SidebarLeft />
    <MainContent />
    <SidebarRight />
  </div>
</div>
```

## Consequences
*   **Positive**: Significant reduction in visual clutter in the main content area.
*   **Positive**: Improved accessibility via consistent navigation positioning.
*   **Positive**: Professional, established UI patterns that users are already familiar with.
*   **Negative**: Requires a major refactor of `App.tsx` and related layout components.
*   **Negative**: Slightly reduces vertical space for terminal content (compensated by removing the current header).
