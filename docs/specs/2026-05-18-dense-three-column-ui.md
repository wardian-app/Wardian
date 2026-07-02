# Dense Three-Column UI

Wardian's primary desktop layout target is a dense three-column Habitat view that remains comfortable at 1920x1080 across operating systems. Smaller windows should degrade through existing collapse and stacked-grid behavior, but the baseline density is desktop-first rather than mobile-first.

## Standards

- The activity rail, left content pane, right roster, grid gaps, card radius, and compact grid header dimensions are defined as CSS custom properties in `src/styles/App.css`.
- Persisted sidebar defaults live in `src/store/useLayoutStore.ts` and should match the CSS fallback widths.
- Side panels should consume the shared density padding variables instead of local fixed padding.
- Grid agent-card headers are single-row compact headers. The agent class remains visible inline beside the agent name.

## Current Defaults

- Activity rail width: `48px`
- Activity rail icon glyphs: `24px`
- Left content pane width: `240px`
- Right roster width: `240px`
- Grid gap: `6px`
- Grid card radius: `8px`
- Grid agent-card header minimum height: `44px`

## Follow-Up Tightening

Future density work should reuse these variables before adding local spacing. Good next candidates are roster row height, left-panel form spacing, dashboard row action controls, and terminal inset padding.
