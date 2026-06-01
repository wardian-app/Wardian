# Explorer File Click Action

Wardian's Explorer sidebar already supports file previews and opening files or folders in the configured external editor through the context menu. The default click behavior should now make files directly openable while keeping folder clicks focused on expanding and collapsing the tree.

Add a global Explorer setting named **File click action**. It controls what happens when a user clicks a file in the Explorer sidebar:

- **Preview in Wardian**: read the file through the existing preview command and show the current preview modal. This is the default because it gives fast local inspection without requiring an editor command or file association.
- **Open in external app**: invoke the existing external-editor backend command with the globally configured Explorer editor settings.

Folder rows continue to expand and collapse. They do not launch preview or external editor actions on normal click. The context menu remains unchanged, so users can still explicitly choose preview, external app, reveal, copy, or delete regardless of the click preference.

The setting is stored with sparse app overrides in `settings/app.json` as `explorer_file_click_action`. Invalid or missing values normalize to `preview`.

The Explorer tree should also remove the standalone folder glyph from directory rows. Expandability is already communicated by the chevron, and files should continue to show type-specific icons.

Verification should cover Settings persistence, Explorer click behavior for both actions, folder expansion without file opening, and the folder icon removal while preserving file icons.
