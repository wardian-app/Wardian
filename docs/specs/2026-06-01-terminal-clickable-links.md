# Terminal Clickable Links

Wardian terminals should behave like familiar embedded PTYs: URLs and file paths printed in terminal output become clickable links in both agent terminals and the bottom user terminal.

The implementation should use xterm's link provider API so hover, underline, pointer cursor, and click activation are owned by the terminal renderer. Link detection lives in a shared frontend terminal module. Agent terminals and the user terminal install the same provider when their xterm instance is created.

URLs open as URLs through the existing Tauri opener plugin. File paths open through Wardian's existing `open_in_external_editor` command with the current Settings-backed Explorer external editor configuration (`external_editor` and `external_editor_custom_executable`). Terminal file clicks do not use `explorer_file_click_action`; that setting controls Explorer row click behavior, while terminal file links map to Explorer's explicit **Open in External App** action.

The detector should support common terminal output forms:

- `https://example.com/path`
- `C:\repo\src\App.tsx:12:3`
- `C:/repo/src/App.tsx`
- `/home/user/repo/src/app.ts`
- `src/app.ts:12`
- `./src/app.ts`
- `../README.md`

Line and column suffixes should be accepted for detection but stripped before invoking `open_in_external_editor`, because the existing external-editor command receives a path. Relative paths should resolve against the terminal's known workspace when available: the agent visible workspace for agent terminals and the selected workspace for the user terminal.

Failures should be visible but non-disruptive. If a URL or file cannot be opened, the terminal surface should show a concise status/error message and keep the terminal usable.

Verification should cover shared link detection, URL activation, Settings-backed file activation, provider installation for both terminal surfaces, and the feature-specific frontend test suite.
