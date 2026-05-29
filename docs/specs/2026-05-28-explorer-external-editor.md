# Explorer External Editor

Wardian's Explorer context menu should let users open a file or folder in a local application directly from the tree. The default behavior is **System default app** because it respects each file type, works across operating systems, and does not require VS Code's `code` command to be installed on `PATH`.

Settings adds an **Explorer** category with an **External editor** row. Users can choose:

- **System default app**: open the selected path with the operating system's registered application.
- **VS Code (code command)**: launch VS Code's command-line entry point with the selected path.
- **Custom executable**: launch a user-provided executable with the selected path as its argument.

The setting is global and stored in `settings/app.json` as sparse app overrides:

```json
{
  "external_editor": "system",
  "external_editor_custom_executable": null
}
```

The Explorer right-click menu exposes **Open in External App** for files and folders. Backend launching stays in Rust so platform-specific process behavior is tested once and the frontend only passes the selected path plus the current editor setting. Launch failures are shown in Explorer so users can fix missing `code` commands, unavailable platform openers, or invalid custom executable paths.

Verification should cover frontend menu behavior, Settings rendering and persistence, backend command argument construction, and documentation updates for Explorer and Settings.
