# Remote Mobile Settings

## Context

Wardian Remote is a phone-first PWA that controls the desktop app through the
remote gateway. The desktop settings modal is too broad for the phone surface
and includes Tauri-only controls such as shell selection, external editors,
provider runtime policy, and remote access setup.

The mobile PWA still needs lightweight settings for appearance and repeated
agent-detail interaction. These settings should be reachable without crowding
the bottom navigation or changing desktop runtime behavior from a phone.

## Decisions

- Add Settings as a gear button in the remote watchlist header, beside refresh.
  Do not add Settings to the bottom navigation.
- Render Settings as a full-screen mobile view with its own Back button.
- Push a browser history state when Settings opens so the phone or browser back
  gesture closes Settings instead of leaving the installed PWA.
- Expose only mobile-safe preferences:
  - Theme: system, light, dark.
  - Agent detail default: terminal or chat.
  - Terminal text size.
- Keep desktop-only and runtime-affecting preferences out of this slice.

## Data Flow

Theme reuses the existing frontend settings store, but the remote PWA applies
the theme locally instead of calling Tauri settings commands. Agent-detail
default view and terminal text size are remote-specific settings stored in the
remote Zustand store with browser `localStorage` persistence.

## Testing

Browser unit tests cover:

- opening Settings from the watchlist header;
- closing Settings through browser history;
- changing theme and seeing `data-theme` update;
- changing the default agent detail view and opening an agent directly in that
  mode;
- applying terminal text size to the remote xterm instance.

The remote PWA browser smoke also opens Settings, applies the dark theme, and
captures a feature screenshot when screenshot output is enabled.
