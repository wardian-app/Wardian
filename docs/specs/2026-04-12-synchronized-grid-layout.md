# Synchronized Grid Layout & Tactile Resizing

* **Status:** Proposed
* **Date:** 2026-04-12

## Context and Problem Statement
Current grid management in Wardian is limited to a simple `flex-wrap` layout with heavy whitespace gaps. To create a high-fidelity "Command Center" experience, we need a system that allows for **Direct Visual Manipulation** of a flush, zero-gap grid where terminals share edges.

## Proposed Decision
We will implement a **Track-Based Flush Grid** where agents sit flush against each other, allowing for synchronized track resizing via shared edges.

### 1. The Flush Tiling Model
*   **Zero Gap**: The grid container will use `gap-0` (or `gap-[1px]` depending on border rendering) to ensure cards are physically touching.
*   **Tiled Edges**: Internal card corners will be square (`rounded-none`) to create a continuous, sharp edge between terminals. Outer container corners may remain rounded to maintain the "Habitat" silhouette.
*   **Shared Gutter**: The single-pixel border between cards serves as the visual and functional "Wall" that users can grab and slide.

### 2. The Track-Based Data Model
The `GridView` maintains a `GridLayout` state:
*   **`column_tracks`**: Array of percentage widths (e.g., `[50, 50]`).
*   **`row_height`**: A single baseline height for all rows.

### 3. Dimension Policy & Constraint Solver
*   **Minimum Track Width**: 450px.
*   **Collision Detection**: Resizing is clamped if it would push any column track below the threshold.

### 4. Tactile Resizing (Shared Edges)
*   **Gutter Interaction**: Hovering over the shared border between terminals reveals a gold handle (`bg-wardian-accent/30`).
*   **Synchronized Movement**: Dragging the vertical gutter updates the percentage width for the entire column track.
*   **Snap Points**: Magnetic "soft locks" at 33.3%, 50.0%, 66.6%, and 100.0%.

### 5. Responsive Reversion
*   **Breakpoint**: 1000px window width.
*   **Behavior**: Reverts to a single-column stack (100% width) with standard margins to ensure terminal legibility on small screens.

## Consequences
*   **Positive**: Maximum screen utilization; zero wasted space.
*   **Positive**: High-end professional aesthetic similar to tiling window managers.
*   **Negative**: Requires careful CSS border-collapse logic to avoid "double-borders" (2px) between cards.
